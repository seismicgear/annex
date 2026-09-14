//! Which posture this binary runs in, and who gets to decide.
//!
//! Several gates behave differently outside development: CORS refuses a
//! wildcard or an empty origin list, clustered mode refuses an in-memory rate
//! limiter, the dev-localhost CORS relaxation is forced off, weak signing keys
//! are rejected, ZK enforcement cannot be switched off, and the first
//! registrant is not silently made founder.
//!
//! Two things were wrong with how that was decided before.
//!
//! **The default was "no gates".** Every gate read `ANNEX_BUILD_PROFILE` for
//! itself and returned success when it was unset. That made the whole
//! production posture opt-in through a variable that `deploy.sh`, `deploy.ps1`
//! and the operator documentation never set. Forgetting it produced no error —
//! which is exactly why it is easy to forget.
//!
//! **Two values were not enough.** A desktop install and a public server are
//! both "not development", but they need different gates. The desktop app
//! embeds a loopback server for a single human: an empty CORS origin list is
//! correct there, and the one person who registers *should* become founder.
//! A binary that could only be `dev` or `production` forced a choice between
//! breaking the desktop app and leaving it with no artifact-provenance checks
//! at all. So there are three profiles, and the gates are grouped by what they
//! are actually protecting:
//!
//! * [`BuildProfile::requires_artifact_provenance`] — Desktop and Production.
//!   "The bytes I load must be the bytes that were signed off." ZK verification
//!   keys, the VRP embedding model, signing-key strength. A desktop bundle
//!   needs these every bit as much as a server does.
//! * [`BuildProfile::requires_multi_tenant_gates`] — Production only.
//!   "Strangers can reach this." CORS origins, founder bootstrap, clustered
//!   rate limiting, ZK enforcement.
//!
//! And the default now comes from the BINARY rather than the environment: a
//! `--release` build is [`BuildProfile::Production`], a debug build is
//! [`BuildProfile::Dev`]. Release binaries are what operators deploy; debug
//! binaries are what the e2e harness, the smoke scripts and `cargo test` run.
//! `ANNEX_BUILD_PROFILE` still wins when set — `docker-compose.yml` runs a
//! release image as `dev` on purpose, and `annex-desktop` sets `desktop`
//! explicitly — but turning a gate off is now a decision someone made and a
//! line in the boot log, rather than the consequence of forgetting something.

use std::fmt;

/// The posture a running server is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildProfile {
    /// Local development, CI and test. Gates relax; nothing here is shipped.
    Dev,
    /// The embedded server inside `annex-desktop`. Single user, loopback,
    /// but shipped — so it verifies what it loads without taking on the gates
    /// that only make sense for a server strangers can reach.
    Desktop,
    /// Anything an operator deploys for other people. Every gate is live.
    Production,
}

impl BuildProfile {
    /// "The bytes I load must be the bytes that were signed off."
    ///
    /// ZK verification keys, the VRP embedding model, signing-key strength.
    /// True for anything shipped to a user, desktop included.
    pub fn requires_artifact_provenance(self) -> bool {
        matches!(self, BuildProfile::Desktop | BuildProfile::Production)
    }

    /// "Strangers can reach this."
    ///
    /// CORS origins, founder bootstrap, clustered rate limiting, ZK
    /// enforcement. Deliberately NOT true for `Desktop`: requiring an explicit
    /// CORS origin list or a founder bootstrap token from someone who just
    /// installed a desktop app would break first run for no security gain on a
    /// loopback listener with one user.
    pub fn requires_multi_tenant_gates(self) -> bool {
        matches!(self, BuildProfile::Production)
    }

    /// The name the environment variable would use for this profile.
    pub fn as_str(self) -> &'static str {
        match self {
            BuildProfile::Dev => "dev",
            BuildProfile::Desktop => "desktop",
            BuildProfile::Production => "production",
        }
    }
}

impl fmt::Display for BuildProfile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What this binary defaults to with no environment variable set.
///
/// `debug_assertions` rather than a Cargo feature so there is no flag to
/// remember to pass at release time: a release build is production by
/// construction.
pub const fn compiled_default() -> BuildProfile {
    if cfg!(debug_assertions) {
        BuildProfile::Dev
    } else {
        BuildProfile::Production
    }
}

/// How the profile was arrived at — used to decide whether to warn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileSource {
    /// No usable environment variable; the binary's own default applied.
    CompiledDefault,
    /// `ANNEX_BUILD_PROFILE` named this profile.
    Environment,
    /// `ANNEX_BUILD_PROFILE` held something unrecognised, which is ignored.
    ///
    /// Deliberately not a hard error. An unparseable value on a release binary
    /// falls back to production, which is the safe direction: erroring turns a
    /// typo into an outage, and treating it as `dev` turns a typo into an open
    /// server.
    Unrecognised,
}

