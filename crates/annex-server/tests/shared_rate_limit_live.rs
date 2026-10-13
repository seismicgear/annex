//! Deliberately requires a real Redis primary. Run the ignored lane explicitly
//! with ANNEX_TEST_REDIS_URL; absence is a failure, never a silent pass.
//! Three child processes serve actual Axum HTTP requests against one budget.
mod common;

use annex_server::middleware::{RateLimitCategory, RateLimitKey, RateLimiter};
use annex_server::shared_rate_limit::SharedRateLimiter;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

struct ServerProcess(Child);
impl Drop for ServerProcess {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

async fn spawn_server(endpoint: &str, namespace: &str) -> (ServerProcess, String) {
    let directory = tempfile::tempdir().unwrap();
    let readiness = directory.path().join("ready.json");
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--ignored", "--exact", "redis_http_worker", "--nocapture"])
        .env("ANNEX_TEST_REDIS_WORKER_URL", endpoint)
        .env("ANNEX_TEST_REDIS_WORKER_NAMESPACE", namespace)
        .env("ANNEX_TEST_REDIS_WORKER_READY", &readiness)
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let mut process = ServerProcess(command.spawn().unwrap());
    let url = tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            assert!(
                process.0.try_wait().unwrap().is_none(),
                "HTTP worker exited before readiness"
            );
            if let Ok(bytes) = std::fs::read(&readiness) {
                if let Ok(url) = serde_json::from_slice::<String>(&bytes) {
                    break url;
                }
            }
            tokio::time::sleep(Duration::from_millis(40)).await;
        }
    })
    .await
    .expect("HTTP worker readiness deadline");
    (process, url)
}

