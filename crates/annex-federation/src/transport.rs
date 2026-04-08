use crate::signal::{SignalClient, SignalError, SignalingPayload};
use futures_util::Future;
use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;
use thiserror::Error;
use tokio::sync::{Mutex, RwLock};
use webrtc::api::APIBuilder;
use webrtc::data_channel::data_channel_init::RTCDataChannelInit;
use webrtc::data_channel::RTCDataChannel;
use webrtc::ice_transport::ice_server::RTCIceServer;
use webrtc::peer_connection::configuration::RTCConfiguration;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;
use webrtc::peer_connection::RTCPeerConnection;

pub type InboundHandler = Arc<
    dyn Fn(String) -> Pin<Box<dyn Future<Output = ()> + Send + 'static>> + Send + Sync + 'static,
>;

#[derive(Debug, Error)]
pub enum TransportError {
    #[error("signal error: {0}")]
    Signal(#[from] SignalError),
    #[error("webrtc error: {0}")]
    WebRtc(String),
    #[error("transport not connected for remote slug: {0}")]
    NotConnected(String),
}

#[derive(Clone)]
pub struct FederationTransport {
    local_server_slug: String,
    signal: SignalClient,
    peers: Arc<RwLock<HashMap<String, Arc<RTCPeerConnection>>>>,
    channels: Arc<RwLock<HashMap<String, Arc<RTCDataChannel>>>>,
    pending_answers: Arc<Mutex<HashMap<String, tokio::sync::oneshot::Sender<String>>>>,
    inbound_handler: InboundHandler,
}

impl FederationTransport {
    pub fn new(
        local_server_slug: impl Into<String>,
        signal: SignalClient,
        inbound_handler: InboundHandler,
    ) -> Self {
        Self {
            local_server_slug: local_server_slug.into(),
            signal,
            peers: Arc::new(RwLock::new(HashMap::new())),
            channels: Arc::new(RwLock::new(HashMap::new())),
            pending_answers: Arc::new(Mutex::new(HashMap::new())),
            inbound_handler,
        }
    }

    pub fn spawn_signal_listener(self: Arc<Self>) {
        tokio::spawn(async move {
            loop {
                match self.signal.poll_signal(&self.local_server_slug, 55).await {
                    Ok(Some(payload)) => {
                        if let Err(err) = self.handle_signal_payload(payload).await {
                            tracing::warn!(error = %err, "failed to process inbound federation signal");
                        }
                    }
                    Ok(None) => {}
                    Err(err) => {
                        tracing::warn!(error = %err, "signal long-poll failed");
                        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                    }
                }
            }
        });
    }

    pub async fn establish_peer(&self, remote_server_slug: &str) -> Result<(), TransportError> {
        let pc = self
            .build_peer_connection(remote_server_slug.to_string())
            .await?;

        let dc = pc
            .create_data_channel(
                "annex-federation",
                Some(RTCDataChannelInit {
                    ordered: Some(true),
                    ..Default::default()
                }),
            )
            .await
            .map_err(|e| TransportError::WebRtc(e.to_string()))?;
        self.bind_data_channel(remote_server_slug.to_string(), dc.clone())
            .await;

        let offer = pc
            .create_offer(None)
            .await
            .map_err(|e| TransportError::WebRtc(e.to_string()))?;
        pc.set_local_description(offer.clone())
            .await
            .map_err(|e| TransportError::WebRtc(e.to_string()))?;

        let (tx, rx) = tokio::sync::oneshot::channel::<String>();
        self.pending_answers
            .lock()
            .await
            .insert(remote_server_slug.to_string(), tx);

        self.signal
            .post_signal(&SignalingPayload {
                from_server_slug: self.local_server_slug.clone(),
                to_server_slug: remote_server_slug.to_string(),
                sdp_type: "offer".to_string(),
                sdp: offer.sdp,
            })
            .await?;

        let answer_sdp = rx
            .await
            .map_err(|_| TransportError::WebRtc("answer channel closed".to_string()))?;
        let answer = RTCSessionDescription::answer(answer_sdp)
            .map_err(|e| TransportError::WebRtc(e.to_string()))?;
        pc.set_remote_description(answer)
            .await
            .map_err(|e| TransportError::WebRtc(e.to_string()))?;

        self.peers
            .write()
            .await
            .insert(remote_server_slug.to_string(), pc);

        Ok(())
    }

