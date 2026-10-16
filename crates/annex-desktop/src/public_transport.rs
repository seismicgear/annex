//! Outbound, authenticated application transport for monolithannex.com.
//! The peer can address only the already-running embedded Annex listener.

use base64::{engine::general_purpose::STANDARD, Engine};
use ed25519_dalek::{Signer, SigningKey};
use futures_util::{SinkExt, StreamExt};
use rand::RngCore;
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{collections::HashMap, path::Path, time::Duration};
use tokio::sync::{mpsc, watch};
use tokio_tungstenite::{
    connect_async,
    tungstenite::{client::IntoClientRequest, Message},
};
use tokio_util::sync::CancellationToken;

const MAX_BODY: usize = 4 * 1024 * 1024;
const MAX_STREAMS: usize = 64;

pub(crate) struct Transport {
    pub(crate) public_url: String,
    pub(crate) connected: watch::Receiver<bool>,
    pub(crate) stop: CancellationToken,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Frame {
    Challenge {
        challenge: String,
    },
    Ready {
        #[serde(rename = "publicUrl")]
        public_url: String,
    },
    Http {
        id: String,
        method: String,
        path: String,
        headers: HashMap<String, String>,
        body: String,
    },
    HttpOpen {
        id: String,
        method: String,
        path: String,
        headers: HashMap<String, String>,
    },
    HttpData {
        id: String,
        data: String,
    },
    HttpEnd {
        id: String,
    },
    WsOpen {
        id: String,
        path: String,
        headers: HashMap<String, String>,
    },
    WsData {
        id: String,
        data: String,
        binary: bool,
    },
    WsClose {
        id: String,
    },
    Cancel {
        id: String,
    },
}

/// The routing key is separate from user identities. It stays alongside this
/// server's database across upgrades and ordinary uninstall/reinstall.
pub(crate) fn load_key(directory: &Path) -> Result<SigningKey, String> {
    use std::io::Write;
    let path = directory.join("public-host-key-v1");
    if !path.exists() {
        let mut key = [0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut key);
        let temporary = tempfile::NamedTempFile::new_in(directory).map_err(|e| e.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            temporary
                .as_file()
                .set_permissions(std::fs::Permissions::from_mode(0o600))
                .map_err(|e| e.to_string())?;
        }
        temporary
            .as_file()
            .write_all(&key)
            .map_err(|e| e.to_string())?;
        temporary.as_file().sync_all().map_err(|e| e.to_string())?;
        if let Err(error) = temporary.persist_noclobber(&path) {
            if error.error.kind() != std::io::ErrorKind::AlreadyExists {
                return Err(error.error.to_string());
            }
        }
    }
    let bytes =
        std::fs::read(path).map_err(|e| format!("Could not read the public hosting key: {e}"))?;
    let bytes: [u8; 32] = bytes.try_into().map_err(|_| "The public hosting key is damaged. Restore the server backup; do not replace its identity.".to_string())?;
    Ok(SigningKey::from_bytes(&bytes))
}

pub(crate) fn start(
    local_url: String,
    gateway: String,
    key: SigningKey,
) -> Result<Transport, String> {
    let local = url::Url::parse(&local_url).map_err(|e| e.to_string())?;
    if local.scheme() != "http"
        || local.host_str() != Some("127.0.0.1")
        || local.port().is_none()
        || local.path() != "/"
    {
        return Err("Public hosting requires the running loopback Annex listener".into());
    }
    let origin = url::Url::parse(&gateway).map_err(|e| e.to_string())?;
    let local_test = origin.scheme() == "http" && origin.host_str() == Some("127.0.0.1");
    if (origin.scheme() != "https" && !local_test)
        || !origin.username().is_empty()
        || origin.password().is_some()
        || origin.path() != "/"
        || origin.query().is_some()
        || origin.fragment().is_some()
    {
        return Err("The hosting gateway must be an HTTPS origin".into());
    }
    let host = hex::encode(Sha256::digest(key.verifying_key().as_bytes()));
    let public_url = format!("{}/host/{host}", gateway.trim_end_matches('/'));
    let mut socket_url = origin;
    socket_url
        .set_scheme(if local_test { "ws" } else { "wss" })
        .map_err(|_| "Invalid gateway scheme")?;
    socket_url.set_path("/api/host/connect");
    let (status, connected) = watch::channel(false);
    let stop = CancellationToken::new();
    let task_stop = stop.clone();
    let expected = public_url.clone();
    tokio::spawn(async move {
        let mut backoff = 1u64;
        while !task_stop.is_cancelled() {
            let result = tokio::select! {
                _ = task_stop.cancelled() => break,
                result = connection(socket_url.as_str(), &local_url, &expected, &key, &status, &task_stop) => result,
            };
            let was_connected = *status.borrow();
            let _ = status.send(false);
            if result.is_err() {
                tracing::warn!("Public hosting connection interrupted; reconnecting");
            }
            if was_connected {
                backoff = 1;
            }
            tokio::select! { _ = task_stop.cancelled() => break, _ = tokio::time::sleep(Duration::from_secs(backoff)) => {} }
            backoff = (backoff * 2).min(30);
        }
        let _ = status.send(false);
    });
    Ok(Transport {
        public_url,
        connected,
        stop,
    })
}

fn valid_path(path: &str) -> bool {
    let pathname = path.split('?').next().unwrap_or_default();
    path.len() <= 8192
        && !path
            .chars()
            .any(|c| c.is_control() || c.is_whitespace() || c == '\\')
        && (pathname.starts_with("/api/")
            || pathname.starts_with("/uploads/")
            || matches!(pathname, "/ws" | "/health" | "/ready"))
        && !pathname.contains('%')
        && !pathname.contains("/.")
        && !pathname.contains("//")
        && !path.contains('#')
}

fn valid_id(id: &str) -> bool {
    uuid::Uuid::parse_str(id).is_ok() && id.len() == 36
}

fn allowed_header(name: &str) -> bool {
    matches!(
        name,
        "authorization"
            | "content-type"
            | "accept"
            | "accept-encoding"
            | "origin"
            | "range"
            | "if-none-match"
            | "if-modified-since"
    ) || name.starts_with("x-annex-")
}

async fn send(out: &mpsc::Sender<Value>, value: Value) -> Result<(), String> {
    tokio::time::timeout(Duration::from_secs(10), out.send(value))
        .await
        .map_err(|_| "Public host transport is congested")?
        .map_err(|_| "Public host transport disconnected".into())
}

async fn connection(
    socket_url: &str,
    local: &str,
    expected: &str,
    key: &SigningKey,
    status: &watch::Sender<bool>,
    stop: &CancellationToken,
) -> Result<(), String> {
    let (mut socket, _) = tokio::time::timeout(Duration::from_secs(15), connect_async(socket_url))
        .await
        .map_err(|_| "Gateway connection timed out")?
        .map_err(|e| e.to_string())?;
    let challenge = tokio::time::timeout(Duration::from_secs(5), socket.next())
        .await
        .map_err(|_| "Gateway authentication timed out")?
        .ok_or("Gateway disconnected")?
        .map_err(|e| e.to_string())?;
    let Frame::Challenge { challenge } =
        serde_json::from_slice(&challenge.into_data()).map_err(|e| e.to_string())?
    else {
        return Err("Invalid gateway challenge".into());
    };
    if challenge.len() != 64 || !challenge.bytes().all(|c| c.is_ascii_hexdigit()) {
        return Err("Invalid gateway challenge".into());
    }
    let signature = key.sign(format!("annex-public-host-v1:{challenge}").as_bytes());
    socket.send(Message::Text(json!({ "type": "authenticate", "publicKey": hex::encode(key.verifying_key().as_bytes()), "signature": hex::encode(signature.to_bytes()) }).to_string().into())).await.map_err(|e| e.to_string())?;
    let ready = tokio::time::timeout(Duration::from_secs(10), socket.next())
        .await
        .map_err(|_| "Gateway registration timed out")?
        .ok_or("Gateway disconnected")?
        .map_err(|e| e.to_string())?;
    let Frame::Ready { public_url } =
        serde_json::from_slice(&ready.into_data()).map_err(|e| e.to_string())?
    else {
        return Err("Gateway rejected host registration".into());
    };
    if public_url != expected {
        return Err("Gateway returned a different host identity".into());
    }

    let (mut writer, mut reader) = socket.split();
    let (output, mut outgoing) = mpsc::channel::<Value>(64);
    let session_stop = stop.child_token();
    let _cancel_on_drop = session_stop.clone().drop_guard();
    let writer_stop = session_stop.clone();
    let writer_task = tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = writer_stop.cancelled() => { let _ = writer.close().await; break; },
                value = outgoing.recv() => match value {
                    Some(value) => if writer.send(Message::Text(value.to_string().into())).await.is_err() { break; },
                    None => break,
                }
            }
        }
        writer_stop.cancel();
    });
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(120))
        .build()
        .map_err(|e| e.to_string())?;
    let mut streams: HashMap<String, (CancellationToken, Option<mpsc::Sender<Message>>)> =
        HashMap::new();
    let mut uploads: HashMap<String, (mpsc::Sender<Vec<u8>>, usize)> = HashMap::new();
    let (finished, mut completed) = mpsc::unbounded_channel::<String>();
    let _ = status.send(true);
    loop {
        tokio::select! {
            _ = session_stop.cancelled() => break,
            _ = tokio::time::sleep(Duration::from_secs(35)) => break,
            Some(id) = completed.recv() => { streams.remove(&id); uploads.remove(&id); },
            incoming = reader.next() => {
                let Some(Ok(message)) = incoming else { break; };
                if matches!(message, Message::Close(_)) { break; }
                // Tungstenite queues control replies itself. Only JSON data is
                // an application instruction; no control payload is parsed.
                if !matches!(message, Message::Text(_)) { continue; }
                if message.len() > 6 * 1024 * 1024 { break; }
                let Ok(frame) = serde_json::from_slice::<Frame>(&message.into_data()) else { break; };
                match frame {
                    Frame::HttpOpen { id, method, path, headers } => {
                        if !valid_id(&id) || streams.contains_key(&id) { break; }
                        if streams.len() >= MAX_STREAMS || !valid_path(&path) || !path.starts_with("/api/") || !matches!(method.as_str(), "POST" | "PUT" | "PATCH") {
                            let _ = send(&output, json!({"type":"error", "id":id})).await; continue;
                        }
                        let cancel = session_stop.child_token();
                        let (input, receiver) = mpsc::channel::<Vec<u8>>(2);
                        uploads.insert(id.clone(), (input, 0)); streams.insert(id.clone(), (cancel.clone(), None));
                        let out = output.clone(); let client = client.clone(); let local = local.to_owned(); let done = finished.clone();
                        tokio::spawn(async move {
                            let ack_out = out.clone(); let ack_id = id.clone();
                            let body = futures_util::stream::unfold((receiver, ack_out, ack_id), |(mut receiver, out, id)| async move {
                                let bytes = receiver.recv().await?;
                                if send(&out, json!({"type":"request_ack", "id":id})).await.is_err() { return None; }
                                Some((Ok::<_, std::io::Error>(bytes), (receiver, out, id)))
                            });
                            let result = tokio::select! {
                                _ = cancel.cancelled() => Ok(()),
                                result = async {
                                    send(&out, json!({"type":"request_ready", "id":id})).await?;
                                    proxy_http_body(&client, &local, &id, RequestHead { method, path, headers }, reqwest::Body::wrap_stream(body), &out).await
                                } => result,
                            };
                            if result.is_err() { let _ = send(&out, json!({"type":"error", "id":id})).await; }
                            let _ = done.send(id);
                        });
                    },
                    Frame::HttpData { id, data } => {
                        if let Some((input, total)) = uploads.get_mut(&id) {
                            let bytes = STANDARD.decode(data).ok();
                            let valid = bytes.as_ref().is_some_and(|bytes| bytes.len() <= 64 * 1024 && *total + bytes.len() <= 50 * 1024 * 1024 + 64 * 1024);
                            if valid {
                                let bytes = bytes.unwrap(); *total += bytes.len();
                                if input.try_send(bytes).is_ok() { continue; }
                            }
                            if let Some((cancel, _)) = streams.get(&id) { cancel.cancel(); }
                            uploads.remove(&id);
                            let _ = send(&output, json!({"type":"error", "id":id})).await;
                        }
                    },
                    Frame::HttpEnd { id } => { uploads.remove(&id); },
                    Frame::Http { id, method, path, headers, body } => {
                        if !valid_id(&id) || streams.contains_key(&id) { break; }
                        if streams.len() >= MAX_STREAMS || !valid_path(&path) { let _ = send(&output, json!({"type":"error", "id":id})).await; continue; }
                        let cancel = session_stop.child_token(); streams.insert(id.clone(), (cancel.clone(), None));
                        let out = output.clone(); let client = client.clone(); let local = local.to_owned(); let done = finished.clone();
                        tokio::spawn(async move {
                            let result = tokio::select! { _ = cancel.cancelled() => Ok(()), result = proxy_http(&client, &local, &id, RequestHead { method, path, headers }, body, &out) => result };
                            if result.is_err() { let _ = send(&out, json!({"type":"error", "id":id})).await; }
                            let _ = done.send(id);
                        });
                    },
                    Frame::WsOpen { id, path, headers } => {
                        if !valid_id(&id) || streams.contains_key(&id) { break; }
                        if streams.len() >= MAX_STREAMS || !valid_path(&path) || !matches!(path.split('?').next(), Some("/ws" | "/api/ws")) {
                            let _ = send(&output, json!({"type":"error", "id":id})).await; continue;
                        }
                        let cancel = session_stop.child_token(); let (input, incoming) = mpsc::channel(32);
                        streams.insert(id.clone(), (cancel.clone(), Some(input)));
                        let out = output.clone(); let local = local.to_owned(); let done = finished.clone();
                        tokio::spawn(async move {
                            let _ = tokio::select! { _ = cancel.cancelled() => Ok(()), result = proxy_socket(&local, &id, path, headers, incoming, &out) => result };
                            let _ = send(&out, json!({"type":"ws_close", "id":id})).await;
                            let _ = done.send(id);
                        });
                    },
                    Frame::WsData { id, data, binary } => {
                        if let Some((cancel, Some(input))) = streams.get(&id) {
                            let message = STANDARD.decode(data).ok().and_then(|bytes| {
                                if bytes.len() > MAX_BODY { return None; }
                                if binary { Some(Message::Binary(bytes.into())) } else { String::from_utf8(bytes).ok().map(|s| Message::Text(s.into())) }
                            });
                            if message.is_none_or(|message| input.try_send(message).is_err()) { cancel.cancel(); }
                        }
                    },
                    Frame::WsClose { id } | Frame::Cancel { id } => { uploads.remove(&id); if let Some((cancel, _)) = streams.remove(&id) { cancel.cancel(); } },
                    Frame::Challenge { .. } | Frame::Ready { .. } => break,
                }
            }
        }
    }
    session_stop.cancel();
    for (cancel, _) in streams.values() {
        cancel.cancel();
    }
    writer_task.abort();
    let _ = writer_task.await;
    Err("Gateway connection ended".into())
}

