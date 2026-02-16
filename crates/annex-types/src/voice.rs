//! Voice profile and model definitions.
//!
//! This module defines the types for configuring voice generation for agents.
//! A `VoiceProfile` maps a logical ID to a specific TTS model and its parameters.

use serde::{Deserialize, Serialize};

/// Supported TTS model architectures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VoiceModel {
    /// Piper TTS (ONNX-based, fast, local).
    #[default]
    Piper,
    /// Bark (Transformer-based, high quality, slow).
    Bark,
    /// System TTS (OS-provided).
    System,
}

/// A voice profile configuration.
///
/// Defines how an agent's voice sounds.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VoiceProfile {
    /// Unique identifier for the voice profile.
    pub id: String,
    /// Human-readable name.
    pub name: String,
    /// The underlying TTS model architecture.
    pub model: VoiceModel,
    /// Path to the model file (relative to `assets/voices/` or absolute).
    pub model_path: String,
    /// Path to the model configuration file (if applicable).
    pub config_path: Option<String>,
    /// Speech speed multiplier (1.0 is normal).
    pub speed: f32,
    /// Pitch shift factor (1.0 is normal).
    pub pitch: f32,
    /// Speaker ID within a multi-speaker model (0-indexed).
    pub speaker_id: Option<u32>,
}

impl Default for VoiceProfile {
    fn default() -> Self {
        Self {
            id: "default".to_string(),
            name: "Default Voice".to_string(),
            model: VoiceModel::Piper,
            model_path: "en_US-lessac-medium.onnx".to_string(),
            config_path: Some("en_US-lessac-medium.onnx.json".to_string()),
            speed: 1.0,
            pitch: 1.0,
            speaker_id: None,
        }
    }
}
