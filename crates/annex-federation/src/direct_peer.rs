//! Authenticated invitation introduction followed by direct WebRTC data channels.
//! The introducer sees one key exchange and one sealed SDP offer/answer. It never
//! carries application frames, and its lifetime does not own established peers.

use crate::host_tunnel_crypto::{ClientHandshake, Packet};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    future::Future,
    pin::Pin,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, LazyLock,
    },
    time::Duration,
};
use tokio::sync::{mpsc, Mutex, Notify, OnceCell};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tokio_util::sync::CancellationToken;
use webrtc::{
    api::APIBuilder,
    data_channel::{data_channel_state::RTCDataChannelState, RTCDataChannel},
    ice_transport::ice_server::RTCIceServer,
    peer_connection::{
        configuration::RTCConfiguration, peer_connection_state::RTCPeerConnectionState,
        sdp::session_description::RTCSessionDescription, RTCPeerConnection,
    },
};

pub const PROTOCOL: &str = "annex-direct-v3";
const CHANNEL: &str = "annex-application-v3";
const CONTROL: &str = "annex-peer-v3";
const MAX_PACKET: usize = 64 * 1024;
const TIMEOUT: Duration = Duration::from_secs(25);
type PeerCell = Arc<OnceCell<Result<Arc<Peer>, String>>>;
static PEERS: LazyLock<Mutex<HashMap<String, PeerCell>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
pub type ChannelHandler =
    Arc<dyn Fn(DirectChannel) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;

#[derive(Clone)]
pub struct DirectSender {
    output: mpsc::Sender<Value>,
    stopped: CancellationToken,
}
impl DirectSender {
    pub async fn send(&self, value: Value) -> Result<(), String> {
        tokio::select! {
            _ = self.stopped.cancelled() => Err("Direct peer channel closed".into()),
            result = tokio::time::timeout(Duration::from_secs(10), self.output.send(value)) =>
                result.map_err(|_| "Direct peer send timed out")?.map_err(|_| "Direct peer channel closed".into()),
        }
    }
}

pub struct DirectChannel {
    sender: DirectSender,
    input: mpsc::Receiver<Value>,
    stopped: CancellationToken,
}
impl DirectChannel {
    fn attach(dc: Arc<RTCDataChannel>) -> Self {
        let (input, receiver) = mpsc::channel(128);
        let (output, mut outgoing) = mpsc::channel::<Value>(128);
        let stopped = CancellationToken::new();
        let opened = Arc::new(Notify::new());
        let notify = opened.clone();
        dc.on_open(Box::new(move || {
            Box::pin(async move {
                notify.notify_one();
            })
        }));
        let closed = stopped.clone();
        dc.on_close(Box::new(move || {
            let closed = closed.clone();
            Box::pin(async move {
                closed.cancel();
            })
        }));
        let failed = stopped.clone();
        dc.on_message(Box::new(move |message| {
            let input = input.clone();
            let failed = failed.clone();
            Box::pin(async move {
                if !message.is_string || message.data.len() > MAX_PACKET {
                    failed.cancel();
                    return;
                }
                match serde_json::from_slice::<Value>(&message.data) {
                    Ok(value) => {
                        if input.try_send(value).is_err() {
                            failed.cancel();
                        }
                    }
                    Err(_) => failed.cancel(),
                }
            })
        }));
        let writer_stop = stopped.clone();
        tokio::spawn(async move {
            let result = async {
                if dc.ready_state() != RTCDataChannelState::Open {
                    tokio::select! { _ = writer_stop.cancelled() => return Err(()),
                        result = tokio::time::timeout(TIMEOUT, opened.notified()) => { result.map_err(|_| ())?; } }
                }
                loop {
                    let value = tokio::select! { _ = writer_stop.cancelled() => break,
                        value = outgoing.recv() => match value { Some(value) => value, None => break } };
                    let text = value.to_string();
                    if text.len() > MAX_PACKET { return Err(()); }
                    tokio::time::timeout(Duration::from_secs(10), async {
                        while dc.buffered_amount().await > 1024 * 1024 {
                            tokio::select! { _ = writer_stop.cancelled() => return Err(()),
                                _ = tokio::time::sleep(Duration::from_millis(5)) => {} }
                        }
                        dc.send_text(text).await.map_err(|_| ())?;
                        Ok(())
                    }).await.map_err(|_| ())??;
                }
                Ok::<(), ()>(())
            }.await;
            let _ = result;
            writer_stop.cancel();
            let _ = dc.close().await;
        });
        Self {
            sender: DirectSender {
                output,
                stopped: stopped.clone(),
            },
            input: receiver,
            stopped,
        }
    }
    pub fn sender(&self) -> DirectSender {
        self.sender.clone()
    }
    pub async fn send(&self, value: Value) -> Result<(), String> {
        self.sender.send(value).await
    }
    pub async fn receive(&mut self) -> Result<Value, String> {
        tokio::select! { biased;
            value = self.input.recv() => value.ok_or_else(|| "Direct peer channel ended".into()),
            _ = self.stopped.cancelled() => Err("Direct peer channel closed".into()),
        }
    }
    pub fn close(&self) {
        self.stopped.cancel();
    }
}
impl Drop for DirectChannel {
    fn drop(&mut self) {
        self.stopped.cancel();
    }
}

