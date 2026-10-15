//! Resolve a profile once. Provider arguments and audio format share this snapshot.
use crate::{error::VoiceError, speech_dsp::validate_factors};
use annex_types::voice::{VoiceModel, VoiceProfile};
use serde::Deserialize;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

pub(crate) const BARK_MODEL: &str = "suno/bark-small";
pub(crate) const BARK_REVISION: &str = "1dbd7a128513b8ae4a4e2130fed57b7ac9da5bcd";

pub(crate) enum PreparedProfile {
    Piper {
        model: PathBuf,
        config: Vec<u8>,
        sample_rate: usize,
        speaker: Option<u32>,
        speed: f32,
        pitch: f32,
    },
    Bark {
        model: String,
        revision: String,
        speaker: Option<u32>,
        speed: f32,
        pitch: f32,
    },
    System {
        voice: String,
        speed: f32,
        pitch: f32,
    },
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BarkConfig {
    version: u32,
    revision: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SystemConfig {
    version: u32,
    #[serde(default)]
    speakers: BTreeMap<u32, String>,
}
fn config_error(message: impl Into<String>) -> VoiceError {
    VoiceError::Config(message.into())
}
fn resolve_path(directory: &Path, path: &str) -> PathBuf {
    if Path::new(path).is_absolute() {
        PathBuf::from(path)
    } else {
        directory.join(path)
    }
}
async fn read_config(path: &Path) -> Result<Vec<u8>, VoiceError> {
    let metadata = tokio::fs::metadata(path)
        .await
        .map_err(|e| config_error(format!("Cannot read voice config {}: {e}", path.display())))?;
    if !metadata.is_file() || metadata.len() > 1024 * 1024 {
        return Err(config_error(
            "Voice config must be a file no larger than 1 MiB",
        ));
    }
    let bytes = tokio::fs::read(path)
        .await
        .map_err(|e| config_error(format!("Cannot read voice config: {e}")))?;
    if bytes.len() > 1024 * 1024 {
        return Err(config_error("Voice config exceeded its size limit"));
    }
    Ok(bytes)
}
pub(crate) fn valid_system_voice(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'/' | b'+'))
        && !value.starts_with(['-', '/'])
        && !value.contains("..")
        && value.matches('+').count() <= 1
}
pub(crate) fn system_voice_exists(voice: &str, voices: &str, variants: &str) -> bool {
    let (base, variant) = voice
        .split_once('+')
        .map_or((voice, None), |(a, b)| (a, Some(b)));
    let has = |list: &str, value: &str, is_variant: bool| {
        list.lines().skip(1).any(|line| {
            let fields: Vec<_> = line.split_whitespace().collect();
            fields.len() >= 5
                && if is_variant {
                    fields[4].strip_prefix("!v/") == Some(value)
                } else {
                    [fields[1], fields[3], fields[4]]
                        .iter()
                        .any(|field| field.eq_ignore_ascii_case(value))
                }
        })
    };
    has(voices, base, false) && variant.is_none_or(|variant| has(variants, variant, true))
}

