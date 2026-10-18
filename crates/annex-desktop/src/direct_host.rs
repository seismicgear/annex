//! The host's introduction listener and independently owned direct peers.
use annex_federation::{
    direct_peer::{self, ChannelHandler, Peer},
    host_tunnel_crypto::{Cipher, Packet},
};
use ed25519_dalek::{Signer, SigningKey};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::{collections::HashMap, sync::Arc, time::Duration};
use tokio::sync::{watch, Mutex, Semaphore};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tokio_util::sync::CancellationToken;

type IntroductionAnswer = (String, Result<(Arc<Peer>, Packet), String>);

async fn introduction_json<S>(
    socket: &mut tokio_tungstenite::WebSocketStream<S>,
) -> Result<Value, String>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    loop {
        match socket
            .next()
            .await
            .ok_or("Introducer disconnected")?
            .map_err(|e| e.to_string())?
        {
            Message::Ping(bytes) => socket
                .send(Message::Pong(bytes))
                .await
                .map_err(|e| e.to_string())?,
            Message::Pong(_) => {}
            Message::Close(frame) => {
                return Err(match frame {
                    Some(frame) => format!(
                        "Introducer closed the connection ({})",
                        u16::from(frame.code)
                    ),
                    None => "Introducer closed the connection".into(),
                })
            }
            Message::Text(text) if text.len() <= 64 * 1024 => {
                return serde_json::from_str(&text)
                    .map_err(|_| "Invalid introduction packet".into());
            }
            _ => return Err("Invalid introduction packet".into()),
        }
    }
}

pub(crate) async fn run(
    socket_url: String,
    local: String,
    expected: String,
    key: SigningKey,
    status: watch::Sender<bool>,
    stop: CancellationToken,
    renew_after: Duration,
) {
    let peers: Arc<Mutex<Vec<Arc<Peer>>>> = Arc::new(Mutex::new(Vec::new()));
    let channels = Arc::new(Semaphore::new(128));
    let mut backoff = 1;
    while !stop.is_cancelled() {
        peers.lock().await.retain(|peer| peer.alive());
        let host = Registration {
            local: &local,
            expected: &expected,
            key: &key,
            status: &status,
            stop: &stop,
            peers: peers.clone(),
            channels: channels.clone(),
        };
        tokio::select! {
            _ = stop.cancelled() => break,
            result = tokio::time::timeout(renew_after, host.listen(&socket_url)) => {
                if let Ok(Err(reason)) = &result { tracing::warn!(%reason, "Peer introduction interrupted; established peers remain connected"); }
                if result.is_err() { backoff = 1; }
            }
        }
        let _ = status.send(false);
        tokio::select! { _ = stop.cancelled() => break, _ = tokio::time::sleep(Duration::from_secs(backoff)) => {} }
        backoff = (backoff * 2).min(30);
    }
    let _ = status.send(false);
    for peer in peers.lock().await.drain(..) {
        peer.close().await;
    }
}

