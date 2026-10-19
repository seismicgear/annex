//! Launch-bound Array credential-wallet client. No listener, discovery or secrets
//! in argv/environment. Array owns the parent end and binds it to the installed
//! Annex executable. A second-instance launch cannot attach this pipe to the
//! already-running Tauri process.
//!
//! Only this module writes protocol frames. Requests are serialized, bounded and
//! correlated; a timeout, malformed frame or EOF permanently closes this launch's
//! channel. Stdout's lock keeps complete frames separate from ordinary logs,
//! which Array ignores unless they start with the reserved prefix.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

const PREFIX: &[u8] = b"KI_ARRAY_WALLET_V1 ";
const MAX_REQUEST_FRAME_BYTES: usize = 64 * 1024;
const MAX_RESPONSE_FRAME_BYTES: usize = 256 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const FORMAT: &str = "annex.identity.v1";
const FIELD_ORDER_HEX: &str = "30644e72e131a029b85045b68181585d2833e84879b9709143e1f593f0000001";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WalletError {
    Unavailable,
    Timeout,
    Protocol,
    Busy,
    Pairing,
    Locked,
    Denied,
    Missing,
    InvalidIdentity,
    Preference,
}

impl WalletError {
    // Never return a remote error body, IO diagnostic or submitted payload.
    pub(crate) fn message(self) -> &'static str {
        match self {
            Self::Unavailable | Self::Timeout =>
                "Array is unavailable. Reopen Annex from Array, or export a backup file.",
            Self::Protocol => "Array returned an invalid wallet response. Reopen Annex from Array.",
            Self::Busy => "The Array wallet is busy. Try again.",
            Self::Pairing => "Pair Annex in Array Settings to use its credential wallet.",
            Self::Locked => "Unlock the credential wallet in Array and try again.",
            Self::Denied => "Array did not allow this action. Check Annex's wallet permissions in Array Settings.",
            Self::Missing => "This backup is no longer available in Array.",
            Self::InvalidIdentity => "This is not a usable Annex identity backup.",
            Self::Preference => "Annex could not update its Array wallet preference. Check the Annex data folder and try again.",
        }
    }
}

struct Job {
    id: String,
    method: &'static str,
    params: Value,
    reply: mpsc::SyncSender<Result<Value, WalletError>>,
}

struct Transport {
    jobs: mpsc::SyncSender<Job>,
    alive: Arc<AtomicBool>,
}

impl Transport {
    fn stdio() -> Self {
        let (jobs, queue) = mpsc::sync_channel(8);
        let (responses, incoming) = mpsc::sync_channel(1);
        let alive = Arc::new(AtomicBool::new(true));
        let reader_alive = alive.clone();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(std::io::stdin());
            loop {
                let response = read_frame(&mut reader);
                let failed = response.is_err();
                if responses.send(response).is_err() || failed {
                    reader_alive.store(false, Ordering::Release);
                    break;
                }
            }
        });
        let worker_alive = alive.clone();
        std::thread::spawn(move || {
            let stdout = std::io::stdout();
            serve_requests(queue, incoming, worker_alive, REQUEST_TIMEOUT, |frame| {
                let mut writer = stdout.lock();
                writer.write_all(frame).and_then(|_| writer.flush())
            });
        });
        Self { jobs, alive }
    }

    fn request(&self, method: &'static str, params: Value) -> Result<Value, WalletError> {
        if !self.alive.load(Ordering::Acquire) {
            return Err(WalletError::Unavailable);
        }
        let (reply, result) = mpsc::sync_channel(1);
        self.jobs
            .try_send(Job {
                id: uuid::Uuid::new_v4().to_string(),
                method,
                params,
                reply,
            })
            .map_err(|e| match e {
                mpsc::TrySendError::Full(_) => WalletError::Busy,
                mpsc::TrySendError::Disconnected(_) => WalletError::Unavailable,
            })?;
        match result.recv_timeout(REQUEST_TIMEOUT) {
            Ok(result) => result,
            Err(_) => {
                self.alive.store(false, Ordering::Release);
                Err(WalletError::Timeout)
            }
        }
    }
}

