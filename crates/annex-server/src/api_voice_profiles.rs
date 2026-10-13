//! Operator-only catalog of configured speech profiles. Readiness is derived
//! from the actual loaded profile and backend; filesystem paths are private.
use crate::{api::ApiError, middleware::IdentityContext, AppState};
use annex_types::voice::{VoiceModel, VoiceProfile};
use axum::{Extension, Json};
use futures_util::{stream, StreamExt};
use rusqlite::Connection;
use serde::Serialize;
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq)]
struct Configured {
    id: String,
    name: String,
    model: String,
    model_path: String,
    config_path: Option<String>,
    speed: f32,
    pitch: f32,
    speaker_id: Option<u32>,
}
impl Configured {
    fn profile(&self) -> Option<VoiceProfile> {
        Some(VoiceProfile {
            id: self.id.clone(),
            name: self.name.clone(),
            model: serde_json::from_value(serde_json::json!(self.model)).ok()?,
            model_path: self.model_path.clone(),
            config_path: self.config_path.clone(),
            speed: self.speed,
            pitch: self.pitch,
            speaker_id: self.speaker_id,
        })
    }
}

#[derive(Debug, Serialize)]
pub struct ProfileAvailability {
    pub id: String,
    pub name: String,
    pub model: Option<VoiceModel>,
    pub speed: Option<f32>,
    pub pitch: Option<f32>,
    pub speaker_id: Option<u32>,
    pub available: bool,
}
#[derive(Debug, Serialize)]
pub struct ProfileCatalog {
    pub profiles: Vec<ProfileAvailability>,
}

fn failure(error: impl std::fmt::Display) -> ApiError {
    ApiError::InternalServerError(format!("voice profile catalog: {error}"))
}
fn authorize(
    conn: &Connection,
    state: &AppState,
    captured: &annex_identity::PlatformIdentity,
) -> Result<(), ApiError> {
    let current =
        annex_identity::get_platform_identity(conn, state.server_id, &captured.pseudonym_id)
            .map_err(|_| ApiError::Unauthorized("operator session is unavailable".into()))?;
    if !current.active || current.token_epoch != captured.token_epoch {
        return Err(ApiError::Unauthorized(
            "operator session is no longer active".into(),
        ));
    }
    if !current.can_moderate {
        return Err(ApiError::Forbidden(
            "only current operators can list voice profiles".into(),
        ));
    }
    Ok(())
}
fn configured(conn: &Connection, server_id: i64) -> Result<Vec<Configured>, ApiError> {
    let mut stmt = conn.prepare("SELECT profile_id,name,model,model_path,config_path,speed,pitch,speaker_id FROM voice_profiles WHERE server_id=?1 ORDER BY profile_id LIMIT 257").map_err(failure)?;
    let result = stmt
        .query_map([server_id], |r| {
            Ok(Configured {
                id: r.get(0)?,
                name: r.get(1)?,
                model: r.get(2)?,
                model_path: r.get(3)?,
                config_path: r.get(4)?,
                speed: r.get(5)?,
                pitch: r.get(6)?,
                speaker_id: r.get(7)?,
            })
        })
        .map_err(failure)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(failure)?;
    if result.len() > 256 {
        return Err(ApiError::ServiceUnavailable(
            "voice profile catalog exceeds the supported 256 configured profiles".into(),
        ));
    }
    Ok(result)
}

pub async fn list_voice_profiles_handler(
    Extension(state): Extension<Arc<AppState>>,
    Extension(IdentityContext(identity)): Extension<IdentityContext>,
) -> Result<Json<ProfileCatalog>, ApiError> {
    let snapshot = tokio::task::spawn_blocking({
        let state = state.clone();
        let identity = identity.clone();
        move || {
            let conn = state.pool.get().map_err(failure)?;
            let tx = annex_db::read_transaction(&conn).map_err(failure)?;
            authorize(&tx, &state, &identity)?;
            let rows = configured(&tx, state.server_id)?;
            tx.commit().map_err(failure)?;
            Ok::<_, ApiError>(rows)
        }
    })
    .await
    .map_err(failure)??;
    let mut profiles: Vec<_> = stream::iter(snapshot.iter().cloned().map(|row| {
        let tts = state.tts_service.clone();
        async move {
            let profile = row.profile();
            let loaded = tts.get_profile(&row.id).await;
            let available = match (profile.as_ref(), loaded.as_ref()) {
                (Some(configured), Some(loaded)) if configured == loaded => {
                    tts.validate_profile(loaded).await.is_ok()
                }
                _ => false,
            };
            ProfileAvailability {
                id: row.id,
                name: row.name,
                model: profile.map(|p| p.model),
                speed: row.speed.is_finite().then_some(row.speed),
                pitch: row.pitch.is_finite().then_some(row.pitch),
                speaker_id: row.speaker_id,
                available,
            }
        }
    }))
    .buffered(4)
    .collect()
    .await;
    if !snapshot.iter().any(|row| row.id == "default") {
        let loaded = state.tts_service.get_profile("default").await;
        let available = match &loaded {
            Some(profile) => state.tts_service.validate_profile(profile).await.is_ok(),
            None => false,
        };
        profiles.insert(
            0,
            ProfileAvailability {
                id: "default".into(),
                name: loaded
                    .as_ref()
                    .map_or_else(|| "Default".into(), |p| p.name.clone()),
                model: loaded.as_ref().map(|p| p.model),
                speed: loaded.as_ref().map(|p| p.speed),
                pitch: loaded.as_ref().map(|p| p.pitch),
                speaker_id: loaded.as_ref().and_then(|p| p.speaker_id),
                available,
            },
        );
    }
    tokio::task::spawn_blocking(move || {
        let conn = state.pool.get().map_err(failure)?;
        let tx = annex_db::read_transaction(&conn).map_err(failure)?;
        authorize(&tx, &state, &identity)?;
        if configured(&tx, state.server_id)? != snapshot {
            return Err(ApiError::Conflict(
                "voice profile configuration changed; refresh the catalog".into(),
            ));
        }
        tx.commit().map_err(failure)?;
        Ok::<_, ApiError>(())
    })
    .await
    .map_err(failure)??;
    Ok(Json(ProfileCatalog { profiles }))
}