struct Registration<'a> {
    local: &'a str,
    expected: &'a str,
    key: &'a SigningKey,
    status: &'a watch::Sender<bool>,
    stop: &'a CancellationToken,
    peers: Arc<Mutex<Vec<Arc<Peer>>>>,
    channels: Arc<Semaphore>,
}
impl Registration<'_> {
    async fn listen(&self, url: &str) -> Result<(), String> {
        let (mut socket, _) = tokio::time::timeout(Duration::from_secs(10), connect_async(url))
            .await
            .map_err(|_| "Introduction connection timed out")?
            .map_err(|e| e.to_string())?;
        let challenge =
            tokio::time::timeout(Duration::from_secs(5), introduction_json(&mut socket))
                .await
                .map_err(|_| "Introduction challenge timed out")??;
        let challenge = challenge["challenge"]
            .as_str()
            .filter(|s| s.len() == 64 && s.bytes().all(|c| c.is_ascii_hexdigit()))
            .ok_or("Invalid challenge")?;
        let signature = self
            .key
            .sign(format!("annex-public-host-v1:{challenge}").as_bytes());
        socket.send(Message::Text(json!({"type":"authenticate", "publicKey":hex::encode(self.key.verifying_key().as_bytes()),
            "signature":hex::encode(signature.to_bytes())}).to_string().into())).await.map_err(|e| e.to_string())?;
        let ready = tokio::time::timeout(Duration::from_secs(10), introduction_json(&mut socket))
            .await
            .map_err(|_| "Introduction registration timed out")??;
        if ready["type"] != "ready"
            || ready["protocol"] != direct_peer::PROTOCOL
            || ready["publicUrl"] != self.expected
        {
            return Err("The gateway does not support direct peer introduction".into());
        }
        let _ = self.status.send(true);
        let mut pending: HashMap<String, (Cipher, tokio::time::Instant)> = HashMap::new();
        let mut answers: tokio::task::JoinSet<IntroductionAnswer> = tokio::task::JoinSet::new();
        loop {
            tokio::select! {
                _ = self.stop.cancelled() => break,
                answer = answers.join_next(), if !answers.is_empty() => {
                    if let Some(Ok((id, result))) = answer {
                        let frame = match result {
                            Ok((peer, packet)) => { self.peers.lock().await.push(peer); json!({"type":"peer_answer", "id":id, "sequence":packet.sequence, "data":packet.data}) },
                            Err(_) => json!({"type":"peer_error", "id":id}),
                        };
                        socket.send(Message::Text(frame.to_string().into())).await.map_err(|e| e.to_string())?;
                    }
                },
                incoming = socket.next() => {
                    let Some(message) = incoming else { break; };
                    let message = message.map_err(|e| e.to_string())?;
                    if let Message::Ping(bytes) = message { socket.send(Message::Pong(bytes)).await.map_err(|e| e.to_string())?; continue; }
                    if message.is_close() { break; }
                    if !message.is_text() || message.len() > 64 * 1024 { return Err("Invalid introduction packet".into()); }
                    let frame: Value = serde_json::from_slice(&message.into_data()).map_err(|_| "Invalid introduction packet")?;
                    let id = frame["id"].as_str().filter(|s| s.len() == 36 && uuid::Uuid::parse_str(s).is_ok()).ok_or("Invalid introduction address")?.to_string();
                    pending.retain(|_, (_, since)| since.elapsed() < Duration::from_secs(25));
                    self.peers.lock().await.retain(|peer| peer.alive());
                    match frame["type"].as_str() {
                        Some("tunnel_open") => {
                            if pending.len() + answers.len() + self.peers.lock().await.len() >= 64 || pending.contains_key(&id) {
                                socket.send(Message::Text(json!({"type":"peer_error", "id":id}).to_string().into())).await.map_err(|e| e.to_string())?; continue;
                            }
                            let accepted = frame["hello"].as_str().ok_or("Missing client handshake".to_string()).and_then(|hello| Cipher::accept(self.key, hello));
                            let Ok((cipher, hello)) = accepted else {
                                socket.send(Message::Text(json!({"type":"peer_error", "id":id}).to_string().into())).await.map_err(|e| e.to_string())?; continue;
                            };
                            pending.insert(id.clone(), (cipher, tokio::time::Instant::now()));
                            socket.send(Message::Text(json!({"type":"tunnel_hello", "id":id, "hello":hello}).to_string().into())).await.map_err(|e| e.to_string())?;
                        },
                        Some("peer_offer") => {
                            let Some((mut cipher, _)) = pending.remove(&id) else { continue; };
                            let offer = serde_json::from_value::<Packet>(frame).map_err(|_| "Invalid direct offer".to_string())
                                .and_then(|packet| cipher.open(&packet)).and_then(|value| value.ok_or("Incomplete direct offer".into()));
                            let Ok(offer) = offer else {
                                socket.send(Message::Text(json!({"type":"peer_error", "id":id}).to_string().into())).await.map_err(|e| e.to_string())?; continue;
                            };
                            let local = self.local.to_owned(); let key = self.key.clone(); let stop = self.stop.clone();
                            let channels = self.channels.clone();
                            let handler: ChannelHandler = Arc::new(move |channel| {
                                let local = local.clone(); let key = key.clone(); let stop = stop.clone(); let channels = channels.clone();
                                Box::pin(async move {
                                    let Ok(_permit) = channels.try_acquire_owned() else { channel.close(); return; };
                                    let _ = crate::public_transport::serve_direct_channel(channel, &local, &key, &stop).await;
                                })
                            });
                            answers.spawn(async move {
                                let result = async {
                                    let (peer, answer) = direct_peer::answer(offer, handler).await?;
                                    let mut packets = cipher.seal(&answer)?;
                                    if packets.len() != 1 { return Err("Direct answer exceeds its limit".to_string()); }
                                    Ok((peer, packets.remove(0)))
                                }.await;
                                (id, result)
                            });
                        },
                        Some("peer_cancel") => { pending.remove(&id); },
                        _ => return Err("Application data is forbidden on the introducer".into()),
                    }
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio_tungstenite::{
        tungstenite::protocol::{frame::coding::CloseCode, CloseFrame, Role},
        WebSocketStream,
    };

    #[tokio::test]
    async fn handshake_answers_control_frames_and_reports_service_closure() {
        let (client, server) = tokio::io::duplex(4096);
        let mut client = WebSocketStream::from_raw_socket(client, Role::Client, None).await;
        let mut server = WebSocketStream::from_raw_socket(server, Role::Server, None).await;
        server
            .send(Message::Ping(vec![1, 2, 3].into()))
            .await
            .unwrap();
        server
            .send(Message::Text(r#"{"type":"ready"}"#.into()))
            .await
            .unwrap();
        assert_eq!(
            introduction_json(&mut client).await.unwrap()["type"],
            "ready"
        );
        assert_eq!(
            server.next().await.unwrap().unwrap(),
            Message::Pong(vec![1, 2, 3].into())
        );
        server
            .send(Message::Close(Some(CloseFrame {
                code: CloseCode::Error,
                reason: "service unavailable".into(),
            })))
            .await
            .unwrap();
        assert_eq!(
            introduction_json(&mut client).await.unwrap_err(),
            "Introducer closed the connection (1011)"
        );
    }
}