pub(crate) async fn prepare(
    profile: &VoiceProfile,
    directory: &Path,
) -> Result<PreparedProfile, VoiceError> {
    validate_factors(profile.speed, profile.pitch)?;
    match profile.model {
        VoiceModel::Piper => {
            if profile.model_path.is_empty() {
                return Err(config_error("Piper requires a model path"));
            }
            let model = resolve_path(directory, &profile.model_path);
            if !tokio::fs::metadata(&model)
                .await
                .is_ok_and(|m| m.is_file() && m.len() > 0)
            {
                return Err(config_error("Piper model is not an existing nonempty file"));
            }
            let config_path = profile
                .config_path
                .clone()
                .unwrap_or_else(|| format!("{}.json", profile.model_path));
            let config = read_config(&resolve_path(directory, &config_path)).await?;
            let parsed: serde_json::Value = serde_json::from_slice(&config)
                .map_err(|e| config_error(format!("Invalid Piper config: {e}")))?;
            let sample_rate = parsed["audio"]["sample_rate"]
                .as_u64()
                .filter(|n| (8_000..=192_000).contains(n))
                .ok_or_else(|| config_error("Piper config must declare audio.sample_rate"))?
                as usize;
            if let Some(speaker) = profile.speaker_id {
                let count = parsed["num_speakers"]
                    .as_u64()
                    .filter(|n| *n > 0)
                    .ok_or_else(|| {
                        config_error("An explicit Piper speaker requires config.num_speakers")
                    })?;
                if u64::from(speaker) >= count {
                    return Err(config_error(
                        "Piper speaker ID is outside this model's speaker range",
                    ));
                }
            }
            Ok(PreparedProfile::Piper {
                model,
                config,
                sample_rate,
                speaker: profile.speaker_id,
                speed: profile.speed,
                pitch: profile.pitch,
            })
        }
        VoiceModel::Bark => {
            let model = if profile.model_path.is_empty() {
                BARK_MODEL.into()
            } else {
                profile.model_path.clone()
            };
            let parts: Vec<_> = model.split('/').collect();
            if parts.len() != 2
                || parts.iter().any(|p| {
                    p.is_empty()
                        || p.starts_with('.')
                        || !p
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
                })
            {
                return Err(config_error(
                    "Bark model_path must be a model repository ID such as suno/bark-small",
                ));
            }
            let revision = if let Some(path) = &profile.config_path {
                let config: BarkConfig =
                    serde_json::from_slice(&read_config(&resolve_path(directory, path)).await?)
                        .map_err(|e| config_error(format!("Invalid Bark config: {e}")))?;
                if config.version != 1 {
                    return Err(config_error("Unsupported Bark config version"));
                }
                config.revision
            } else if model == BARK_MODEL {
                BARK_REVISION.into()
            } else {
                return Err(config_error(
                    "A nondefault Bark model requires an explicit pinned config revision",
                ));
            };
            if revision.len() != 40
                || !revision
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            {
                return Err(config_error(
                    "Bark revision must be an immutable lowercase 40-character commit",
                ));
            }
            if profile.speaker_id.is_some_and(|speaker| speaker > 9) {
                return Err(config_error(
                    "Bark English v2 speaker ID must be between 0 and 9",
                ));
            }
            Ok(PreparedProfile::Bark {
                model,
                revision,
                speaker: profile.speaker_id,
                speed: profile.speed,
                pitch: profile.pitch,
            })
        }
        VoiceModel::System => {
            let mut voice = if profile.model_path.is_empty() {
                "en-us".into()
            } else {
                profile.model_path.clone()
            };
            let config = if let Some(path) = &profile.config_path {
                let config: SystemConfig =
                    serde_json::from_slice(&read_config(&resolve_path(directory, path)).await?)
                        .map_err(|e| config_error(format!("Invalid System config: {e}")))?;
                if config.version != 1 || config.speakers.values().any(|v| !valid_system_voice(v)) {
                    return Err(config_error("Invalid System voice configuration"));
                }
                Some(config)
            } else {
                None
            };
            if let Some(speaker) = profile.speaker_id {
                voice = config
                    .and_then(|c| c.speakers.get(&speaker).cloned())
                    .ok_or_else(|| {
                        config_error(
                            "System speaker ID requires an explicit config.speakers mapping",
                        )
                    })?;
            }
            if !valid_system_voice(&voice) {
                return Err(config_error(
                    "System model_path must name a supported espeak-ng voice",
                ));
            }
            Ok(PreparedProfile::System {
                voice,
                speed: profile.speed,
                pitch: profile.pitch,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn profile(model: VoiceModel) -> VoiceProfile {
        VoiceProfile {
            model,
            model_path: String::new(),
            config_path: None,
            ..VoiceProfile::default()
        }
    }
    #[tokio::test]
    async fn bark_forwards_default_or_explicit_immutable_model_revision_and_speaker() {
        let dir = tempfile::tempdir().unwrap();
        let mut p = profile(VoiceModel::Bark);
        p.speaker_id = Some(3);
        let PreparedProfile::Bark {
            model,
            revision,
            speaker,
            ..
        } = prepare(&p, dir.path()).await.unwrap()
        else {
            panic!()
        };
        assert_eq!(model, BARK_MODEL);
        assert_eq!(revision, BARK_REVISION);
        assert_eq!(speaker, Some(3));
        p.model_path = "suno/bark".into();
        assert!(prepare(&p, dir.path()).await.is_err());
        p.config_path = Some("bark.json".into());
        std::fs::write(
            dir.path().join("bark.json"),
            r#"{"version":1,"revision":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}"#,
        )
        .unwrap();
        assert!(prepare(&p, dir.path()).await.is_ok());
        p.speaker_id = Some(10);
        assert!(prepare(&p, dir.path()).await.is_err());
    }
    #[tokio::test]
    async fn system_speaker_is_explicit_and_config_cannot_be_silently_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let mut p = profile(VoiceModel::System);
        p.speaker_id = Some(2);
        assert!(prepare(&p, dir.path()).await.is_err());
        p.config_path = Some("system.json".into());
        std::fs::write(
            dir.path().join("system.json"),
            r#"{"version":1,"speakers":{"2":"en-us+m2"}}"#,
        )
        .unwrap();
        let PreparedProfile::System { voice, .. } = prepare(&p, dir.path()).await.unwrap() else {
            panic!()
        };
        assert_eq!(voice, "en-us+m2");
        std::fs::write(
            dir.path().join("system.json"),
            r#"{"version":1,"ignored_field":true}"#,
        )
        .unwrap();
        assert!(prepare(&p, dir.path()).await.is_err());
        assert!(!valid_system_voice("--stdout"));
    }
    #[tokio::test]
    async fn piper_speaker_and_audio_format_are_validated_from_one_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let p = VoiceProfile {
            speaker_id: Some(1),
            ..VoiceProfile::default()
        };
        std::fs::write(dir.path().join(&p.model_path), b"model contract fixture").unwrap();
        let config = dir.path().join(p.config_path.as_ref().unwrap());
        std::fs::write(
            &config,
            r#"{"audio":{"sample_rate":24000},"num_speakers":2}"#,
        )
        .unwrap();
        let prepared = prepare(&p, dir.path()).await.unwrap();
        std::fs::write(
            &config,
            r#"{"audio":{"sample_rate":48000},"num_speakers":1}"#,
        )
        .unwrap();
        let PreparedProfile::Piper {
            sample_rate,
            config,
            ..
        } = prepared
        else {
            panic!()
        };
        assert_eq!(sample_rate, 24000);
        assert!(std::str::from_utf8(&config).unwrap().contains("24000"));
        assert!(prepare(&p, dir.path()).await.is_err());
    }
    #[test]
    fn system_selection_must_exist_in_native_voice_and_variant_catalogs() {
        let voices =
            "Pty Language Age/Gender VoiceName File\n 2 en-us --/M English_(America) gmw/en-US";
        let variants = "Pty Language Age/Gender VoiceName File\n 5 variant --/M male2 !v/m2";
        assert!(system_voice_exists("en-us+m2", voices, variants));
        assert!(!system_voice_exists("en-us+m9", voices, variants));
        assert!(!system_voice_exists("unknown", voices, variants));
    }
}
