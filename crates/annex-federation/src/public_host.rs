//! Endpoint-authenticated connections through the content-blind public router.
//! No request path, credential or application payload enters the outer socket.
use crate::host_tunnel_crypto::{Cipher, ClientHandshake, Packet};
use base64::{engine::general_purpose::STANDARD, Engine};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::{collections::HashMap, time::Duration};
use tokio_tungstenite::{connect_async, tungstenite::Message, MaybeTlsStream, WebSocketStream};

pub struct HostedRoute {
    pub socket: String,
    pub host: String,
    pub path: String,
}
impl HostedRoute {
    pub fn parse(value: &str) -> Option<Self> {
        let url = url::Url::parse(value).ok()?;
        if url.scheme() != "https"
            || url.port().is_some()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.fragment().is_some()
            || !matches!(
                url.host_str(),
                Some("monolithannex.com" | "www.monolithannex.com")
            )
        {
            return None;
        }
        Self::from_url(url)
    }
    fn from_url(mut url: url::Url) -> Option<Self> {
        let rest = url.path().strip_prefix("/host/")?;
        let (host, path) = rest.split_once('/')?;
        if host.len() != 64
            || !host
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return None;
        }
        let host = host.to_owned();
        let path = format!(
            "/{path}{}",
            url.query().map(|q| format!("?{q}")).unwrap_or_default()
        );
        url.set_path(&format!("/host/{host}/__tunnel"));
        url.set_query(None);
        url.set_fragment(None);
        url.set_scheme(if url.scheme() == "http" { "ws" } else { "wss" })
            .ok()?;
        Some(Self {
            socket: url.into(),
            host,
            path,
        })
    }
    /// Explicit loopback-only fixture entry point; never selected by production
    /// URL recognition or as a fallback after an encrypted connection failure.
    pub fn loopback_test(value: &str) -> Option<Self> {
        let url = url::Url::parse(value).ok()?;
        if url.scheme() != "http"
            || url.host_str() != Some("127.0.0.1")
            || !url.username().is_empty()
            || url.password().is_some()
        {
            return None;
        }
        Self::from_url(url)
    }
}