fn serve_requests(
    jobs: mpsc::Receiver<Job>,
    incoming: mpsc::Receiver<Result<Value, WalletError>>,
    alive: Arc<AtomicBool>,
    timeout: Duration,
    mut write: impl FnMut(&[u8]) -> std::io::Result<()>,
) {
    for job in jobs {
        let result = (|| {
            if !alive.load(Ordering::Acquire) {
                return Err(WalletError::Unavailable);
            }
            let frame = encode_frame(
                &json!({ "id": &job.id, "method": job.method, "params": &job.params }),
            )?;
            write(&frame).map_err(|_| WalletError::Unavailable)?;
            let response = incoming
                .recv_timeout(timeout)
                .map_err(|_| WalletError::Timeout)??;
            decode_response(response, &job.id)
        })();
        if matches!(
            result,
            Err(WalletError::Unavailable | WalletError::Timeout | WalletError::Protocol)
        ) {
            alive.store(false, Ordering::Release);
        }
        let _ = job.reply.send(result);
    }
}

fn encode_frame(value: &Value) -> Result<Vec<u8>, WalletError> {
    let json = serde_json::to_vec(value).map_err(|_| WalletError::Protocol)?;
    if PREFIX.len() + json.len() + 1 > MAX_REQUEST_FRAME_BYTES {
        return Err(WalletError::Protocol);
    }
    let mut frame = Vec::with_capacity(PREFIX.len() + json.len() + 1);
    frame.extend_from_slice(PREFIX);
    frame.extend_from_slice(&json);
    frame.push(b'\n');
    Ok(frame)
}

fn read_frame(reader: &mut impl BufRead) -> Result<Value, WalletError> {
    let mut frame = Vec::new();
    loop {
        let bytes = reader.fill_buf().map_err(|_| WalletError::Unavailable)?;
        if bytes.is_empty() {
            return Err(WalletError::Unavailable);
        }
        let end = bytes.iter().position(|byte| *byte == b'\n');
        let length = end.map_or(bytes.len(), |index| index + 1);
        if frame.len() + length > MAX_RESPONSE_FRAME_BYTES {
            return Err(WalletError::Protocol);
        }
        frame.extend_from_slice(&bytes[..length]);
        reader.consume(length);
        if end.is_some() {
            break;
        }
    }
    if !frame.starts_with(PREFIX) {
        return Err(WalletError::Protocol);
    }
    serde_json::from_slice(&frame[PREFIX.len()..]).map_err(|_| WalletError::Protocol)
}

fn decode_response(response: Value, id: &str) -> Result<Value, WalletError> {
    let object = response.as_object().ok_or(WalletError::Protocol)?;
    if object.get("id").and_then(Value::as_str) != Some(id)
        || object.contains_key("result") == object.contains_key("error")
    {
        return Err(WalletError::Protocol);
    }
    if let Some(error) = object.get("error") {
        return Err(match error.get("code").and_then(Value::as_str) {
            Some("LOCKED" | "WALLET_LOCKED" | "wallet_locked") => WalletError::Locked,
            Some("NOT_PAIRED" | "PAIRING_REQUIRED" | "not_paired") => WalletError::Pairing,
            Some("NOT_FOUND" | "CREDENTIAL_NOT_FOUND" | "not_found") => WalletError::Missing,
            _ => WalletError::Denied,
        });
    }
    object.get("result").cloned().ok_or(WalletError::Protocol)
}

#[derive(Clone)]
pub(crate) struct ArrayWallet {
    transport: Option<Arc<Transport>>,
    detached: Arc<AtomicBool>,
    source: Arc<WalletSource>,
}

/// A profile-scoped, non-secret backup preference. Presence does not establish
/// trust in a parent process or prove that a remote pairing currently exists.
struct WalletSource {
    marker: Option<PathBuf>,
    managed: AtomicBool,
    persistence_error: AtomicBool,
}

impl WalletSource {
    fn at(marker: PathBuf, attached: bool) -> Self {
        let (remembered, read_failed) = match std::fs::symlink_metadata(&marker) {
            Ok(_) => (true, false),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (false, false),
            Err(_) => (true, true),
        };
        let source = Self {
            marker: Some(marker),
            managed: AtomicBool::new(attached || remembered),
            persistence_error: AtomicBool::new(read_failed),
        };
        // Remember the preference before the first handshake, even if Array
        // crashes during startup. An initial failure must not change Backup's
        // destination on the next standalone launch.
        if attached {
            let _ = source.remember();
        }
        source
    }

