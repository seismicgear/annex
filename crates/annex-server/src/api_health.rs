//! Liveness and readiness.
//!
//! `GET /health` used to be the only answer to "is this server working", and
//! it answered it by returning a literal. It never touched the database, the
//! connection pool, the storage gate or the Merkle tree — so a server whose
//! disk was full, whose pool was exhausted, or whose database file had been
//! deleted out from under it reported `{"status":"ok"}` and kept doing so. A
//! load balancer, a container orchestrator, or an operator at 3am would all
//! have been told the same untrue thing.
//!
//! That is this codebase's first defect class — a failure rendered as an
//! ordinary result — sitting on the endpoint whose entire job is to not do
//! that.
//!
//! The fix is two endpoints, because they answer two different questions and
//! conflating them is how a rolling deploy kills a healthy server:
//!
//! * [`live`] (`GET /health`, `GET /livez`) — is the process running? Cheap,
//!   dependency-free, and a `false` here means "restart me". Its response shape
//!   is UNCHANGED: `scripts/e2e-server.sh` polls it to decide the server has
//!   started, `client/e2e/startup.spec.ts` asserts on its body, and the
//!   puppeteer harness checks it before doing anything. Breaking it would break
//!   four things that have nothing to do with health checks.
//!
//! * [`ready`] (`GET /readyz`) — can it serve a request right now? It takes a
//!   pooled connection, runs a query, reads the storage gate and the Merkle
//!   root, and reports each check separately. A `false` here means "stop
//!   sending me traffic", which is a different instruction from "restart me":
//!   a server that is full, or whose pool is momentarily saturated, recovers
//!   on its own and must not be killed for it.
//!
//! Both are public and unauthenticated. That is deliberate — a probe that
//! needs a credential is a probe an orchestrator cannot run — and it is why
//! [`ready`] reports the SHAPE of a problem (`"degraded"`, which check failed)
//! and never the detail. An error string from `rusqlite` can carry a file path;
//! the count of identities is not something an unauthenticated caller needs.

use std::sync::Arc;

use axum::{extract::Extension, http::StatusCode, response::IntoResponse, Json};
use serde_json::{json, Value};

use crate::state::AppState;

/// Liveness. The process is up and answering.
///
/// Deliberately does no I/O: a liveness probe that touches the database will
/// fail a restart loop into existence the moment the database is slow, and
/// restarting the process does not fix a slow database.
pub async fn live(Extension(state): Extension<Arc<AppState>>) -> Json<Value> {
    Json(json!({
        "status": "ok",
        "version": env!("CARGO_PKG_VERSION"),
        "voice_enabled": state.voice_service.is_enabled()
    }))
}

/// One readiness check's outcome.
struct Check {
    name: &'static str,
    ok: bool,
    detail: &'static str,
}

/// Readiness. Every dependency a request will need is actually there.
///
/// Returns 200 with `"status":"ready"` when all checks pass, 503 with
/// `"status":"degraded"` otherwise — with the same body shape either way, so a
/// caller parses one thing.
pub async fn ready(Extension(state): Extension<Arc<AppState>>) -> impl IntoResponse {
    let mut checks: Vec<Check> = Vec::new();

    // The pool, and the database behind it. `get()` proves a connection is
    // available (an exhausted pool blocks here and times out, which is exactly
    // the condition worth reporting); the query proves the file is readable and
    // the schema is there. `spawn_blocking` because both are blocking calls on
    // an async runtime — the reason this endpoint is worth writing carefully is
    // that a naive version stalls the executor under the load it exists to
    // detect.
    let pool = state.pool.clone();
    let db = tokio::task::spawn_blocking(move || -> Result<(), String> {
        let conn = pool.get().map_err(|e| e.to_string())?;
        conn.query_row("SELECT count(*) FROM _annex_migrations", [], |r| {
            r.get::<_, i64>(0)
        })
        .map_err(|e| e.to_string())?;
        Ok(())
    })
    .await;

    match db {
        Ok(Ok(())) => checks.push(Check {
            name: "database",
            ok: true,
            detail: "reachable",
        }),
        Ok(Err(_)) => checks.push(Check {
            name: "database",
            ok: false,
            // No error text: rusqlite messages carry file paths, and this
            // endpoint is unauthenticated. The server log has the detail.
            detail: "query failed",
        }),
        Err(_) => checks.push(Check {
            name: "database",
            ok: false,
            detail: "probe panicked",
        }),
    }

    // The storage gate. When it is blocking writes the server is answering
    // reads and refusing mutations with 507 — which is degraded, not healthy,
    // and is precisely the state an operator wants a page for.
    let blocked = state.storage_health.writes_blocked();
    checks.push(Check {
        name: "storage",
        ok: !blocked,
        detail: if blocked {
            "writes blocked"
        } else {
            "accepting writes"
        },
    });

    // The Merkle tree backs every membership proof. If the lock is poisoned a
    // thread panicked while holding it and every authentication from here on
    // will fail — the server is alive and cannot do its job, which is the
    // distinction readiness exists to draw.
    let merkle_ok = state.merkle_tree.lock().is_ok();
    checks.push(Check {
        name: "merkle",
        ok: merkle_ok,
        detail: if merkle_ok {
            "readable"
        } else {
            "lock poisoned"
        },
    });

    let all_ok = checks.iter().all(|c| c.ok);
    let body = json!({
        "status": if all_ok { "ready" } else { "degraded" },
        "version": env!("CARGO_PKG_VERSION"),
        "checks": checks
            .iter()
            .map(|c| json!({ "name": c.name, "ok": c.ok, "detail": c.detail }))
            .collect::<Vec<_>>(),
    });

    let code = if all_ok {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    (code, Json(body))
}
