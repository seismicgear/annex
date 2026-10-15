//! `annex://` deep-link parsing and the cold-start invite handoff.
//!
//! Deep-link URLs arrive in two shapes:
//!   * Cold start — the OS launches the app with the URL on the command line.
//!     `main()` parses any URLs it sees during `setup()` and stashes the first
//!     valid invite in `AppManagedState::pending_invite`. The frontend polls
//!     for it via [`get_pending_invite`].
//!   * Runtime — the app is already running and `tauri-plugin-deep-link` emits
//!     the new URL on the `deep-link://new-url` channel. `main()` listens
//!     for that and emits an `annex-invite` Tauri event.
//!
//! Both paths retain a delivery until the frontend acknowledges its UUID.
//! Runtime wiring lives in `main()` because it uses the Tauri builder.

use serde::Serialize;

use crate::app_state::AppManagedState;

/// Parsed invite from an `annex://` protocol handler URL.
/// Emitted to the frontend as a Tauri event.
#[derive(Clone, Serialize)]
pub(crate) struct DeepLinkInvite {
    pub(crate) server: String,
    pub(crate) code: String,
}

// An invite code is a bearer credential. Debug output must not accidentally
// expose it or a server URL that itself contains private path/query material.
impl std::fmt::Debug for DeepLinkInvite {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DeepLinkInvite")
            .field("server", &"[redacted]")
            .field("code", &"[redacted]")
            .finish()
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct InviteDelivery {
    delivery_id: String,
    #[serde(flatten)]
    invite: DeepLinkInvite,
}

/// Latest invite wins, but a stale React invocation cannot consume or erase it.
#[derive(Default)]
pub(crate) struct InviteInbox {
    pending: Option<InviteDelivery>,
    runtime_received: bool,
}

impl InviteInbox {
    pub(crate) fn buffer_cold(&mut self, invite: DeepLinkInvite) {
        if !self.runtime_received {
            self.replace(invite);
        }
    }

    pub(crate) fn buffer_runtime(&mut self, invite: DeepLinkInvite) -> InviteDelivery {
        self.runtime_received = true;
        self.replace(invite)
    }

    fn replace(&mut self, invite: DeepLinkInvite) -> InviteDelivery {
        let delivery = InviteDelivery {
            delivery_id: uuid::Uuid::new_v4().to_string(),
            invite,
        };
        self.pending = Some(delivery.clone());
        delivery
    }