struct RequestHead {
    method: String,
    path: String,
    headers: HashMap<String, String>,
}

async fn proxy_http(
    client: &reqwest::Client,
    local: &str,
    id: &str,
    head: RequestHead,
    body: String,
    out: &mpsc::Sender<Value>,
) -> Result<(), String> {
    if !matches!(
        head.method.as_str(),
        "GET" | "HEAD" | "POST" | "PUT" | "PATCH" | "DELETE" | "OPTIONS"
    ) {
        return Err("Unsupported method".into());
    }
    let bytes = STANDARD.decode(body).map_err(|e| e.to_string())?;
    if bytes.len() > MAX_BODY {
        return Err("Request too large".into());
    }
    proxy_http_body(client, local, id, head, bytes.into(), out).await
}

async fn proxy_http_body(
    client: &reqwest::Client,
    local: &str,
    id: &str,
    head: RequestHead,
    body: reqwest::Body,
    out: &mpsc::Sender<Value>,
) -> Result<(), String> {
    let mut request = client.request(
        head.method
            .parse::<reqwest::Method>()
            .map_err(|e| e.to_string())?,
        format!("{local}{}", head.path),
    );
    for (name, value) in head.headers {
        if allowed_header(&name) && value.len() <= 16_384 {
            request = request.header(name, value);
        }
    }
    let response = request.body(body).send().await.map_err(|e| e.to_string())?;
    let headers: HashMap<_, _> = response
        .headers()
        .iter()
        .filter_map(|(name, value)| {
            value
                .to_str()
                .ok()
                .map(|value| (name.to_string(), value.to_string()))
        })
        .collect();
    send(
        out,
        json!({"type":"response", "id":id, "status":response.status().as_u16(), "headers":headers}),
    )
    .await?;
    let mut body = response.bytes_stream();
    while let Some(bytes) = body.next().await {
        let bytes = bytes.map_err(|e| e.to_string())?;
        for chunk in bytes.chunks(64 * 1024) {
            send(
                out,
                json!({"type":"data", "id":id, "data":STANDARD.encode(chunk)}),
            )
            .await?;
        }
    }
    send(out, json!({"type":"end", "id":id})).await
}