pub struct Peer {
    pc: Arc<RTCPeerConnection>,
    created: tokio::time::Instant,
}
impl Peer {
    pub fn alive(&self) -> bool {
        match self.pc.connection_state() {
            RTCPeerConnectionState::Connected | RTCPeerConnectionState::Disconnected => true,
            RTCPeerConnectionState::New | RTCPeerConnectionState::Connecting => {
                self.created.elapsed() < TIMEOUT
            }
            _ => false,
        }
    }
    pub async fn close(&self) {
        let _ = self.pc.close().await;
    }
    pub async fn channel(&self) -> Result<DirectChannel, String> {
        if !self.alive() {
            return Err("Direct peer disconnected".into());
        }
        let dc = self
            .pc
            .create_data_channel(CHANNEL, None)
            .await
            .map_err(|e| e.to_string())?;
        Ok(DirectChannel::attach(dc))
    }
}
impl Drop for Peer {
    fn drop(&mut self) {
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            let pc = self.pc.clone();
            runtime.spawn(async move {
                let _ = pc.close().await;
            });
        }
    }
}

async fn new_peer() -> Result<Arc<Peer>, String> {
    // STUN discovers addresses; no TURN or other application relay is configured.
    let pc = APIBuilder::new()
        .build()
        .new_peer_connection(RTCConfiguration {
            ice_servers: vec![RTCIceServer {
                urls: vec!["stun:stun.l.google.com:19302".into()],
                ..Default::default()
            }],
            ..Default::default()
        })
        .await
        .map_err(|e| e.to_string())?;
    let pc = Arc::new(pc);
    let weak = Arc::downgrade(&pc);
    let revision = Arc::new(AtomicU64::new(0));
    pc.on_peer_connection_state_change(Box::new(move |state| {
        let weak = weak.clone();
        let revision = revision.clone();
        let current = revision.fetch_add(1, Ordering::SeqCst) + 1;
        Box::pin(async move {
            if state == RTCPeerConnectionState::Disconnected {
                tokio::spawn(async move {
                    tokio::time::sleep(TIMEOUT).await;
                    if revision.load(Ordering::SeqCst) == current {
                        if let Some(pc) = weak.upgrade() {
                            let _ = pc.close().await;
                        }
                    }
                });
            }
        })
    }));
    let weak = Arc::downgrade(&pc);
    tokio::spawn(async move {
        tokio::time::sleep(TIMEOUT).await;
        if let Some(pc) = weak.upgrade() {
            if matches!(
                pc.connection_state(),
                RTCPeerConnectionState::New | RTCPeerConnectionState::Connecting
            ) {
                let _ = pc.close().await;
            }
        }
    });
    Ok(Arc::new(Peer {
        pc,
        created: tokio::time::Instant::now(),
    }))
}

async fn local_description(peer: &Peer, offer: bool) -> Result<Value, String> {
    let description = if offer {
        peer.pc.create_offer(None).await
    } else {
        peer.pc.create_answer(None).await
    }
    .map_err(|e| e.to_string())?;
    let mut gathered = peer.pc.gathering_complete_promise().await;
    peer.pc
        .set_local_description(description)
        .await
        .map_err(|e| e.to_string())?;
    tokio::time::timeout(Duration::from_secs(10), gathered.recv())
        .await
        .map_err(|_| "Direct ICE gathering timed out")?;
    let description = peer
        .pc
        .local_description()
        .await
        .ok_or("Missing direct peer description")?;
    if description.sdp.len() > 24 * 1024 {
        return Err("Direct peer description exceeds its limit".into());
    }
    Ok(
        json!({ "protocol": PROTOCOL, "type": if offer { "offer" } else { "answer" }, "sdp": description.sdp }),
    )
}

fn remote_description(value: Value, offer: bool) -> Result<RTCSessionDescription, String> {
    let kind = if offer { "offer" } else { "answer" };
    if value["protocol"] != PROTOCOL
        || value["type"] != kind
        || value.as_object().is_none_or(|o| o.len() != 3)
    {
        return Err("Only a direct peer description is permitted".into());
    }
    let sdp = value["sdp"]
        .as_str()
        .filter(|s| s.len() <= 24 * 1024 && s.starts_with("v=0\r\n"))
        .ok_or("Invalid direct peer description")?;
    // A malicious introducer cannot silently replace direct service with TURN.
    if sdp.lines().any(|line| {
        line.starts_with("a=candidate:")
            && line
                .split_whitespace()
                .collect::<Vec<_>>()
                .windows(2)
                .any(|w| w == ["typ", "relay"])
    }) {
        return Err("Relayed peer candidates are not permitted".into());
    }
    if offer {
        RTCSessionDescription::offer(sdp.into())
    } else {
        RTCSessionDescription::answer(sdp.into())
    }
    .map_err(|e| e.to_string())
}