    pub async fn relay_message(
        &self,
        remote_server_slug: &str,
        envelope_json: &str,
    ) -> Result<(), TransportError> {
        let channels = self.channels.read().await;
        let Some(dc) = channels.get(remote_server_slug) else {
            return Err(TransportError::NotConnected(remote_server_slug.to_string()));
        };

        dc.send_text(envelope_json)
            .await
            .map_err(|e| TransportError::WebRtc(e.to_string()))
    }

    async fn handle_signal_payload(&self, payload: SignalingPayload) -> Result<(), TransportError> {
        match payload.sdp_type.as_str() {
            "offer" => self.handle_offer(payload).await,
            "answer" => {
                if let Some(tx) = self
                    .pending_answers
                    .lock()
                    .await
                    .remove(&payload.from_server_slug)
                {
                    let _ = tx.send(payload.sdp);
                }
                Ok(())
            }
            other => Err(TransportError::WebRtc(format!(
                "unsupported signaling payload type: {other}"
            ))),
        }
    }

    async fn handle_offer(&self, payload: SignalingPayload) -> Result<(), TransportError> {
        let pc = self
            .build_peer_connection(payload.from_server_slug.clone())
            .await?;

        let offer = RTCSessionDescription::offer(payload.sdp)
            .map_err(|e| TransportError::WebRtc(e.to_string()))?;
        pc.set_remote_description(offer)
            .await
            .map_err(|e| TransportError::WebRtc(e.to_string()))?;

        let answer = pc
            .create_answer(None)
            .await
            .map_err(|e| TransportError::WebRtc(e.to_string()))?;
        pc.set_local_description(answer.clone())
            .await
            .map_err(|e| TransportError::WebRtc(e.to_string()))?;

        self.signal
            .post_signal(&SignalingPayload {
                from_server_slug: self.local_server_slug.clone(),
                to_server_slug: payload.from_server_slug.clone(),
                sdp_type: "answer".to_string(),
                sdp: answer.sdp,
            })
            .await?;

        self.peers
            .write()
            .await
            .insert(payload.from_server_slug, pc);
        Ok(())
    }

    async fn build_peer_connection(
        &self,
        remote_slug: String,
    ) -> Result<Arc<RTCPeerConnection>, TransportError> {
        let api = APIBuilder::new().build();
        let pc = Arc::new(
            api.create_peer_connection(RTCConfiguration {
                ice_servers: vec![RTCIceServer {
                    urls: vec!["stun:stun.l.google.com:19302".to_string()],
                    ..Default::default()
                }],
                ..Default::default()
            })
            .await
            .map_err(|e| TransportError::WebRtc(e.to_string()))?,
        );

        let channels = self.channels.clone();
        let handler = self.inbound_handler.clone();
        let remote_for_dc = remote_slug.clone();
        pc.on_data_channel(Box::new(move |dc: Arc<RTCDataChannel>| {
            let channels = channels.clone();
            let handler = handler.clone();
            let remote_for_dc = remote_for_dc.clone();
            Box::pin(async move {
                channels
                    .write()
                    .await
                    .insert(remote_for_dc.clone(), dc.clone());

                dc.on_message(Box::new(move |msg| {
                    let handler = handler.clone();
                    Box::pin(async move {
                        if let Ok(payload) = String::from_utf8(msg.data.to_vec()) {
                            (handler)(payload).await;
                        }
                    })
                }));
            })
        }));

        Ok(pc)
    }

    async fn bind_data_channel(&self, remote_slug: String, dc: Arc<RTCDataChannel>) {
        self.channels.write().await.insert(remote_slug, dc.clone());
        let handler = self.inbound_handler.clone();
        dc.on_message(Box::new(move |msg| {
            let handler = handler.clone();
            Box::pin(async move {
                if let Ok(payload) = String::from_utf8(msg.data.to_vec()) {
                    (handler)(payload).await;
                }
            })
        }));
    }
}
