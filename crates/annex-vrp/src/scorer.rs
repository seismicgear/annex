//! The alignment scorer this process is using.
//!
//! [`crate::compare_peer_anchor_scored`] decides Aligned / Partial / Conflict
//! for every agent registration and every federation handshake, and it used to
//! construct a [`crate::semantic::ConceptEmbedder`] inline — so the scorer was
//! not a deployment choice, it was a hard-coded one, and
//! [`crate::embedding::StaticEmbedder`] (written, tested, 7.5 MB of pinned
//! weights on disk) was called by nothing.
//!
//! Installed once, at startup, by whoever knows where the model lives and what
//! the build profile demands. Install-once rather than a mutable handle
//! because the score is a trust decision: a scorer that can change under a
//! running server means two handshakes minutes apart are not comparable, and
//! nothing would say so.
//!
//! The default, before anything is installed, is the lexicon. That is the
//! honest fallback for a unit test or a dev binary that never called
//! [`install`] — not an error, because plenty of code in this workspace
//! compares anchors without being a server.

use std::sync::{Arc, OnceLock};

use crate::embedding::ModelFingerprint;
use crate::semantic::{ConceptEmbedder, SemanticEmbedder};

/// A scorer that can be shared across threads and handed to
/// [`crate::semantic::calculate_semantic_alignment`].
pub type SharedScorer = Arc<dyn SemanticEmbedder + Send + Sync>;

static ACTIVE: OnceLock<SharedScorer> = OnceLock::new();

/// Returned when [`install`] is called twice.
///
/// An error rather than a silent no-op: the second caller believes it chose the
/// scorer, and letting it believe that is how a deployment ends up scoring with
/// something other than what its logs say.
#[derive(Debug, thiserror::Error)]
#[error(
    "an alignment scorer is already installed ({installed}); a second install would mean \
     this process scored some handshakes with one model and some with another"
)]
pub struct AlreadyInstalled {
    pub installed: String,
}

/// Install the scorer for this process. Call once, at startup.
pub fn install(scorer: SharedScorer) -> Result<(), AlreadyInstalled> {
    let id = scorer.fingerprint().model_id.clone();
    ACTIVE.set(scorer).map_err(|_| AlreadyInstalled {
        installed: ACTIVE
            .get()
            .map(|s| s.fingerprint().model_id.clone())
            .unwrap_or_else(|| id.clone()),
    })
}

/// The scorer in force, falling back to the lexicon.
pub fn active() -> SharedScorer {
    ACTIVE
        .get_or_init(|| Arc::new(ConceptEmbedder::new()) as SharedScorer)
        .clone()
}

/// What this process would tell a peer it is scoring with.
pub fn active_fingerprint() -> ModelFingerprint {
    active().fingerprint()
}

/// True once [`install`] has been called — i.e. the scorer is a deliberate
/// choice rather than the fallback.
///
/// Distinct from "the fingerprint is not the lexicon's", because a deployment
/// could deliberately install the lexicon.
pub fn is_installed() -> bool {
    ACTIVE.get().is_some()
}
