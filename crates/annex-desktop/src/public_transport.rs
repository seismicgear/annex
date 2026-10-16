//! Outbound, authenticated application transport for monolithannex.com.
//! The peer can address only the already-running embedded Annex listener.

use annex_federation::host_tunnel_crypto::{Cipher, ClientHandshake, Packet};
use base64::{engine::general_purpose::STANDARD, Engine};
use ed25519_dalek::{Signer, SigningKey};
use futures_util::{SinkExt, StreamExt};
use rand::RngCore;
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    path::Path,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::{mpsc, watch};
use tokio_tungstenite::{
    connect_async,
    tungstenite::{client::IntoClientRequest, Message},
};
use tokio_util::sync::CancellationToken;

const MAX_BODY: usize = 4 * 1024 * 1024;
const MAX_STREAMS: usize = 64;
const TRANSFER_WINDOW: usize = 32;

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
        protocol: String,
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
    ResponseAck {
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
            if let Err(reason) = result {
                tracing::warn!(%reason, "Public hosting connection interrupted; reconnecting");
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
    let Frame::Ready {
        public_url,
        protocol,
    } = serde_json::from_slice(&ready.into_data()).map_err(|e| e.to_string())?
    else {
        return Err("Gateway rejected host registration".into());
    };
    if public_url != expected || protocol != "annex-content-blind-v2" {
        return Err("Gateway returned a different host identity".into());
    }

    let (mut writer, mut reader) = socket.split();
    let (output, mut outgoing) = mpsc::channel::<Value>(64);
    let ciphers: Arc<Mutex<HashMap<String, Cipher>>> = Arc::new(Mutex::new(HashMap::new()));
    let writer_ciphers = ciphers.clone();
    let session_stop = stop.child_token();
    let _cancel_on_drop = session_stop.clone().drop_guard();
    let writer_stop = session_stop.clone();
    let writer_task = tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = writer_stop.cancelled() => { let _ = writer.close().await; break; },
                value = outgoing.recv() => match value {
                    Some(value) => {
                        let wire = if matches!(value.get("type").and_then(Value::as_str), Some("tunnel_hello" | "tunnel_error")) {
                            Ok(vec![value])
                        } else if let Some(id) = value.get("id").and_then(Value::as_str) {
                            writer_ciphers.lock().map_err(|_| "Tunnel lock failed".to_string()).and_then(|mut sessions| {
                                let Some(cipher) = sessions.get_mut(id) else { return Ok(Vec::new()); };
                                cipher.seal(&value).map(|packets| packets.into_iter().map(|packet| json!({"type":"tunnel_data", "id":id, "sequence":packet.sequence, "data":packet.data})).collect())
                            })
                        } else { Err("Missing tunnel address".into()) };
                        let Ok(packets) = wire else { break; };
                        let mut failed = false;
                        for packet in packets {
                            if writer.send(Message::Text(packet.to_string().into())).await.is_err() { failed = true; break; }
                        }
                        if failed { break; }
                    },
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
    let mut downloads: HashMap<String, mpsc::Sender<()>> = HashMap::new();
    let (finished, mut completed) = mpsc::unbounded_channel::<String>();
    let _ = status.send(true);
    let mut disconnect = "Gateway connection ended".to_string();
    loop {
        tokio::select! {
            _ = session_stop.cancelled() => break,
            _ = tokio::time::sleep(Duration::from_secs(35)) => break,
            Some(id) = completed.recv() => { streams.remove(&id); uploads.remove(&id); downloads.remove(&id); },
            incoming = reader.next() => {
                let Some(Ok(message)) = incoming else { break; };
                if let Message::Close(frame) = message {
                    if let Some(frame) = frame {
                        // The outer close code and the gateway's fixed reason
                        // contain no application content or credentials.
                        disconnect = format!("Gateway closed {}: {}", frame.code, frame.reason);
                    }
                    break;
                }
                // Tungstenite queues control replies itself. Only JSON data is
                // an application instruction; no control payload is parsed.
                if !matches!(message, Message::Text(_)) { continue; }
                if message.len() > 128 * 1024 { break; }
                let Ok(wire) = serde_json::from_slice::<Value>(&message.into_data()) else { break; };
                let Some(id) = wire.get("id").and_then(Value::as_str).filter(|id| valid_id(id)).map(str::to_owned) else { break; };
                let frame = match wire.get("type").and_then(Value::as_str) {
                    Some("tunnel_open") => {
                        let accepted = {
                            let mut sessions = ciphers.lock().map_err(|_| "Tunnel lock failed")?;
                            if sessions.len() >= MAX_STREAMS || sessions.contains_key(&id) { None }
                            else {
                                wire.get("hello").and_then(Value::as_str).and_then(|hello| Cipher::accept(key, hello).ok()).map(|(cipher, hello)| {
                                    sessions.insert(id.clone(), cipher); hello
                                })
                            }
                        };
                        let response = match accepted { Some(hello) => json!({"type":"tunnel_hello", "id":id, "hello":hello}), None => json!({"type":"tunnel_error", "id":id}) };
                        send(&output, response).await?; continue;
                    },
                    Some("tunnel_close") => {
                        ciphers.lock().map_err(|_| "Tunnel lock failed")?.remove(&id);
                        uploads.remove(&id); downloads.remove(&id); if let Some((cancel, _)) = streams.remove(&id) { cancel.cancel(); }
                        continue;
                    },
                    Some("tunnel_data") => {
                        let opened = {
                            let mut sessions = ciphers.lock().map_err(|_| "Tunnel lock failed")?;
                            let Some(cipher) = sessions.get_mut(&id) else { continue; };
                            serde_json::from_value::<Packet>(wire).map_err(|_| "Invalid encrypted packet".to_string()).and_then(|packet| cipher.open(&packet))
                        };
                        match opened {
                            Ok(Some(mut value)) => {
                                if let Some(object) = value.as_object_mut() {
                                    // Only the authenticated outer connection selects a stream.
                                    object.insert("id".into(), Value::String(id.clone()));
                                    serde_json::from_value::<Frame>(value).ok()
                                } else { None }
                            },
                            Ok(None) => continue,
                            Err(_) => {
                                ciphers.lock().map_err(|_| "Tunnel lock failed")?.remove(&id);
                                uploads.remove(&id); downloads.remove(&id); if let Some((cancel, _)) = streams.remove(&id) { cancel.cancel(); }
                                send(&output, json!({"type":"tunnel_error", "id":id})).await?; continue;
                            }
                        }
                    },
                    _ => None,
                };
                let frame = match frame {
                    Some(Frame::Challenge { .. } | Frame::Ready { .. }) | None => {
                        ciphers.lock().map_err(|_| "Tunnel lock failed")?.remove(&id);
                        uploads.remove(&id); downloads.remove(&id); if let Some((cancel, _)) = streams.remove(&id) { cancel.cancel(); }
                        send(&output, json!({"type":"tunnel_error", "id":id})).await?; continue;
                    },
                    Some(frame) => frame,
                };
                if matches!(&frame, Frame::HttpOpen { .. } | Frame::Http { .. } | Frame::WsOpen { .. }) && streams.contains_key(&id) {
                    // A client may only open one operation on its tunnel. Its
                    // protocol violation must not disconnect other clients.
                    ciphers.lock().map_err(|_| "Tunnel lock failed")?.remove(&id);
                    uploads.remove(&id); downloads.remove(&id); if let Some((cancel, _)) = streams.remove(&id) { cancel.cancel(); }
                    send(&output, json!({"type":"tunnel_error", "id":id})).await?; continue;
                }
                match frame {
                    Frame::HttpOpen { id, method, path, headers } => {
                        if streams.len() >= MAX_STREAMS || !valid_path(&path) || !matches!(method.as_str(), "GET" | "HEAD" | "POST" | "PUT" | "PATCH" | "DELETE" | "OPTIONS") {
                            let _ = send(&output, json!({"type":"error", "id":id})).await; continue;
                        }
                        let cancel = session_stop.child_token();
                        let (input, receiver) = mpsc::channel::<Vec<u8>>(TRANSFER_WINDOW);
                        uploads.insert(id.clone(), (input, 0)); streams.insert(id.clone(), (cancel.clone(), None));
                        let (acknowledge, acknowledgements) = mpsc::channel(TRANSFER_WINDOW);
                        downloads.insert(id.clone(), acknowledge);
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
                                    send(&out, json!({"type":"request_ready", "id":id, "window":TRANSFER_WINDOW})).await?;
                                    proxy_http_body(&client, &local, &id, RequestHead { method, path, headers }, reqwest::Body::wrap_stream(body), &out, acknowledgements).await
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
                    Frame::ResponseAck { id } => {
                        if let Some(input) = downloads.get(&id) {
                            if input.try_send(()).is_err() {
                                if let Some((cancel, _)) = streams.get(&id) { cancel.cancel(); }
                            }
                        }
                    },
                    Frame::Http { id, method, path, headers, body } => {
                        if streams.len() >= MAX_STREAMS || !valid_path(&path) { let _ = send(&output, json!({"type":"error", "id":id})).await; continue; }
                        let cancel = session_stop.child_token(); streams.insert(id.clone(), (cancel.clone(), None));
                        let (acknowledge, acknowledgements) = mpsc::channel(TRANSFER_WINDOW);
                        downloads.insert(id.clone(), acknowledge);
                        let out = output.clone(); let client = client.clone(); let local = local.to_owned(); let done = finished.clone();
                        tokio::spawn(async move {
                            let result = tokio::select! { _ = cancel.cancelled() => Ok(()), result = proxy_http(&client, &local, &id, RequestHead { method, path, headers }, body, &out, acknowledgements) => result };
                            if result.is_err() { let _ = send(&out, json!({"type":"error", "id":id})).await; }
                            let _ = done.send(id);
                        });
                    },
                    Frame::WsOpen { id, path, headers } => {
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
                    Frame::WsClose { id } | Frame::Cancel { id } => { uploads.remove(&id); downloads.remove(&id); if let Some((cancel, _)) = streams.remove(&id) { cancel.cancel(); } },
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
    Err(disconnect)
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
    acknowledgements: mpsc::Receiver<()>,
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
    proxy_http_body(client, local, id, head, bytes.into(), out, acknowledgements).await
}

async fn proxy_http_body(
    client: &reqwest::Client,
    local: &str,
    id: &str,
    head: RequestHead,
    body: reqwest::Body,
    out: &mpsc::Sender<Value>,
    mut acknowledgements: mpsc::Receiver<()>,
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
        json!({"type":"response", "id":id, "status":response.status().as_u16(), "headers":headers, "window":TRANSFER_WINDOW}),
    )
    .await?;
    let mut body = response.bytes_stream();
    let mut pending = 0usize;
    while let Some(bytes) = body.next().await {
        let bytes = bytes.map_err(|e| e.to_string())?;
        for chunk in bytes.chunks(64 * 1024) {
            if pending == TRANSFER_WINDOW {
                tokio::time::timeout(Duration::from_secs(30), acknowledgements.recv())
                    .await
                    .map_err(|_| "Download recipient stopped acknowledging")?
                    .ok_or("Download recipient disconnected")?;
                pending -= 1;
            }
            while acknowledgements.try_recv().is_ok() {
                pending = pending
                    .checked_sub(1)
                    .ok_or("Unexpected download acknowledgement")?;
            }
            send(
                out,
                json!({"type":"data", "id":id, "data":STANDARD.encode(chunk)}),
            )
            .await?;
            pending += 1;
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
                } else if let Message::Close(frame) = message {
                    if let Some(frame) = frame {
                        send(out, json!({"type":"ws_close", "id":id, "code":u16::from(frame.code), "reason":frame.reason.as_str()})).await?;
                    }
                    break;
                }
            }
        }
    }
    Ok(())
}

/// Exercise the real public path using the same pinned endpoint authentication
/// as a remote app. A route registration alone is not successful provisioning.
pub(crate) async fn verify_public_summary(public_url: &str) -> Result<Value, String> {
    tokio::time::timeout(Duration::from_secs(20), async {
        let mut url = url::Url::parse(public_url).map_err(|_| "Invalid public address")?;
        let host = url.path().strip_prefix("/host/").filter(|host| host.len() == 64).ok_or("Invalid hosted identity")?.to_string();
        let scheme = if url.scheme() == "https" { "wss" } else if url.scheme() == "http" && url.host_str() == Some("127.0.0.1") { "ws" } else { return Err("Invalid public address".into()); };
        url.set_scheme(scheme).map_err(|_| "Invalid public address")?;
        url.set_path(&format!("/host/{host}/__tunnel"));
        let (mut socket, _) = connect_async(url.as_str()).await.map_err(|_| "Public encrypted connection failed")?;
        let hello = ClientHandshake::new();
        socket.send(Message::Text(json!({"type":"tunnel_open", "hello":hello.hello()}).to_string().into())).await.map_err(|_| "Public connection closed")?;
        let welcome = socket.next().await.ok_or("Public connection closed")?.map_err(|_| "Public connection failed")?;
        let welcome: Value = serde_json::from_slice(&welcome.into_data()).map_err(|_| "Invalid public handshake")?;
        if welcome.get("type").and_then(Value::as_str) != Some("tunnel_hello") { return Err("Invalid public handshake".into()); }
        let mut cipher = hello.finish(&host, welcome.get("hello").and_then(Value::as_str).ok_or("Invalid public handshake")?)?;
        let request = json!({"type":"http", "method":"GET", "path":"/api/public/server/summary", "headers":{}, "body":""});
        for packet in cipher.seal(&request)? {
            socket.send(Message::Text(json!({"type":"tunnel_data", "sequence":packet.sequence, "data":packet.data}).to_string().into())).await.map_err(|_| "Public connection closed")?;
        }
        let mut status = 0; let mut body = Vec::new();
        while let Some(message) = socket.next().await {
            let message = message.map_err(|_| "Public connection failed")?;
            if !message.is_text() { continue; }
            let packet: Packet = serde_json::from_slice(&message.into_data()).map_err(|_| "Invalid public response")?;
            if let Some(value) = cipher.open(&packet)? {
                match value.get("type").and_then(Value::as_str) {
                    Some("response") => { status = value.get("status").and_then(Value::as_u64).ok_or("Invalid public status")?; },
                    Some("data") => {
                        let data = STANDARD.decode(value.get("data").and_then(Value::as_str).ok_or("Invalid response body")?).map_err(|_| "Invalid response body")?;
                        if body.len() + data.len() > 1024 * 1024 { return Err("Public summary exceeds its limit".into()); }
                        body.extend(data);
                    },
                    Some("end") if status == 200 => {
                        let result = serde_json::from_slice(&body).map_err(|_| "Invalid public summary".into());
                        let _ = socket.close(None).await; return result;
                    },
                    _ => return Err("Public route did not return its server summary".into()),
                }
            }
        }
        Err("Public verification connection ended".into())
    }).await.map_err(|_| "Public encrypted route verification timed out".to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Real Rust client/host sockets through the Node opaque relay fixture.
    /// The fixture's broker is a test double; Vercel services have a separate
    /// deployed acceptance. No claim about the installed GUI is made here.
    #[tokio::test]
    #[ignore = "requires the opaque-native-relay.mjs loopback fixture"]
    async fn real_relay_routes_native_http_websockets_and_reconnects() {
        let _ = tracing_subscriber::fmt().with_env_filter("warn").try_init();
        use annex_federation::public_host::{request, Connection, HostedRoute};
        use axum::{
            body::Bytes,
            extract::WebSocketUpgrade,
            http::HeaderMap,
            routing::{get, post},
            Json, Router,
        };
        async fn echo(headers: HeaderMap, body: Bytes) -> Json<Value> {
            Json(
                json!({"slug":"native-target", "body":String::from_utf8_lossy(&body), "authorization":headers.get("authorization").and_then(|v| v.to_str().ok())}),
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
        let visitor = std::env::var("ANNEX_TEST_HOST_VISITOR_URL").expect("visitor URL required");
        if std::env::var("ANNEX_TEST_DEPLOYED_ROUTER").as_deref() != Ok("1") {
            assert_ne!(
                gateway, visitor,
                "use separate fixture host and visitor listeners"
            );
        } else {
            assert_eq!(gateway, "https://www.monolithannex.com");
            assert_eq!(visitor, gateway);
        }
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let local = format!("http://{}", listener.local_addr().unwrap());
        let transfer_mib = std::env::var("ANNEX_TEST_TRANSFER_MIB")
            .ok()
            .map(|value| value.parse::<usize>().unwrap())
            .unwrap_or(5);
        assert!((1..=50).contains(&transfer_mib));
        let transfer_bytes = transfer_mib * 1024 * 1024;
        let app = Router::new()
            .route("/api/test", post(echo))
            .route(
                "/api/public/server/summary",
                get(|| async { Json(json!({"slug":"native-target"})) }),
            )
            .route(
                "/api/upload",
                post(upload).layer(axum::extract::DefaultBodyLimit::max(51 * 1024 * 1024)),
            )
            .route(
                "/api/download",
                get(move || async move { vec![0x6b; transfer_bytes] }),
            )
            .route("/ws", get(echo_socket));
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
        let route = |endpoint: &str| {
            let url = format!("{visitor}{path}{endpoint}");
            HostedRoute::parse(&url)
                .or_else(|| HostedRoute::loopback_test(&url))
                .unwrap()
        };
        let headers = || [("authorization".into(), "Bearer native-test-only".into())].into();
        eprintln!("native route registered; testing authenticated HTTP");
        let response = request(
            &route("/api/test"),
            "POST",
            headers(),
            b"actual Rust connector",
            1024 * 1024,
        )
        .await
        .unwrap();
        assert_eq!(response.status, 200);
        assert_eq!(
            serde_json::from_slice::<Value>(&response.body).unwrap(),
            json!({"slug":"native-target", "body":"actual Rust connector", "authorization":"Bearer native-test-only"})
        );
        assert_eq!(
            verify_public_summary(&transport.public_url).await.unwrap()["slug"],
            "native-target"
        );
        if gateway.starts_with("https:") {
            let probe = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../scripts/native-acceptance/public-host-summary.mjs");
            let output = tokio::process::Command::new("node")
                .arg(probe)
                .arg(&transport.public_url)
                .output()
                .await
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert_eq!(
                serde_json::from_slice::<Value>(&output.stdout).unwrap()["slug"],
                "native-target"
            );
            eprintln!("independent OpenSSL probe verified the pinned native host");
        }
        assert_eq!(
            reqwest::get(format!("{visitor}{path}/api/test"))
                .await
                .unwrap()
                .status(),
            426
        );

        let mut socket = Connection::connect(&route("/ws")).await.unwrap();
        socket
            .send(&json!({"type":"ws_open", "path":"/ws", "headers":{}}))
            .await
            .unwrap();
        assert_eq!(socket.receive().await.unwrap()["type"], "ws_ready");
        socket.send(&json!({"type":"ws_data", "data":STANDARD.encode("independent encrypted recipient"), "binary":false})).await.unwrap();
        let echoed = socket.receive().await.unwrap();
        assert_eq!(echoed["type"], "ws_data");
        assert_eq!(
            STANDARD.decode(echoed["data"].as_str().unwrap()).unwrap(),
            b"independent encrypted recipient"
        );

        // A hostile client opening twice must lose only its own tunnel.
        socket
            .send(&json!({"type":"ws_open", "path":"/ws", "headers":{}}))
            .await
            .unwrap();
        assert!(
            tokio::time::timeout(Duration::from_secs(5), socket.receive())
                .await
                .unwrap()
                .is_err()
        );
        assert!(*transport.connected.borrow());
        request(
            &route("/api/test"),
            "POST",
            headers(),
            b"other client survives",
            1024 * 1024,
        )
        .await
        .unwrap();

        let file = vec![0x5a; transfer_bytes];
        let upload_started = std::time::Instant::now();
        eprintln!("starting encrypted {transfer_mib} MiB upload");
        let response = request(&route("/api/upload"), "POST", headers(), &file, 1024 * 1024)
            .await
            .unwrap();
        assert_eq!(response.status, 200);
        eprintln!(
            "encrypted upload completed in {:?}",
            upload_started.elapsed()
        );
        assert_eq!(
            serde_json::from_slice::<Value>(&response.body).unwrap(),
            json!({"bytes":file.len(), "sha256":hex::encode(Sha256::digest(&file)), "authorization":"Bearer native-test-only"})
        );
        let download_started = std::time::Instant::now();
        eprintln!("starting encrypted {transfer_mib} MiB download");
        let response = request(
            &route("/api/download"),
            "GET",
            headers(),
            &[],
            transfer_bytes,
        )
        .await
        .unwrap();
        assert_eq!(response.status, 200);
        assert_eq!(response.body.len(), transfer_bytes);
        assert!(response.body.iter().all(|byte| *byte == 0x6b));
        eprintln!(
            "encrypted download completed in {:?}",
            download_started.elapsed()
        );
        // Two heartbeat periods with no traffic must keep the host connected.
        tokio::time::sleep(Duration::from_secs(25)).await;
        assert!(*transport.connected.borrow());
        request(
            &route("/api/test"),
            "POST",
            headers(),
            b"after heartbeat",
            1024 * 1024,
        )
        .await
        .unwrap();
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
        request(
            &route("/api/test"),
            "POST",
            headers(),
            b"after restart",
            1024 * 1024,
        )
        .await
        .unwrap();
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
