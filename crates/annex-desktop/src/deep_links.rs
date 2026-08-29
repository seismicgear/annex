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
//! Only the cold-start path is implemented in this module; the runtime path
//! lives in `main()` because it wires into the Tauri builder directly.

use serde::Serialize;

use crate::app_state::AppManagedState;

/// Parsed invite from an `annex://` protocol handler URL.
/// Emitted to the frontend as a Tauri event.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct DeepLinkInvite {
    pub(crate) server: String,
    pub(crate) code: String,
}

/// Parse an `annex://invite?server=...&code=...` URL.
///
/// Returns `None` unless the URL is a well-formed `annex://invite` carrying
/// both parameters, and `server` parses as an `https` URL with a host.
pub(crate) fn parse_deep_link_invite(raw_url: &str) -> Option<DeepLinkInvite> {
    let parsed = url::Url::parse(raw_url).ok()?;

    if parsed.scheme() != "annex" {
        return None;
    }

    if parsed.host_str() != Some("invite") {
        return None;
    }

    let mut server = None;
    let mut code = None;

    for (key, value) in parsed.query_pairs() {
        match key.as_ref() {
            "server" => server = Some(value.into_owned()),
            "code" => code = Some(value.into_owned()),
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
    let parsed_server = url::Url::parse(&server).ok()?;
    if parsed_server.scheme() != "https" {
        return None;
    }
    if parsed_server.host_str().is_none_or(str::is_empty) {
        return None;
    }

    Some(DeepLinkInvite { server, code })
}

/// Retrieve and clear a buffered cold-start invite.
///
/// During app launch, the deep-link URL may arrive before the React event
/// listener has mounted. This command lets the frontend poll for any invite
/// that was parsed during `setup()`. Returns `null` if none buffered.
/// Clears the buffer so the invite is processed exactly once.
#[tauri::command]
pub(crate) fn get_pending_invite(
    state: tauri::State<'_, AppManagedState>,
) -> Option<DeepLinkInvite> {
    state
        .pending_invite
        .lock()
        .ok()
        .and_then(|mut guard| guard.take())
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn parse_deep_link_invite_rejects_an_unparseable_server() {
        let url = "annex://invite?server=https%3A%2F%2F%5B&code=abc123";
        assert!(parse_deep_link_invite(url).is_none());
    }

    #[test]
    fn parse_deep_link_invite_keeps_a_port_and_path() {
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