/// Resolve the profile from a raw environment value.
///
/// Pure over its input so the precedence table is testable without mutating
/// process-global environment state — the same reason
/// [`crate::http::cors::dev_localhost_enabled`] is written this way.
pub fn resolve(raw: Option<&str>) -> (BuildProfile, ProfileSource) {
    let trimmed = raw.map(|v| v.trim().to_ascii_lowercase());
    match trimmed.as_deref() {
        None | Some("") => (compiled_default(), ProfileSource::CompiledDefault),
        Some("production") | Some("release") => {
            (BuildProfile::Production, ProfileSource::Environment)
        }
        Some("desktop") => (BuildProfile::Desktop, ProfileSource::Environment),
        Some("dev") | Some("development") => (BuildProfile::Dev, ProfileSource::Environment),
        Some(_) => (compiled_default(), ProfileSource::Unrecognised),
    }
}

/// The profile this process is running under.
pub fn current() -> BuildProfile {
    resolve(std::env::var("ANNEX_BUILD_PROFILE").ok().as_deref()).0
}

/// True when the gates that protect other people's traffic should be live.
pub fn requires_multi_tenant_gates() -> bool {
    current().requires_multi_tenant_gates()
}

/// True when shipped artifacts must be verified against their pins.
pub fn requires_artifact_provenance() -> bool {
    current().requires_artifact_provenance()
}

/// Log how the profile was decided, once, at startup.
///
/// The downgrade warning is the reason this exists. A release binary told to
/// run as `dev` has just had its CORS, ZK-enforcement and founder gates turned
/// off, and the boot log is the only place that will ever be visible.
pub fn log_resolution() {
    let raw = std::env::var("ANNEX_BUILD_PROFILE").ok();
    let (profile, source) = resolve(raw.as_deref());
    match source {
        ProfileSource::Unrecognised => {
            tracing::warn!(
                value = ?raw,
                profile = %profile,
                "unrecognised ANNEX_BUILD_PROFILE (expected \"production\", \"desktop\" or \
                 \"dev\"); using this binary's compiled default"
            );
        }
        ProfileSource::CompiledDefault => {
            tracing::info!(
                profile = %profile,
                "build profile from the compiled default (ANNEX_BUILD_PROFILE unset)"
            );
        }
        ProfileSource::Environment => {
            if profile == BuildProfile::Dev && compiled_default().is_shipped() {
                tracing::warn!(
                    "ANNEX_BUILD_PROFILE=dev on a release binary: the production gates are \
                     DISABLED. Wildcard CORS is permitted, ZK enforcement may be switched off, \
                     and the first identity to register becomes founder. Correct for a local \
                     container; wrong for anything reachable off-host."
                );
            } else {
                tracing::info!(profile = %profile, "build profile from ANNEX_BUILD_PROFILE");
            }
        }
    }
}

impl BuildProfile {
    /// Anything that is not local development.
    fn is_shipped(self) -> bool {
        !matches!(self, BuildProfile::Dev)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unset_falls_back_to_the_compiled_default() {
        for raw in [None, Some(""), Some("   ")] {
            assert_eq!(
                resolve(raw),
                (compiled_default(), ProfileSource::CompiledDefault),
                "{raw:?} should fall back to the compiled default"
            );
        }
    }

    #[test]
    fn explicit_values_win() {
        for v in ["production", "PRODUCTION", " release ", "Release"] {
            assert_eq!(
                resolve(Some(v)),
                (BuildProfile::Production, ProfileSource::Environment),
                "{v:?} should resolve to production"
            );
        }
        for v in ["desktop", "DESKTOP", " Desktop "] {
            assert_eq!(
                resolve(Some(v)),
                (BuildProfile::Desktop, ProfileSource::Environment),
                "{v:?} should resolve to desktop"
            );
        }
        for v in ["dev", "DEV", "development"] {
            assert_eq!(
                resolve(Some(v)),
                (BuildProfile::Dev, ProfileSource::Environment),
                "{v:?} should resolve to dev"
            );
        }
    }

    /// A typo must never be the thing that opens a server.
    ///
    /// `ANNEX_BUILD_PROFILE=produktion` used to read as "not production" to
    /// every gate — each compared against the exact strings and returned
    /// `Ok(())` otherwise. On a release binary the compiled default catches it.
    #[test]
    fn a_typo_falls_back_to_the_compiled_default_not_to_dev() {
        let (profile, source) = resolve(Some("produktion"));
        assert_eq!(source, ProfileSource::Unrecognised);
        assert_eq!(profile, compiled_default());
    }

    #[test]
    fn debug_builds_default_to_dev_and_release_builds_to_production() {
        // Whichever half runs, it pins the mapping for that build kind.
        if cfg!(debug_assertions) {
            assert_eq!(compiled_default(), BuildProfile::Dev);
        } else {
            assert_eq!(compiled_default(), BuildProfile::Production);
        }
    }

    /// The grouping is the whole point of having three profiles rather than
    /// two, so it is pinned rather than left to the reader.
    #[test]
    fn desktop_verifies_what_it_loads_without_taking_the_multi_tenant_gates() {
        assert!(BuildProfile::Desktop.requires_artifact_provenance());
        assert!(!BuildProfile::Desktop.requires_multi_tenant_gates());

        assert!(BuildProfile::Production.requires_artifact_provenance());
        assert!(BuildProfile::Production.requires_multi_tenant_gates());

        assert!(!BuildProfile::Dev.requires_artifact_provenance());
        assert!(!BuildProfile::Dev.requires_multi_tenant_gates());
    }
}