    fn acknowledge(&mut self, id: &str) -> bool {
        if self
            .pending
            .as_ref()
            .is_some_and(|value| value.delivery_id == id)
        {
            self.pending = None;
            true
        } else {
            false
        }
    }
}

/// Parse an `annex://invite?server=...&code=...` URL.
///
/// Returns `None` unless the URL is a well-formed `annex://invite` carrying
/// both parameters, and `server` parses as an `https` URL with a host.
pub(crate) fn parse_deep_link_invite(raw_url: &str) -> Option<DeepLinkInvite> {
    if raw_url.len() > 8192 || raw_url.chars().any(char::is_control) {
        return None;
    }
    let parsed = url::Url::parse(raw_url).ok()?;

    if parsed.scheme() != "annex" {
        return None;
    }

    if parsed.host_str() != Some("invite") {
        return None;
    }
    if !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.port().is_some()
        || !matches!(parsed.path(), "" | "/")
        || parsed.fragment().is_some()
    {
        return None;
    }

    let mut server = None;
    let mut code = None;

    for (key, value) in parsed.query_pairs() {
        match key.as_ref() {
            "server" if server.is_none() => server = Some(value.into_owned()),
            "code" if code.is_none() => code = Some(value.into_owned()),
            "server" | "code" => return None,
            _ => {}
        }
    }

    let server = server?;
    let code = code?;

    // Parse it rather than matching a prefix. `starts_with("https://")` accepts
    // the bare string `"https://"`, which has no host at all, and says nothing
    // about what follows — the app then carries the value into a confirmation
    // banner and, on approval, into every request it makes. A URL that does not
    // parse, is not https, or names no host is not a server.
    //
    // The confirmation banner is still the real gate: this URL arrives from
    // whatever asked the OS to open an `annex://` link, which is not a trusted
    // source. This only ensures what reaches that banner is a server address.
    annex_server::api_invite::validate_invite_destination(&server, false).ok()?;
    // Credentials in the address make the banner lie. In
    // `https://annex.trusted.example@evil.com` the host is `evil.com` and the
    // part that reads as the hostname is the username — and this string is
    // what the confirmation banner renders ("Invite received for …") while the
    // accept handler passes it straight to `redeemInvite` and
    // `beginRemoteRegistration`. The server the user approves and the server
    // the app contacts would be different machines, which is precisely what
    // the banner exists to prevent. Annex never sends credentials in a URL, so
    // there is nothing here to support.
    // An empty `code=` reaches the banner and then a redeem request that can
    // only fail. It is not a code.
    if code.is_empty()
        || code.len() > 100
        || code.trim() != code
        || code.chars().any(char::is_control)
    {
        return None;
    }

    Some(DeepLinkInvite { server, code })
}

/// Read a buffered invite without consuming it across a frontend remount.
///
/// During app launch, the deep-link URL may arrive before the React event
/// listener has mounted. This command lets the frontend poll for any invite
/// that was parsed during `setup()`. Returns `null` if none buffered.
/// Only [`ack_pending_invite`] clears the exact delivery accepted by the UI.
#[tauri::command]
pub(crate) fn get_pending_invite(
    state: tauri::State<'_, AppManagedState>,
) -> Option<InviteDelivery> {
    state
        .pending_invite
        .lock()
        .ok()
        .and_then(|guard| guard.pending.clone())
}

#[tauri::command]
pub(crate) fn ack_pending_invite(
    state: tauri::State<'_, AppManagedState>,
    delivery_id: String,
) -> bool {
    state
        .pending_invite
        .lock()
        .is_ok_and(|mut inbox| inbox.acknowledge(&delivery_id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delivery_is_retained_until_its_exact_ack_and_stale_ack_cannot_clear_new_click() {
        let mut inbox = InviteInbox::default();
        let make = || DeepLinkInvite {
            server: "https://annex.example.test".into(),
            code: "same-code".into(),
        };
        inbox.buffer_cold(make());
        let cold = inbox.pending.clone().unwrap();
        assert_eq!(
            inbox.pending.as_ref().unwrap().delivery_id,
            cold.delivery_id
        );
        let runtime = inbox.buffer_runtime(make());
        assert_ne!(cold.delivery_id, runtime.delivery_id);
        assert!(!inbox.acknowledge(&cold.delivery_id));
        assert_eq!(
            inbox.pending.as_ref().unwrap().delivery_id,
            runtime.delivery_id
        );
        assert!(inbox.acknowledge(&runtime.delivery_id));
        assert!(inbox.pending.is_none());
        // Startup inspection racing a handled runtime click cannot resurrect
        // the old invite after React already acknowledged its newer delivery.
        inbox.buffer_cold(make());
        assert!(inbox.pending.is_none());
    }

    #[test]
    fn parser_rejects_ambiguous_parameters_envelopes_and_unroutable_origins() {
        for link in [
            "annex://invite?server=https://good.example&server=https://evil.example&code=x",
            "annex://invite?server=https://good.example&code=x&code=y",
            "annex://user@invite?server=https://good.example&code=x",
            "annex://invite:123?server=https://good.example&code=x",
            "annex://invite/other?server=https://good.example&code=x",
            "annex://invite?server=https://good.example&code=x#fragment",
            "annex://invite?server=https%3A%2F%2Fgood.example%2Fpath&code=x",
            "annex://invite?server=https%3A%2F%2Fgood.example%3Fx%3Dy&code=x",
            "annex://invite?server=https://good.example&code=%00secret",
            "annex://invite?server=https://good.example&code=%20secret",
        ] {
            assert!(parse_deep_link_invite(link).is_none(), "accepted {link}");
        }
        assert!(parse_deep_link_invite(&format!(
            "annex://invite?server=https://good.example&code={}",
            "x".repeat(101)
        ))
        .is_none());
        // Private HTTPS is allowed only for a direct, explicitly confirmed
        // native invite; generating a public hosted share link rejects it.
        assert!(
            parse_deep_link_invite("annex://invite?server=https://127.0.0.1:8443&code=x").is_some()
        );
    }

    #[test]
    fn debug_redacts_invite_credentials_without_changing_frontend_handoff() {
        let invite = DeepLinkInvite {
            server: "https://private.example/private?token=secret-query".into(),
            code: "secret-invite-code".into(),
        };
        let diagnostic = format!("{invite:?}");
        assert!(!diagnostic.contains("private.example"));
        assert!(!diagnostic.contains("secret"));
        let handoff = serde_json::to_value(&invite).unwrap();
        assert_eq!(handoff["code"], "secret-invite-code");
        assert_eq!(handoff["server"], invite.server);
    }

    #[test]
    fn parse_valid_annex_invite_deep_link() {
        let url = "annex://invite?server=https%3A%2F%2Fannex.example.com&code=abc123";
        let invite = parse_deep_link_invite(url).unwrap();
        assert_eq!(invite.server, "https://annex.example.com");
        assert_eq!(invite.code, "abc123");
    }

    #[test]
    fn parse_deep_link_invite_rejects_http() {
        let url = "annex://invite?server=http%3A%2F%2Fannex.example.com&code=abc123";
        assert!(parse_deep_link_invite(url).is_none());
    }

    #[test]
    fn parse_deep_link_invite_rejects_wrong_path() {
        let url = "annex://settings?server=https%3A%2F%2Fannex.example.com&code=abc123";
        assert!(parse_deep_link_invite(url).is_none());
    }

    #[test]
    fn parse_deep_link_invite_missing_code() {
        let url = "annex://invite?server=https%3A%2F%2Fannex.example.com";
        assert!(parse_deep_link_invite(url).is_none());
    }

    #[test]
    fn parse_deep_link_invite_rejects_a_scheme_with_no_host() {
        // `starts_with("https://")` accepted this.
        let url = "annex://invite?server=https%3A%2F%2F&code=abc123";
        assert!(parse_deep_link_invite(url).is_none());
    }

    #[test]
    fn parse_deep_link_invite_rejects_credentials_in_the_server() {
        // `https://annex.trusted.example@evil.com` has host `evil.com`; the
        // part that reads as the hostname is the username. The confirmation
        // banner renders this string verbatim ("Invite received for …") and
        // the accept handler passes it to `redeemInvite` and
        // `beginRemoteRegistration`, so the server the user approves and the
        // server the app contacts would be different machines.
        let url =
            "annex://invite?server=https%3A%2F%2Fannex.trusted.example%40evil.com&code=abc123";
        assert!(parse_deep_link_invite(url).is_none());
    }

    #[test]
    fn parse_deep_link_invite_rejects_a_password_in_the_server() {
        let url = "annex://invite?server=https%3A%2F%2Fuser%3Apass%40evil.com&code=abc123";
        assert!(parse_deep_link_invite(url).is_none());
    }

    #[test]
    fn parse_deep_link_invite_rejects_an_empty_code() {
        // An empty code reaches the banner and then the redeem request, which
        // can only fail. `code=` is not a code.
        let url = "annex://invite?server=https%3A%2F%2Fannex.example.com&code=";
        assert!(parse_deep_link_invite(url).is_none());
    }

    #[test]
    fn parse_deep_link_invite_rejects_an_unparseable_server() {
        let url = "annex://invite?server=https%3A%2F%2F%5B&code=abc123";
        assert!(parse_deep_link_invite(url).is_none());
    }

    #[test]
    fn parse_deep_link_invite_keeps_an_explicit_port() {
        let url = "annex://invite?server=https%3A%2F%2Fannex.example.com%3A8443&code=abc123";
        let invite = parse_deep_link_invite(url).unwrap();
        assert_eq!(invite.server, "https://annex.example.com:8443");
    }

    #[test]
    fn parse_deep_link_invite_missing_server() {
        let url = "annex://invite?code=abc123";
        assert!(parse_deep_link_invite(url).is_none());
    }
}