#[tokio::test]
#[ignore = "child process fixture for actual Redis lane"]
async fn redis_http_worker() {
    let endpoint = std::env::var("ANNEX_TEST_REDIS_WORKER_URL").expect("worker endpoint required");
    let namespace = std::env::var("ANNEX_TEST_REDIS_WORKER_NAMESPACE").unwrap();
    let readiness = std::env::var("ANNEX_TEST_REDIS_WORKER_READY").unwrap();
    let pool = annex_db::create_pool(":memory:", Default::default()).unwrap();
    annex_db::run_migrations(&pool.get().unwrap()).unwrap();
    let mut policy = annex_types::ServerPolicy::default();
    policy.rate_limit.default_limit = 12;
    let mut state =
        common::build_app_state(pool, annex_identity::MerkleTree::new(20).unwrap(), policy);
    state.rate_limiter = RateLimiter::from_shared(
        SharedRateLimiter::connect(&endpoint, &namespace)
            .await
            .unwrap(),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    std::fs::write(readiness, serde_json::to_vec(&url).unwrap()).unwrap();
    axum::serve(
        listener,
        annex_server::app(state).into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .await
    .unwrap();
}

#[tokio::test]
#[ignore = "requires actual Redis: ANNEX_TEST_REDIS_URL"]
async fn actual_redis_three_processes_share_budget_restart_and_fail_closed() {
    let endpoint = std::env::var("ANNEX_TEST_REDIS_URL").expect("real Redis endpoint is mandatory");
    let namespace = format!("annex-live-{}", uuid::Uuid::new_v4());
    let account = format!("annex-test-{}", uuid::Uuid::new_v4());
    let password = uuid::Uuid::new_v4().to_string();
    let mut admin = redis::Client::open(endpoint.as_str())
        .unwrap()
        .get_multiplexed_async_connection()
        .await
        .unwrap();
    redis::cmd("ACL")
        .arg("SETUSER")
        .arg(&account)
        .arg("on")
        .arg(format!(">{password}"))
        .arg("~annex:admission:v1:*")
        .arg("+@all")
        .query_async::<()>(&mut admin)
        .await
        .unwrap();
    let mut worker_url = url::Url::parse(&endpoint).unwrap();
    worker_url.set_username(&account).unwrap();
    worker_url.set_password(Some(&password)).unwrap();
    let worker_endpoint = worker_url.as_str();
    let (first, first_url) = spawn_server(worker_endpoint, &namespace).await;
    let (second, second_url) = spawn_server(worker_endpoint, &namespace).await;
    let (_third, third_url) = spawn_server(worker_endpoint, &namespace).await;
    let urls = [first_url, second_url, third_url.clone()];
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();
    let mut tasks = Vec::new();
    for index in 0..30 {
        let client = client.clone();
        let url = format!("{}/api/registry/current-root", urls[index % 3]);
        tasks.push(tokio::spawn(async move {
            client.get(url).send().await.unwrap()
        }));
    }
    let mut accepted = 0;
    let mut refused = 0;
    for task in tasks {
        let response = task.await.unwrap();
        match response.status().as_u16() {
            200 => accepted += 1,
            429 => {
                refused += 1;
                assert_eq!(response.headers()["retry-after"], "60");
            }
            other => panic!("unexpected actual HTTP status {other}"),
        }
    }
    assert_eq!(
        (accepted, refused),
        (12, 18),
        "one quota across three processes"
    );
    // Probe requests use a distinct category, even when ordinary IP quota is exhausted.
    assert_eq!(
        client
            .get(format!("{third_url}/health"))
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    drop(first);
    drop(second);
    let (_restarted, restarted_url) = spawn_server(worker_endpoint, &namespace).await;
    assert_eq!(
        client
            .get(format!("{restarted_url}/api/registry/current-root"))
            .send()
            .await
            .unwrap()
            .status(),
        429
    );
    // Withdraw the store's actual scripting permission. Existing connections
    // must return503 instead of granting local budgets or pretending this is429.
    redis::cmd("ACL")
        .arg("SETUSER")
        .arg(&account)
        .arg("-eval")
        .arg("-evalsha")
        .query_async::<()>(&mut admin)
        .await
        .unwrap();
    assert_eq!(
        client
            .get(format!("{third_url}/api/registry/current-root"))
            .send()
            .await
            .unwrap()
            .status(),
        503
    );
    redis::cmd("ACL")
        .arg("SETUSER")
        .arg(&account)
        .arg("+eval")
        .arg("+evalsha")
        .query_async::<()>(&mut admin)
        .await
        .unwrap();
    assert_eq!(
        client
            .get(format!("{third_url}/api/registry/current-root"))
            .send()
            .await
            .unwrap()
            .status(),
        429
    );
    redis::cmd("ACL")
        .arg("DELUSER")
        .arg(&account)
        .query_async::<u64>(&mut admin)
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires actual Redis: ANNEX_TEST_REDIS_URL"]
async fn actual_redis_namespaces_categories_and_identities_are_independent() {
    let endpoint = std::env::var("ANNEX_TEST_REDIS_URL").expect("real Redis endpoint is mandatory");
    let namespace = format!("annex-isolation-{}", uuid::Uuid::new_v4());
    let first = SharedRateLimiter::connect(&endpoint, &namespace)
        .await
        .unwrap();
    let second = SharedRateLimiter::connect(&endpoint, &namespace)
        .await
        .unwrap();
    let separate = SharedRateLimiter::connect(&endpoint, &format!("{namespace}-other"))
        .await
        .unwrap();
    let pseudonym = RateLimitKey::Pseudonym("127.0.0.1".into(), RateLimitCategory::Default);
    assert!(first.check(&pseudonym, 1).await.unwrap());
    assert!(!second.check(&pseudonym, 1).await.unwrap());
    assert!(separate.check(&pseudonym, 1).await.unwrap());
    assert!(second
        .check(
            &RateLimitKey::Ip("127.0.0.1".parse().unwrap(), RateLimitCategory::Default),
            1
        )
        .await
        .unwrap());
    assert!(second
        .check(
            &RateLimitKey::Pseudonym("127.0.0.1".into(), RateLimitCategory::Verification),
            1
        )
        .await
        .unwrap());
    assert!(!format!("{first:?}").contains(&endpoint));
}