    fn remember(&self) -> Result<(), WalletError> {
        self.managed.store(true, Ordering::Release);
        let result =
            self.marker.as_ref().map_or(Ok(()), |marker| {
                match std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(marker)
                {
                    Ok(file) => file.sync_all(),
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
                    Err(error) => Err(error),
                }
            });
        self.persistence_error
            .store(result.is_err(), Ordering::Release);
        result.map_err(|_| WalletError::Preference)
    }

    fn clear(&self) -> Result<(), WalletError> {
        let result =
            self.marker
                .as_ref()
                .map_or(Ok(()), |marker| match std::fs::remove_file(marker) {
                    Ok(()) => Ok(()),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                    Err(error) => Err(error),
                });
        self.persistence_error
            .store(result.is_err(), Ordering::Release);
        result.map_err(|_| WalletError::Preference)?;
        self.managed.store(false, Ordering::Release);
        Ok(())
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WalletStatus {
    managed_by: Option<&'static str>,
    available: bool,
    paired: bool,
    account_label: Option<String>,
    message: Option<&'static str>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PortableIdentity {
    sk: String,
    role_code: u64,
    node_id: u64,
    commitment_hex: String,
    pseudonym_id: Option<String>,
    server_slug: String,
    created_at: String,
}

impl PortableIdentity {
    fn checked(mut self) -> Result<Self, WalletError> {
        if self.sk.is_empty()
            || self.sk.len() > 64
            || !self.sk.bytes().all(|c| c.is_ascii_hexdigit())
            || self.commitment_hex.len() != 64
            || !self.commitment_hex.bytes().all(|c| c.is_ascii_hexdigit())
            || self.role_code == 0
            || self.role_code > 255
            || self.node_id > 9_007_199_254_740_991
            || self.server_slug.len() > 512
            || self.created_at.is_empty()
            || self.created_at.len() > 128
            || self
                .pseudonym_id
                .as_ref()
                .is_some_and(|id| id.encode_utf16().count() > 256)
        {
            return Err(WalletError::InvalidIdentity);
        }
        self.sk = self.sk.to_ascii_lowercase();
        self.commitment_hex = self.commitment_hex.to_ascii_lowercase();
        let padded = format!("{:0>64}", self.sk);
        if padded.as_str() >= FIELD_ORDER_HEX || self.sk.bytes().all(|c| c == b'0') {
            return Err(WalletError::InvalidIdentity);
        }
        Ok(self)
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WalletReceipt {
    credential_id: String,
    saved_at: String,
    managed_by: &'static str,
}

#[derive(Serialize)]
pub(crate) struct WalletBackup {
    id: String,
    label: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WalletRecovery {
    format: &'static str,
    value: PortableIdentity,
    managed_by: &'static str,
}

fn text_field(value: &Value, key: &str, max: usize) -> Result<String, WalletError> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty() && text.len() <= max)
        .map(str::to_owned)
        .ok_or(WalletError::Protocol)
}

impl ArrayWallet {
    pub(crate) fn from_environment(data_dir: &Path) -> Self {
        let enabled = std::env::var("KI_ARRAY_WALLET_PROTOCOL").ok().as_deref() == Some("stdio-v1");
        let source = Arc::new(WalletSource::at(
            data_dir.join("array-wallet-managed"),
            enabled,
        ));
        Self {
            transport: enabled.then(|| Arc::new(Transport::stdio())),
            detached: Arc::new(AtomicBool::new(false)),
            source,
        }
    }

    /// Call only from Tauri's primary-process setup. The singleton plugin must
    /// decide which process survives before this child acknowledges Array.
    pub(crate) fn start_handshake(&self) {
        if self.transport.is_some() {
            let wallet = self.clone();
            std::thread::spawn(move || {
                let _ = wallet.hello();
            });
        }
    }

    fn request(&self, method: &'static str, params: Value) -> Result<Value, WalletError> {
        if self.detached.load(Ordering::Acquire) {
            return Err(WalletError::Pairing);
        }
        self.transport
            .as_ref()
            .ok_or(WalletError::Unavailable)?
            .request(method, params)
    }

    fn hello(&self) -> Result<bool, WalletError> {
        let result = self.request("hello", json!({ "protocol": 1 }))?;
        if result.get("protocol").and_then(Value::as_u64) != Some(1)
            || result.get("managedBy").and_then(Value::as_str) != Some("array")
            || result.get("appId").and_then(Value::as_str) != Some("annex")
        {
            return Err(WalletError::Protocol);
        }
        result
            .get("paired")
            .and_then(Value::as_bool)
            .ok_or(WalletError::Protocol)
    }

    fn require_pairing(&self) -> Result<(), WalletError> {
        if self.source.persistence_error.load(Ordering::Acquire) {
            return Err(WalletError::Preference);
        }
        if self.hello()? {
            Ok(())
        } else {
            Err(WalletError::Pairing)
        }
    }

    pub(crate) fn status(&self) -> WalletStatus {
        let mut status = WalletStatus {
            managed_by: None,
            available: false,
            paired: false,
            account_label: None,
            message: None,
        };
        if !self.source.managed.load(Ordering::Acquire) {
            return status;
        }
        // A previously managed connection failing must not silently turn Backup
        // into a local export or imply that the remote pairing was revoked.
        status.managed_by = Some("array");
        if self.source.persistence_error.load(Ordering::Acquire) {
            status.message = Some(WalletError::Preference.message());
            return status;
        }
        if self.transport.is_none() {
            status.message = Some(WalletError::Unavailable.message());
            return status;
        }
        let result = (|| {
            self.hello()?;
            let result = self.request("status", json!({}))?;
            if result.get("managedBy").and_then(Value::as_str) != Some("array") {
                return Err(WalletError::Protocol);
            }
            let paired = result
                .get("paired")
                .and_then(Value::as_bool)
                .ok_or(WalletError::Protocol)?;
            status.available = true;
            status.paired = paired;
            status.account_label = result
                .get("accountLabel")
                .and_then(Value::as_str)
                .filter(|label| label.len() <= 768)
                .map(str::to_owned);
            if !paired {
                status.message = Some(WalletError::Pairing.message());
            }
            Ok::<(), WalletError>(())
        })();
        if let Err(error) = result {
            status.message = Some(error.message());
        }
        status
    }

    pub(crate) fn save(&self, identity: PortableIdentity) -> Result<WalletReceipt, WalletError> {
        let value = identity.checked()?;
        let scope = if value.server_slug.is_empty() {
            "Unregistered"
        } else {
            &value.server_slug
        };
        let mut context = String::new();
        for character in scope.chars() {
            if context.len() + character.len_utf8() > 56 {
                break;
            }
            context.push(character);
        }
        let label = format!(
            "Annex ZKP key · {} · {}",
            context,
            &value.commitment_hex[..8]
        );
        self.require_pairing()?;
        let result = self.request(
            "recovery.put",
            json!({ "label": label, "format": FORMAT, "value": value }),
        )?;
        if result.get("managedBy").and_then(Value::as_str) != Some("array") {
            return Err(WalletError::Protocol);
        }
        Ok(WalletReceipt {
            credential_id: text_field(&result, "credentialId", 256)?,
            saved_at: text_field(&result, "savedAt", 128)?,
            managed_by: "array",
        })
    }

    pub(crate) fn list(&self) -> Result<Vec<WalletBackup>, WalletError> {
        self.require_pairing()?;
        let result = self.request("recovery.list", json!({}))?;
        let records = result
            .get("credentials")
            .and_then(Value::as_array)
            .ok_or(WalletError::Protocol)?;
        if records.len() > 250 {
            return Err(WalletError::Protocol);
        }
        records
            .iter()
            .filter(|record| {
                record
                    .get("format")
                    .and_then(Value::as_str)
                    .is_none_or(|format| format == FORMAT)
            })
            .map(|record| {
                Ok(WalletBackup {
                    id: text_field(record, "id", 256)?,
                    label: text_field(record, "label", 512)?,
                })
            })
            .collect()
    }

    pub(crate) fn restore(&self, credential_id: String) -> Result<WalletRecovery, WalletError> {
        if credential_id.is_empty() || credential_id.len() > 256 {
            return Err(WalletError::Missing);
        }
        self.require_pairing()?;
        let result = self.request("recovery.get", json!({ "credentialId": credential_id }))?;
        if result.get("format").and_then(Value::as_str) != Some(FORMAT)
            || result.get("managedBy").and_then(Value::as_str) != Some("array")
        {
            return Err(WalletError::Protocol);
        }
        let value: PortableIdentity =
            serde_json::from_value(result.get("value").cloned().ok_or(WalletError::Protocol)?)
                .map_err(|_| WalletError::InvalidIdentity)?;
        Ok(WalletRecovery {
            format: FORMAT,
            value: value.checked()?,
            managed_by: "array",
        })
    }

    pub(crate) fn disconnect(&self) -> Result<WalletStatus, WalletError> {
        if !self.source.managed.load(Ordering::Acquire) {
            return Ok(self.status());
        }
        if !self.detached.load(Ordering::Acquire) {
            let receipt = self.request("disconnect", json!({}))?;
            if receipt.get("paired").and_then(Value::as_bool) != Some(false)
                || receipt.get("managedBy") != Some(&Value::Null)
            {
                return Err(WalletError::Protocol);
            }
            self.detached.store(true, Ordering::Release);
        }
        // If deleting the local preference fails after the remote ACK, retain
        // the acknowledgement so a retry can clear it without a live pipe.
        self.source.clear()?;
        Ok(self.status())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn frame_round_trip_and_split_reads_preserve_payload() {
        let payload = json!({ "id": "r1", "result": { "value": "line one\nline two" } });
        let bytes = encode_frame(&payload).unwrap();
        let mut reader = BufReader::with_capacity(3, Cursor::new(bytes));
        assert_eq!(read_frame(&mut reader).unwrap(), payload);
    }

    #[test]
    fn rejects_oversized_truncated_nonprotocol_and_ambiguous_frames() {
        assert_eq!(
            encode_frame(&json!({"value": "x".repeat(MAX_REQUEST_FRAME_BYTES)})),
            Err(WalletError::Protocol)
        );
        let oversized = [PREFIX, "x".repeat(MAX_RESPONSE_FRAME_BYTES).as_bytes()].concat();
        assert_eq!(
            read_frame(&mut Cursor::new(oversized)),
            Err(WalletError::Protocol)
        );
        assert_eq!(
            read_frame(&mut Cursor::new(b"KI_ARRAY_WALLET_V1 {}")),
            Err(WalletError::Unavailable)
        );
        assert_eq!(
            read_frame(&mut Cursor::new(b"{}\n")),
            Err(WalletError::Protocol)
        );
        assert_eq!(
            decode_response(json!({"id":"other","result":{}}), "r1"),
            Err(WalletError::Protocol)
        );
        assert_eq!(
            decode_response(json!({"id":"r1","result":{},"error":{}}), "r1"),
            Err(WalletError::Protocol)
        );
    }

    #[test]
    fn remote_error_messages_and_payloads_never_reach_ui() {
        let error = decode_response(
            json!({"id":"r1","error":{"code":"unexpected","message":"private-value"}}),
            "r1",
        )
        .unwrap_err();
        assert_eq!(error, WalletError::Denied);
        assert!(!error.message().contains("private-value"));
    }

    fn identity(sk: &str) -> PortableIdentity {
        PortableIdentity {
            sk: sk.into(),
            role_code: 1,
            node_id: 1,
            commitment_hex: "a".repeat(64),
            pseudonym_id: None,
            server_slug: String::new(),
            created_at: "2026-10-19T00:00:00Z".into(),
        }
    }

    #[test]
    fn backup_validation_accepts_short_scalars_and_strips_session_fields() {
        assert!(identity("1").checked().is_ok());
        let mut zero_node = identity("1");
        zero_node.node_id = 0;
        assert!(zero_node.checked().is_ok());
        assert!(identity("0").checked().is_err());
        assert!(identity(FIELD_ORDER_HEX).checked().is_err());
        assert!(identity("0x1").checked().is_err());
        let mut source = serde_json::to_value(identity("1")).unwrap();
        source["sessionToken"] = json!("source-session");
        source["zkProofPayload"] = json!("source-proof");
        source["id"] = json!("source-local-id");
        let portable: PortableIdentity = serde_json::from_value(source).unwrap();
        let exported = serde_json::to_value(portable.checked().unwrap()).unwrap();
        assert!(exported.get("sessionToken").is_none());
        assert!(exported.get("zkProofPayload").is_none());
        assert!(exported.get("id").is_none());
    }

    #[test]
    fn accepts_complete_large_metadata_responses_without_relaxing_request_limit() {
        let records: Vec<_> = (0..250)
            .map(|index| {
                json!({
                    "id": format!("backup-{index}"), "label": "界".repeat(120),
                    "format": FORMAT, "updatedAt": "2026-10-19T00:00:00Z",
                })
            })
            .collect();
        let payload = json!({"id":"list","result":{"credentials":records}});
        let mut frame = PREFIX.to_vec();
        frame.extend(serde_json::to_vec(&payload).unwrap());
        frame.push(b'\n');
        assert!(frame.len() > MAX_REQUEST_FRAME_BYTES);
        assert!(frame.len() < MAX_RESPONSE_FRAME_BYTES);
        let decoded = read_frame(&mut BufReader::with_capacity(4096, Cursor::new(frame))).unwrap();
        assert_eq!(
            decoded["result"]["credentials"].as_array().unwrap().len(),
            250
        );
    }

    #[test]
    fn worker_matches_each_request_and_stops_after_protocol_failure() {
        let (jobs, queue) = mpsc::sync_channel(3);
        let (responses, incoming) = mpsc::sync_channel(3);
        let alive = Arc::new(AtomicBool::new(true));
        let mut replies = Vec::new();
        for id in ["first", "second", "third"] {
            let (reply, receiver) = mpsc::sync_channel(1);
            jobs.send(Job {
                id: id.into(),
                method: "status",
                params: json!({}),
                reply,
            })
            .unwrap();
            replies.push(receiver);
        }
        drop(jobs);
        responses
            .send(Ok(json!({"id":"first","result":{"paired":true}})))
            .unwrap();
        responses
            .send(Ok(json!({"id":"wrong","result":{}})))
            .unwrap();
        let mut writes = 0;
        serve_requests(queue, incoming, alive.clone(), REQUEST_TIMEOUT, |_| {
            writes += 1;
            Ok(())
        });
        assert!(replies[0].recv().unwrap().is_ok());
        assert_eq!(replies[1].recv().unwrap(), Err(WalletError::Protocol));
        assert_eq!(replies[2].recv().unwrap(), Err(WalletError::Unavailable));
        assert_eq!(writes, 2);
        assert!(!alive.load(Ordering::Acquire));
    }

    fn simulated_parent(
        mut answer: impl FnMut(&Value) -> Value + Send + 'static,
    ) -> (ArrayWallet, std::thread::JoinHandle<()>) {
        let (jobs, queue) = mpsc::sync_channel(8);
        let (responses, incoming) = mpsc::sync_channel(1);
        let alive = Arc::new(AtomicBool::new(true));
        let worker_alive = alive.clone();
        let worker = std::thread::spawn(move || {
            serve_requests(queue, incoming, worker_alive, REQUEST_TIMEOUT, |frame| {
                assert!(frame.len() <= MAX_REQUEST_FRAME_BYTES);
                let request = read_frame(&mut Cursor::new(frame)).unwrap();
                let mut response = answer(&request);
                response["id"] = request["id"].clone();
                responses.send(Ok(response)).unwrap();
                Ok(())
            });
        });
        (
            ArrayWallet {
                transport: Some(Arc::new(Transport { jobs, alive })),
                detached: Arc::new(AtomicBool::new(false)),
                source: Arc::new(WalletSource {
                    marker: None,
                    managed: AtomicBool::new(true),
                    persistence_error: AtomicBool::new(false),
                }),
            },
            worker,
        )
    }

    #[test]
    fn backup_restore_and_disconnect_use_scoped_protocol_and_durable_receipts() {
        let mut saved = Value::Null;
        let (wallet, worker) = simulated_parent(move |request| {
            assert!(request["params"].get("appId").is_none());
            match request["method"].as_str().unwrap() {
                "hello" => {
                    json!({"result":{"protocol":1,"managedBy":"array","appId":"annex","paired":true}})
                }
                "status" => {
                    json!({"result":{"managedBy":"array","paired":true,"accountLabel":"Test account"}})
                }
                "recovery.put" => {
                    assert_eq!(request["params"]["format"], FORMAT);
                    assert!(request["params"]["label"]
                        .as_str()
                        .unwrap()
                        .contains("alpha"));
                    saved = request["params"]["value"].clone();
                    json!({"result":{"credentialId":"saved-key","savedAt":"2026-10-19T00:00:00Z","managedBy":"array"}})
                }
                "recovery.list" => {
                    json!({"result":{"credentials":[{"id":"saved-key","label":"Annex alpha","format":FORMAT}]}})
                }
                "recovery.get" => {
                    assert_eq!(request["params"]["credentialId"], "saved-key");
                    json!({"result":{"format":FORMAT,"value":saved,"managedBy":"array"}})
                }
                "disconnect" => json!({"result":{"paired":false,"managedBy":null}}),
                _ => panic!("unexpected wallet method"),
            }
        });
        assert!(wallet.status().paired);
        let mut backup = identity("1");
        backup.server_slug = "alpha".into();
        let receipt = wallet.save(backup).unwrap();
        assert_eq!(receipt.credential_id, "saved-key");
        assert_eq!(wallet.list().unwrap()[0].id, "saved-key");
        let restored = wallet.restore("saved-key".into()).unwrap();
        assert_eq!(restored.value.sk, "1");
        assert_eq!(restored.value.server_slug, "alpha");
        assert_eq!(wallet.disconnect().unwrap().managed_by, None);
        assert!(!wallet.status().paired);
        assert_eq!(
            wallet.restore("saved-key".into()).err(),
            Some(WalletError::Pairing)
        );
        drop(wallet);
        worker.join().unwrap();
    }

    #[test]
    fn denied_save_has_no_receipt_and_failed_disconnect_keeps_managed_state() {
        let (wallet, worker) = simulated_parent(|request| {
            match request["method"].as_str().unwrap() {
                "hello" => {
                    json!({"result":{"protocol":1,"managedBy":"array","appId":"annex","paired":true}})
                }
                "status" => json!({"result":{"managedBy":"array","paired":true}}),
                _ => {
                    json!({"error":{"code":"WALLET_LOCKED","message":"private parent diagnostic"}})
                }
            }
        });
        assert_eq!(wallet.save(identity("1")).err(), Some(WalletError::Locked));
        assert_eq!(wallet.disconnect().err(), Some(WalletError::Locked));
        assert_eq!(wallet.status().managed_by, Some("array"));
        drop(wallet);
        worker.join().unwrap();
    }

    #[test]
    fn timeout_closes_channel_and_does_not_send_queued_request() {
        let (jobs, queue) = mpsc::sync_channel(2);
        let (_responses, incoming) = mpsc::sync_channel(1);
        let alive = Arc::new(AtomicBool::new(true));
        let (first, first_reply) = mpsc::sync_channel(1);
        let (second, second_reply) = mpsc::sync_channel(1);
        jobs.send(Job {
            id: "first".into(),
            method: "status",
            params: json!({}),
            reply: first,
        })
        .unwrap();
        jobs.send(Job {
            id: "second".into(),
            method: "status",
            params: json!({}),
            reply: second,
        })
        .unwrap();
        drop(jobs);
        let mut writes = 0;
        serve_requests(
            queue,
            incoming,
            alive.clone(),
            Duration::from_millis(10),
            |_| {
                writes += 1;
                Ok(())
            },
        );
        assert_eq!(first_reply.recv().unwrap(), Err(WalletError::Timeout));
        assert_eq!(second_reply.recv().unwrap(), Err(WalletError::Unavailable));
        assert_eq!(writes, 1);
        assert!(!alive.load(Ordering::Acquire));
    }

    fn marker_path() -> PathBuf {
        let directory =
            std::env::temp_dir().join(format!("annex-wallet-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&directory).unwrap();
        directory.join("array-wallet-managed")
    }

    fn remembered_wallet(marker: PathBuf) -> ArrayWallet {
        ArrayWallet {
            transport: None,
            detached: Arc::new(AtomicBool::new(false)),
            source: Arc::new(WalletSource::at(marker, false)),
        }
    }

    #[test]
    fn first_handshake_failure_remains_managed_on_later_standalone_launch() {
        let marker = marker_path();
        assert_eq!(remembered_wallet(marker.clone()).status().managed_by, None);
        let (mut wallet, worker) = simulated_parent(|_| json!({"error":{"code":"WALLET_LOCKED"}}));
        wallet.source = Arc::new(WalletSource::at(marker.clone(), true));
        // Persistence is complete before any parent request is made.
        assert!(marker.is_file());
        assert_eq!(wallet.hello(), Err(WalletError::Locked));
        drop(wallet);
        worker.join().unwrap();
        let reopened = remembered_wallet(marker.clone());
        let status = reopened.status();
        assert_eq!(status.managed_by, Some("array"));
        assert!(!status.available && !status.paired);
        assert_eq!(reopened.disconnect().err(), Some(WalletError::Unavailable));
        assert!(marker.is_file());
        std::fs::remove_dir_all(marker.parent().unwrap()).unwrap();
    }

    #[test]
    fn disconnect_requires_explicit_receipt_before_clearing_profile_preference() {
        let marker = marker_path();
        let mut attempt = 0;
        let (mut wallet, worker) = simulated_parent(move |request| {
            assert_eq!(request["method"], "disconnect");
            attempt += 1;
            match attempt {
                1 => json!({"result":{"paired":true,"managedBy":"array"}}),
                2 => json!({"result":{"paired":false}}),
                _ => json!({"result":{"paired":false,"managedBy":null}}),
            }
        });
        wallet.source = Arc::new(WalletSource::at(marker.clone(), true));
        for _ in 0..2 {
            assert_eq!(wallet.disconnect().err(), Some(WalletError::Protocol));
            assert!(!wallet.detached.load(Ordering::Acquire));
            assert!(marker.is_file());
        }
        assert_eq!(wallet.disconnect().unwrap().managed_by, None);
        assert!(!marker.exists());
        assert_eq!(remembered_wallet(marker.clone()).status().managed_by, None);
        drop(wallet);
        worker.join().unwrap();
        std::fs::remove_dir_all(marker.parent().unwrap()).unwrap();
    }

    #[test]
    fn preference_write_and_clear_failures_are_visible_and_do_not_claim_revocation() {
        let marker = marker_path();
        let failed_source = WalletSource::at(marker.join("missing-parent"), true);
        assert!(failed_source.managed.load(Ordering::Acquire));
        assert!(failed_source.persistence_error.load(Ordering::Acquire));
        let failed_wallet = ArrayWallet {
            transport: None,
            detached: Arc::new(AtomicBool::new(false)),
            source: Arc::new(failed_source),
        };
        assert_eq!(failed_wallet.status().managed_by, Some("array"));
        assert_eq!(
            failed_wallet.status().message,
            Some(WalletError::Preference.message())
        );

        let (mut wallet, worker) = simulated_parent(|request| {
            assert_eq!(request["method"], "disconnect");
            json!({"result":{"paired":false,"managedBy":null}})
        });
        wallet.source = Arc::new(WalletSource::at(marker.clone(), true));
        std::fs::remove_file(&marker).unwrap();
        std::fs::create_dir(&marker).unwrap();
        assert_eq!(wallet.disconnect().err(), Some(WalletError::Preference));
        assert!(wallet.detached.load(Ordering::Acquire));
        assert_eq!(
            wallet.status().message,
            Some(WalletError::Preference.message())
        );
        std::fs::remove_dir(&marker).unwrap();
        // The remote ACK is retained; clearing the local preference can retry
        // without sending a second request on Array's now-closed channel.
        assert_eq!(wallet.disconnect().unwrap().managed_by, None);
        drop(wallet);
        worker.join().unwrap();
        std::fs::remove_dir_all(marker.parent().unwrap()).unwrap();
    }

    #[test]
    fn pseudonym_limit_matches_javascript_utf16_units() {
        for pseudonym in ["界".repeat(256), "🔑".repeat(128)] {
            let mut value = identity("1");
            value.pseudonym_id = Some(pseudonym);
            assert!(value.checked().is_ok());
        }
        let mut value = identity("1");
        value.pseudonym_id = Some("🔑".repeat(129));
        assert!(value.checked().is_err());
    }
}