/// Answer one authenticated introduction. The returned peer belongs to the
/// host's peer registry, never to the introducer socket or registration lease.
pub async fn answer(offer: Value, handler: ChannelHandler) -> Result<(Arc<Peer>, Value), String> {
    let peer = new_peer().await?;
    let weak = Arc::downgrade(&peer.pc);
    peer.pc.on_data_channel(Box::new(move |dc| {
        let handler = handler.clone();
        let weak = weak.clone();
        Box::pin(async move {
            if dc.label() == CONTROL {
                return;
            }
            if dc.label() != CHANNEL {
                let _ = dc.close().await;
                return;
            }
            if weak.upgrade().is_none() {
                let _ = dc.close().await;
                return;
            }
            let channel = DirectChannel::attach(dc);
            tokio::spawn(async move {
                handler(channel).await;
            });
        })
    }));
    peer.pc
        .set_remote_description(remote_description(offer, true)?)
        .await
        .map_err(|e| e.to_string())?;
    let description = local_description(&peer, false).await?;
    Ok((peer, description))
}

/// Reuse the connection after the introduction; subsequent channels do not
/// contact Monolith. A new introduction is needed only after peer failure.
pub async fn connect(socket_url: &str, host: &str) -> Result<DirectChannel, String> {
    let key = format!("{host}|{socket_url}");
    let cell = {
        let mut peers = PEERS.lock().await;
        peers.retain(|_, cell| {
            cell.get()
                .is_none_or(|result| result.as_ref().is_ok_and(|peer| peer.alive()))
        });
        if peers.len() >= 64 && !peers.contains_key(&key) {
            return Err("Too many direct peers".into());
        }
        peers.entry(key.clone()).or_default().clone()
    };
    match cell.get_or_init(|| introduce(socket_url, host)).await {
        Ok(peer) => peer.channel().await,
        Err(error) => {
            let mut peers = PEERS.lock().await;
            if peers
                .get(&key)
                .is_some_and(|current| Arc::ptr_eq(current, &cell))
            {
                peers.remove(&key);
            }
            Err(error.clone())
        }
    }
}

async fn introduce(socket_url: &str, host: &str) -> Result<Arc<Peer>, String> {
    tokio::time::timeout(TIMEOUT, async {
        let (mut socket, _) = connect_async(socket_url).await.map_err(|e| e.to_string())?;
        let hello = ClientHandshake::new();
        socket.send(Message::Text(json!({ "type": "tunnel_open", "hello": hello.hello() }).to_string().into())).await.map_err(|e| e.to_string())?;
        let welcome = next_signal(&mut socket).await?;
        if welcome["type"] != "tunnel_hello" { return Err("Host rejected introduction".into()); }
        let mut cipher = hello.finish(host, welcome["hello"].as_str().ok_or("Missing host authentication")?)?;
        let peer = new_peer().await?;
        let control = peer.pc.create_data_channel(CONTROL, None).await.map_err(|e| e.to_string())?;
        let opened = Arc::new(Notify::new()); let notify = opened.clone();
        control.on_open(Box::new(move || Box::pin(async move { notify.notify_one(); })));
        let offer = local_description(&peer, true).await?;
        let packets = cipher.seal(&offer)?;
        if packets.len() != 1 { return Err("Direct introduction exceeds its limit".into()); }
        socket.send(Message::Text(json!({ "type": "peer_offer", "sequence": packets[0].sequence, "data": packets[0].data }).to_string().into())).await.map_err(|e| e.to_string())?;
        let answer = next_signal(&mut socket).await?;
        if answer["type"] != "peer_answer" { return Err("Host did not answer introduction".into()); }
        let packet: Packet = serde_json::from_value(answer).map_err(|_| "Invalid introduction")?;
        let description = cipher.open(&packet)?.ok_or("Incomplete introduction")?;
        peer.pc.set_remote_description(remote_description(description, false)?).await.map_err(|e| e.to_string())?;
        // Close the rendezvous before any application operation can begin.
        let _ = socket.close(None).await;
        if control.ready_state() != RTCDataChannelState::Open { opened.notified().await; }
        Ok(peer)
    }).await.map_err(|_| "Could not establish a direct peer connection. Check the peers' network reachability; application relaying is disabled.".to_string())?
}

async fn next_signal<S>(socket: &mut tokio_tungstenite::WebSocketStream<S>) -> Result<Value, String>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    while let Some(message) = socket.next().await {
        let message = message.map_err(|e| e.to_string())?;
        match message {
            Message::Text(text) if text.len() <= MAX_PACKET => {
                return serde_json::from_str(&text).map_err(|_| "Invalid introduction".into())
            }
            Message::Ping(_) | Message::Pong(_) => continue,
            _ => return Err("Introduction connection ended".into()),
        }
    }
    Err("Introduction connection ended".into())
}