async fn proxy_socket(
    local: &str,
    id: &str,
    path: String,
    headers: HashMap<String, String>,
    mut input: mpsc::Receiver<Message>,
    out: &mpsc::Sender<Value>,
) -> Result<(), String> {
    let url = format!("ws://{}{path}", local.trim_start_matches("http://"));
    let mut request = url.into_client_request().map_err(|e| e.to_string())?;
    for (name, value) in headers {
        if allowed_header(&name) && value.len() <= 16_384 {
            request.headers_mut().insert(
                name.parse::<tokio_tungstenite::tungstenite::http::HeaderName>()
                    .map_err(|e| e.to_string())?,
                value.parse().map_err(|_| "Invalid header")?,
            );
        }
    }
    let (mut socket, _) = tokio::time::timeout(Duration::from_secs(10), connect_async(request))
        .await
        .map_err(|_| "Hosted websocket timed out")?
        .map_err(|e| e.to_string())?;
    send(out, json!({"type":"ws_ready", "id":id})).await?;
    loop {
        tokio::select! {
            message = input.recv() => { let Some(message) = message else { break; }; socket.send(message).await.map_err(|e| e.to_string())?; },
            message = socket.next() => {
                let Some(Ok(message)) = message else { break; };
                let binary = message.is_binary();
                if message.is_text() || binary {
                    if message.len() > MAX_BODY { return Err("Hosted websocket frame too large".into()); }
                    send(out, json!({"type":"ws_data", "id":id, "data":STANDARD.encode(message.into_data()), "binary":binary})).await?;
                } else if message.is_close() { break; }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Run explicitly against two independent Node relay processes and real
    /// Redis. This exercises the shipping Rust connector, not a reference peer.
    #[tokio::test]
    #[ignore = "requires ANNEX_TEST_HOST_RELAY_URL and ANNEX_TEST_HOST_VISITOR_URL"]
    async fn real_relay_routes_native_http_websockets_and_reconnects() {
        use axum::{
            body::Bytes,
            extract::{State, WebSocketUpgrade},
            http::HeaderMap,
            routing::{get, post},
            Json, Router,
        };
        async fn echo(State(slug): State<String>, headers: HeaderMap, body: Bytes) -> Json<Value> {
            Json(
                json!({"slug":slug, "body":String::from_utf8_lossy(&body), "authorization":headers.get("authorization").and_then(|v| v.to_str().ok())}),
            )
        }
        async fn echo_socket(upgrade: WebSocketUpgrade) -> impl axum::response::IntoResponse {
            upgrade.on_upgrade(|mut socket| async move {
                while let Some(Ok(message)) = socket.next().await {
                    if socket.send(message).await.is_err() {
                        break;
                    }
                }
            })
        }
        async fn upload(headers: HeaderMap, body: Bytes) -> Json<Value> {
            Json(
                json!({"bytes":body.len(), "sha256":hex::encode(Sha256::digest(&body)), "authorization":headers.get("authorization").and_then(|v| v.to_str().ok())}),
            )
        }
        let gateway = std::env::var("ANNEX_TEST_HOST_RELAY_URL").expect("relay URL required");
        let visitor =
            std::env::var("ANNEX_TEST_HOST_VISITOR_URL").expect("independent visitor URL required");
        assert_ne!(
            gateway, visitor,
            "the native test must cross two relay processes"
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let local = format!("http://{}", listener.local_addr().unwrap());
        let app = Router::new()
            .route("/api/test", post(echo))
            .route(
                "/api/upload",
                post(upload).layer(axum::extract::DefaultBodyLimit::max(51 * 1024 * 1024)),
            )
            .route("/ws", get(echo_socket))
            .with_state("native-target".to_owned());
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let dir = tempfile::tempdir().unwrap();
        let mut transport = start(
            local.clone(),
            gateway.clone(),
            load_key(dir.path()).unwrap(),
        )
        .unwrap();
        let _stop = transport.stop.clone().drop_guard();
        tokio::time::timeout(
            Duration::from_secs(15),
            transport.connected.wait_for(|ready| *ready),
        )
        .await
        .unwrap()
        .unwrap();
        let path = url::Url::parse(&transport.public_url)
            .unwrap()
            .path()
            .to_owned();
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .unwrap();
        let request = || {
            client
                .post(format!("{visitor}{path}/api/test"))
                .header("authorization", "Bearer native-test-only")
                .body("actual Rust connector")
                .send()
        };
        let response: Value = request()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(
            response,
            json!({"slug":"native-target", "body":"actual Rust connector", "authorization":"Bearer native-test-only"})
        );
        let (mut socket, _) =
            connect_async(format!("{}{path}/ws", visitor.replace("http:", "ws:")))
                .await
                .unwrap();
        socket
            .send(Message::Text("independent native recipient".into()))
            .await
            .unwrap();
        let echo = tokio::time::timeout(Duration::from_secs(10), socket.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(echo.to_text().unwrap(), "independent native recipient");
        socket.close(None).await.unwrap();
        let (mut upload_socket, _) = connect_async(format!(
            "{}{path}/__request",
            visitor.replace("http:", "ws:")
        ))
        .await
        .unwrap();
        upload_socket.send(Message::Text(json!({"type":"http_request", "method":"POST", "path":"/api/upload", "headers":{"authorization":"Bearer streamed-test-only", "content-type":"application/octet-stream"}}).to_string().into())).await.unwrap();
        async fn frame(
            socket: &mut tokio_tungstenite::WebSocketStream<
                tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
            >,
        ) -> Value {
            loop {
                let message = tokio::time::timeout(Duration::from_secs(10), socket.next())
                    .await
                    .unwrap()
                    .unwrap()
                    .unwrap();
                if message.is_text() {
                    return serde_json::from_slice(&message.into_data()).unwrap();
                }
            }
        }
        assert_eq!(frame(&mut upload_socket).await["type"], "request_ready");
        let file = vec![0x5a; 5 * 1024 * 1024];
        for chunk in file.chunks(64 * 1024) {
            upload_socket
                .send(Message::Text(
                    json!({"type":"http_data", "data":STANDARD.encode(chunk)})
                        .to_string()
                        .into(),
                ))
                .await
                .unwrap();
            assert_eq!(frame(&mut upload_socket).await["type"], "request_ack");
        }
        upload_socket
            .send(Message::Text(json!({"type":"http_end"}).to_string().into()))
            .await
            .unwrap();
        assert_eq!(frame(&mut upload_socket).await["status"], 200);
        let mut result = Vec::new();
        loop {
            let next = frame(&mut upload_socket).await;
            if next["type"] == "end" {
                break;
            }
            assert_eq!(next["type"], "data");
            result.extend(STANDARD.decode(next["data"].as_str().unwrap()).unwrap());
        }
        let result: Value = serde_json::from_slice(&result).unwrap();
        assert_eq!(
            result,
            json!({"bytes":file.len(), "sha256":hex::encode(Sha256::digest(&file)), "authorization":"Bearer streamed-test-only"})
        );
        // Two heartbeat periods with no application traffic must not break the
        // native connection or falsely leave its readiness flag true.
        tokio::time::sleep(Duration::from_secs(25)).await;
        assert!(*transport.connected.borrow());
        request().await.unwrap().error_for_status().unwrap();
        transport.stop.cancel();
        tokio::time::timeout(
            Duration::from_secs(5),
            transport.connected.wait_for(|ready| !*ready),
        )
        .await
        .unwrap()
        .unwrap();
        let mut restored = start(local, gateway, load_key(dir.path()).unwrap()).unwrap();
        let _stop_restored = restored.stop.clone().drop_guard();
        assert_eq!(restored.public_url, transport.public_url);
        tokio::time::timeout(
            Duration::from_secs(15),
            restored.connected.wait_for(|ready| *ready),
        )
        .await
        .unwrap()
        .unwrap();
        request().await.unwrap().error_for_status().unwrap();
        restored.stop.cancel();
        server.abort();
    }
    #[test]
    fn route_key_survives_relaunch_and_is_not_regenerated_when_damaged() {
        let dir = tempfile::tempdir().unwrap();
        let key = load_key(dir.path()).unwrap().to_bytes();
        assert_eq!(key, load_key(dir.path()).unwrap().to_bytes());
        std::fs::write(dir.path().join("public-host-key-v1"), b"damaged").unwrap();
        assert!(load_key(dir.path()).is_err());
    }
    #[test]
    fn peer_cannot_forward_to_other_origins_or_non_application_paths() {
        for path in [
            "https://evil.example/api/registry",
            "//evil.example/api/registry",
            "/api/../private",
            "/api/%2e%2e/private",
            "/api//private",
            "/api/\\private",
            "/index.html",
            "/api/a\nHost:evil",
        ] {
            assert!(!valid_path(path), "{path}");
        }
        for path in [
            "/api/public/server/summary",
            "/api/registry/register",
            "/ws?token=one%2Ftwo",
            "/health",
        ] {
            assert!(valid_path(path), "{path}");
        }
    }
}