pub struct Connection {
    socket: WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>,
    cipher: Cipher,
}
impl Connection {
    pub async fn connect(route: &HostedRoute) -> Result<Self, String> {
        tokio::time::timeout(Duration::from_secs(20), async {
            let (mut socket, _) = connect_async(&route.socket)
                .await
                .map_err(|e| e.to_string())?;
            let handshake = ClientHandshake::new();
            socket
                .send(Message::Text(
                    json!({"type":"tunnel_open", "hello":handshake.hello()})
                        .to_string()
                        .into(),
                ))
                .await
                .map_err(|e| e.to_string())?;
            let welcome = next_wire(&mut socket).await?;
            if welcome["type"] != "tunnel_hello" {
                return Err("Host rejected encrypted connection".into());
            }
            let cipher = handshake.finish(
                &route.host,
                welcome["hello"]
                    .as_str()
                    .ok_or("Missing host authentication")?,
            )?;
            Ok(Self { socket, cipher })
        })
        .await
        .map_err(|_| "Encrypted connection timed out".to_string())?
    }
    pub async fn send(&mut self, value: &Value) -> Result<(), String> {
        for packet in self.cipher.seal(value)? {
            self.socket
                .send(Message::Text(
                    json!({"type":"tunnel_data", "sequence":packet.sequence, "data":packet.data})
                        .to_string()
                        .into(),
                ))
                .await
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }
    pub async fn receive(&mut self) -> Result<Value, String> {
        loop {
            let wire = next_wire(&mut self.socket).await?;
            if wire["type"] != "tunnel_data" {
                return Err("Encrypted connection interrupted".into());
            }
            let packet: Packet =
                serde_json::from_value(wire).map_err(|_| "Invalid encrypted packet")?;
            if let Some(value) = self.cipher.open(&packet)? {
                return Ok(value);
            }
        }
    }
    pub async fn close(&mut self) {
        let _ = self.socket.close(None).await;
    }
}
async fn next_wire(
    socket: &mut WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>,
) -> Result<Value, String> {
    while let Some(message) = socket.next().await {
        let message = message.map_err(|e| e.to_string())?;
        if let Message::Close(frame) = message {
            return Err(match frame {
                Some(frame) => format!(
                    "Encrypted connection closed {}: {}",
                    frame.code, frame.reason
                ),
                None => "Encrypted connection closed".into(),
            });
        }
        if !message.is_text() {
            continue;
        }
        if message.len() > 128 * 1024 {
            return Err("Encrypted packet exceeds its limit".into());
        }
        return serde_json::from_slice(&message.into_data())
            .map_err(|_| "Invalid encrypted packet".into());
    }
    Err("Encrypted connection ended".into())
}

pub struct Response {
    pub status: u16,
    pub body: Vec<u8>,
}
/// Streaming requests preserve the existing federation size bounds. An
/// interrupted mutation is never retried here or downgraded to readable HTTP.
pub async fn request(
    route: &HostedRoute,
    method: &str,
    headers: HashMap<String, String>,
    body: &[u8],
    max_response: usize,
) -> Result<Response, String> {
    if body.len() > 50 * 1024 * 1024 {
        return Err("Hosted request exceeds its limit".into());
    }
    tokio::time::timeout(Duration::from_secs(120), async {
        let mut connection = Connection::connect(route).await?;
        connection
            .send(
                &json!({"type":"http_open", "method":method, "path":route.path, "headers":headers}),
            )
            .await?;
        let mut chunks = body.chunks(64 * 1024);
        let mut ready = false;
        let mut ended = false;
        let mut in_flight = 0usize;
        let mut window = 1usize;
        let mut status = 0;
        let mut bytes = Vec::new();
        let result = async {
            loop {
                let frame = connection.receive().await?;
                match frame["type"].as_str() {
                    Some("request_ready") if !ready => {
                        ready = true;
                        window = frame["window"]
                            .as_u64()
                            .filter(|v| (1..=32).contains(v))
                            .unwrap_or(1) as usize;
                    }
                    Some("request_ack") if ready && in_flight > 0 => {
                        in_flight -= 1;
                    }
                    Some("response") if status == 0 => {
                        status = frame["status"]
                            .as_u64()
                            .filter(|s| (200..=599).contains(s))
                            .ok_or("Invalid hosted response status")?
                            as u16;
                    }
                    Some("data") if status != 0 => {
                        let chunk = STANDARD
                            .decode(frame["data"].as_str().ok_or("Invalid response bytes")?)
                            .map_err(|_| "Invalid response bytes")?;
                        if bytes.len() + chunk.len() > max_response {
                            return Err("Hosted response exceeds its limit".into());
                        }
                        bytes.extend(chunk);
                        connection.send(&json!({"type":"response_ack"})).await?;
                    }
                    Some("end") if status != 0 => {
                        return Ok(Response {
                            status,
                            body: bytes,
                        })
                    }
                    _ => return Err("The host could not complete the request".into()),
                }
                if ready && !ended {
                    while in_flight < window {
                        if let Some(chunk) = chunks.next() {
                            connection
                                .send(&json!({"type":"http_data", "data":STANDARD.encode(chunk)}))
                                .await?;
                            in_flight += 1;
                        } else {
                            ended = true;
                            connection.send(&json!({"type":"http_end"})).await?;
                            break;
                        }
                    }
                }
            }
        }
        .await;
        connection.close().await;
        result
    })
    .await
    .map_err(|_| "Hosted request timed out; outcome may be unknown".to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn routing_hides_application_paths_and_tokens_from_the_outer_socket() {
        let base = format!("https://www.monolithannex.com/host/{}", "a".repeat(64));
        let route =
            HostedRoute::parse(&format!("{base}/api/federation/vrp-root?token=private")).unwrap();
        assert_eq!(
            route.socket,
            format!("{}/__tunnel", base.replace("https:", "wss:"))
        );
        assert_eq!(route.path, "/api/federation/vrp-root?token=private");
        for origin in [
            "https://other.example",
            "http://monolithannex.com",
            "https://monolithannex.com.evil.test",
            "https://user:password@monolithannex.com",
            "https://monolithannex.com:444",
        ] {
            assert!(
                HostedRoute::parse(&format!("{origin}/host/{}/api/test", "a".repeat(64))).is_none()
            );
        }
    }
}
