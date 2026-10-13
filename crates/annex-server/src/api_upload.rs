//! Upload API handlers for images, videos, and files with automatic
//! metadata stripping and MIME type verification.
//!
//! Supports server image uploads (admin) and chat uploads (members).
//! All uploaded images have EXIF and other metadata stripped for privacy.
//! Video and file types are verified via magic-byte detection to prevent
//! type spoofing (e.g. zip bombs disguised as images).

use crate::{api::ApiError, middleware::IdentityContext, AppState};
use annex_observe::EventPayload;
use axum::{
    extract::{Extension, Multipart, Path},
    response::{IntoResponse, Response},
    Json as AxumJson,
};
use std::sync::Arc;
use uuid::Uuid;

// ── Upload Categories ──

/// Categorizes an upload by its detected content type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UploadCategory {
    Image,
    Video,
    File,
}

impl UploadCategory {
    /// Returns the string label for database storage.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Image => "image",
            Self::Video => "video",
            Self::File => "file",
        }
    }
}

// ── Allowed Content Types ──

/// Allowed image MIME types.
pub(crate) const ALLOWED_IMAGE_TYPES: &[&str] =
    &["image/jpeg", "image/png", "image/gif", "image/webp"];

/// Allowed video MIME types.
pub(crate) const ALLOWED_VIDEO_TYPES: &[&str] = &["video/mp4", "video/webm", "video/quicktime"];

/// Blocked MIME types that are never allowed (executables, etc.).
pub(crate) const BLOCKED_TYPES: &[&str] = &[
    "application/x-executable",
    "application/x-dosexec",
    "application/x-sharedlib",
];

/// Maximum upload file size for server images: 10 MiB (not configurable).
const MAX_SERVER_IMAGE_SIZE: usize = 10 * 1024 * 1024;

// ── Content Type Detection ──

/// Determines file extension from content type.
pub(crate) fn ext_from_content_type(ct: &str) -> &str {
    match ct {
        "image/jpeg" => "jpg",
        "image/png" => "png",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "video/mp4" => "mp4",
        "video/webm" => "webm",
        "video/quicktime" => "mov",
        "application/pdf" => "pdf",
        "application/zip" => "zip",
        "text/plain" => "txt",
        _ => "bin",
    }
}

/// Detects content type from the first bytes of a file (magic-byte detection).
///
/// Returns `(mime_type, category)` if recognized, or `None` for unknown formats.
pub(crate) fn detect_content_type(data: &[u8]) -> Option<(&'static str, UploadCategory)> {
    // Images
    if data.len() >= 2 && data[0] == 0xFF && data[1] == 0xD8 {
        return Some(("image/jpeg", UploadCategory::Image));
    }
    if data.len() >= 8 && data[..8] == [137, 80, 78, 71, 13, 10, 26, 10] {
        return Some(("image/png", UploadCategory::Image));
    }
    if data.len() >= 4 && &data[..4] == b"GIF8" {
        return Some(("image/gif", UploadCategory::Image));
    }
    if data.len() >= 12 && &data[..4] == b"RIFF" && &data[8..12] == b"WEBP" {
        return Some(("image/webp", UploadCategory::Image));
    }

    // Videos — MP4/MOV (ISO Base Media File Format: ftyp box)
    if data.len() >= 12 && &data[4..8] == b"ftyp" {
        // Check for MOV-specific brands
        let brand = &data[8..12];
        if brand == b"qt  " {
            return Some(("video/quicktime", UploadCategory::Video));
        }
        // All other ftyp-based formats are MP4
        return Some(("video/mp4", UploadCategory::Video));
    }

    // Videos — WebM/MKV (EBML header)
    if data.len() >= 4 && data[..4] == [0x1A, 0x45, 0xDF, 0xA3] {
        return Some(("video/webm", UploadCategory::Video));
    }

    // Files — PDF
    if data.len() >= 4 && &data[..4] == b"%PDF" {
        return Some(("application/pdf", UploadCategory::File));
    }

    // Files — ZIP (also covers docx, xlsx, etc.)
    if data.len() >= 4 && data[..4] == [0x50, 0x4B, 0x03, 0x04] {
        return Some(("application/zip", UploadCategory::File));
    }

    // Files — ELF executable (blocked)
    if data.len() >= 4 && data[..4] == [0x7F, 0x45, 0x4C, 0x46] {
        return Some(("application/x-executable", UploadCategory::File));
    }

    // Files — PE/DOS executable (blocked)
    if data.len() >= 2 && data[..2] == [0x4D, 0x5A] {
        return Some(("application/x-dosexec", UploadCategory::File));
    }

    None
}

/// Returns the maximum upload size in bytes for a given category based on server policy.
#[cfg(test)]
fn max_size_for_category(policy: &annex_types::ServerPolicy, category: UploadCategory) -> usize {
    let mb = match category {
        UploadCategory::Image => policy.max_image_size_mb,
        UploadCategory::Video => policy.max_video_size_mb,
        UploadCategory::File => policy.max_file_size_mb,
    };
    (mb as usize).saturating_mul(1024 * 1024)
}

/// Checks whether a given upload category is enabled in the server policy.
#[cfg(test)]
fn is_category_enabled(policy: &annex_types::ServerPolicy, category: UploadCategory) -> bool {
    match category {
        UploadCategory::Image => policy.images_enabled,
        UploadCategory::Video => policy.videos_enabled,
        UploadCategory::File => policy.files_enabled,
    }
}

// ── EXIF / Metadata Stripping ──

mod image_privacy;

/// Remove descriptive/private image metadata without re-encoding pixels or
/// flattening animation. Required color profiles and presentation controls stay.
/// Malformed containers are refused before any file or upload row is persisted.
pub(crate) fn strip_metadata(data: &[u8], content_type: &str) -> Result<Vec<u8>, ApiError> {
    let result = match content_type {
        "image/jpeg" => image_privacy::jpeg(data),
        "image/png" => image_privacy::png(data),
        "image/gif" => image_privacy::gif(data),
        "image/webp" => image_privacy::webp(data),
        _ => Ok(data.to_vec()),
    };
    result.map_err(|error| ApiError::BadRequest(error.into()))
}

/// Reads a multipart field in chunks, enforcing a byte limit during
/// streaming. Returns `Err` as soon as the limit is exceeded, preventing
/// oversized payloads from being fully buffered into RAM.
pub(crate) async fn read_field_capped(
    field: &mut axum::extract::multipart::Field<'_>,
    max_bytes: usize,
) -> Result<Vec<u8>, ApiError> {
    let mut buf = Vec::new();
    while let Some(chunk) = field
        .chunk()
        .await
        .map_err(|e| ApiError::BadRequest(format!("failed to read upload chunk: {e}")))?
    {
        if buf.len() + chunk.len() > max_bytes {
            return Err(ApiError::BadRequest(format!(
                "upload exceeds maximum size of {max_bytes} bytes",
            )));
        }
        buf.extend_from_slice(&chunk);
    }
    Ok(buf)
}

// ── Upload Handlers ──

/// Handler for `POST /api/admin/server/image`.
///
/// Uploads a server icon/branding image. Requires `can_moderate` permission.
pub async fn upload_server_image_handler(
    Extension(state): Extension<Arc<AppState>>,
    Extension(IdentityContext(identity)): Extension<IdentityContext>,
    mut multipart: Multipart,
) -> Result<Response, ApiError> {
    let current = state.clone();
    let actor = identity.clone();
    tokio::task::spawn_blocking(move || {
        let conn = current
            .pool
            .get()
            .map_err(|error| ApiError::InternalServerError(error.to_string()))?;
        crate::services::upload_service::authorize_server_image(
            &conn,
            &current,
            &actor.pseudonym_id,
            actor.token_epoch,
        )
        .map_err(ApiError::from)
    })
    .await
    .map_err(|error| ApiError::InternalServerError(error.to_string()))??;
    let mut field = multipart
        .next_field()
        .await
        .map_err(|error| ApiError::BadRequest(error.to_string()))?
        .ok_or_else(|| ApiError::BadRequest("no file provided".into()))?;
    if field.name() != Some("file") {
        return Err(ApiError::BadRequest("expected one image file field".into()));
    }
    let original_filename = field.file_name().unwrap_or("image").to_string();
    if original_filename.len() > 512 || original_filename.contains('\0') {
        return Err(ApiError::BadRequest("invalid filename".into()));
    }
    let data = read_field_capped(&mut field, MAX_SERVER_IMAGE_SIZE).await?;
    drop(field);
    if multipart
        .next_field()
        .await
        .map_err(|error| ApiError::BadRequest(error.to_string()))?
        .is_some()
    {
        return Err(ApiError::BadRequest(
            "only one server image is supported".into(),
        ));
    }
    let (detected_ct, category) = detect_content_type(&data)
        .ok_or_else(|| ApiError::BadRequest("unsupported file format".into()))?;
    if category != UploadCategory::Image || !ALLOWED_IMAGE_TYPES.contains(&detected_ct) {
        return Err(ApiError::BadRequest(
            "server image must be JPEG, PNG, GIF, or WebP".into(),
        ));
    }
    let cleaned = strip_metadata(&data, detected_ct)?;
    let upload_id = Uuid::new_v4().to_string();
    let metadata = crate::services::upload_service::UploadMetadata {
        upload_id: upload_id.clone(),
        channel_id: String::new(),
        uploader: identity.pseudonym_id.clone(),
        token_epoch: identity.token_epoch,
        category: "image".into(),
        original_filename,
        content_type: detected_ct.into(),
        extension: ext_from_content_type(detected_ct).into(),
        encrypted_epoch: None,
    };
    let image_url = tokio::task::spawn_blocking(move || {
        let (url, _) =
            crate::services::upload_service::persist_server_image(&state, &metadata, &cleaned)?;
        if let Ok(conn) = state.pool.get() {
            let payload = EventPayload::ModerationAction {
                moderator_pseudonym: metadata.uploader.clone(),
                action_type: "server_image_upload".into(),
                target_pseudonym: None,
                description: "Server image updated".into(),
            };
            crate::emit_and_broadcast(
                &conn,
                state.server_id,
                &metadata.uploader,
                &payload,
                &state.observe_tx,
                &state.signing_key,
            );
        }
        Ok::<_, ApiError>(url)
    })
    .await
    .map_err(|error| ApiError::InternalServerError(error.to_string()))??;
    Ok(
        AxumJson(serde_json::json!({"status":"ok","upload_id":upload_id,"url":image_url}))
            .into_response(),
    )
}

/// Handler for `POST /api/channels/{channelId}/upload`.
///
/// Uploads an image, video, or file to a channel. Requires channel membership.
/// Automatically strips EXIF and other metadata from images for privacy.
/// Enforces per-category size limits and enabled/disabled toggles from server policy.
pub async fn upload_chat_handler(
    state: Extension<Arc<AppState>>,
    identity: Extension<IdentityContext>,
    channel: Path<String>,
    multipart: Multipart,
) -> Result<Response, ApiError> {
    crate::api_upload_lifecycle::upload_chat_handler(state, identity, channel, multipart).await
}

/// Handler for `GET /api/admin/server/image`.
///
/// Returns the current server image URL.
pub async fn get_server_image_handler(
    Extension(state): Extension<Arc<AppState>>,
) -> Result<Response, ApiError> {
    let state_clone = state.clone();

    let image_url = tokio::task::spawn_blocking(move || {
        let conn = state_clone
            .pool
            .get()
            .map_err(|e| ApiError::InternalServerError(format!("db connection failed: {e}")))?;

        let url: Option<String> = conn
            .query_row(
                "SELECT image_url FROM servers WHERE id = ?1",
                rusqlite::params![state_clone.server_id],
                |row| row.get(0),
            )
            .map_err(|e| ApiError::InternalServerError(format!("failed to query server: {e}")))?;

        Ok::<_, ApiError>(url)
    })
    .await
    .map_err(|e| ApiError::InternalServerError(format!("task join error: {e}")))??;

    Ok(AxumJson(serde_json::json!({
        "image_url": image_url,
    }))
    .into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_metadata_errors_are_typed_before_persistence() {
        for (mime, data) in [
            ("image/jpeg", b"\xff\xd8".as_slice()),
            ("image/png", b"\x89PNG\r\n\x1a\n".as_slice()),
            ("image/gif", b"GIF89a".as_slice()),
            ("image/webp", b"RIFF\x04\0\0\0WEBP".as_slice()),
        ] {
            assert!(
                matches!(strip_metadata(data, mime), Err(ApiError::BadRequest(_))),
                "{mime}"
            );
        }
        assert_eq!(
            strip_metadata(b"ordinary attachment", "application/octet-stream").unwrap(),
            b"ordinary attachment"
        );
    }

    #[test]
    fn detect_jpeg() {
        let (ct, cat) = detect_content_type(&[0xFF, 0xD8, 0xFF]).unwrap();
        assert_eq!(ct, "image/jpeg");
        assert_eq!(cat, UploadCategory::Image);
    }

    #[test]
    fn detect_png() {
        let (ct, cat) = detect_content_type(&[137, 80, 78, 71, 13, 10, 26, 10]).unwrap();
        assert_eq!(ct, "image/png");
        assert_eq!(cat, UploadCategory::Image);
    }

    #[test]
    fn detect_gif() {
        let (ct, cat) = detect_content_type(b"GIF89a__").unwrap();
        assert_eq!(ct, "image/gif");
        assert_eq!(cat, UploadCategory::Image);
    }

    #[test]
    fn detect_mp4() {
        // ftyp box: size(4) + "ftyp" + brand
        let mut data = vec![0x00, 0x00, 0x00, 0x14]; // size
        data.extend_from_slice(b"ftyp");
        data.extend_from_slice(b"isom"); // brand
        let (ct, cat) = detect_content_type(&data).unwrap();
        assert_eq!(ct, "video/mp4");
        assert_eq!(cat, UploadCategory::Video);
    }

    #[test]
    fn detect_mov() {
        let mut data = vec![0x00, 0x00, 0x00, 0x14];
        data.extend_from_slice(b"ftyp");
        data.extend_from_slice(b"qt  ");
        let (ct, cat) = detect_content_type(&data).unwrap();
        assert_eq!(ct, "video/quicktime");
        assert_eq!(cat, UploadCategory::Video);
    }

    #[test]
    fn detect_webm() {
        let (ct, cat) = detect_content_type(&[0x1A, 0x45, 0xDF, 0xA3]).unwrap();
        assert_eq!(ct, "video/webm");
        assert_eq!(cat, UploadCategory::Video);
    }

    #[test]
    fn detect_pdf() {
        let (ct, cat) = detect_content_type(b"%PDF-1.4").unwrap();
        assert_eq!(ct, "application/pdf");
        assert_eq!(cat, UploadCategory::File);
    }

    #[test]
    fn detect_zip() {
        let (ct, cat) = detect_content_type(&[0x50, 0x4B, 0x03, 0x04]).unwrap();
        assert_eq!(ct, "application/zip");
        assert_eq!(cat, UploadCategory::File);
    }

    #[test]
    fn detect_elf_executable() {
        let (ct, cat) = detect_content_type(&[0x7F, 0x45, 0x4C, 0x46]).unwrap();
        assert_eq!(ct, "application/x-executable");
        assert_eq!(cat, UploadCategory::File);
    }

    #[test]
    fn detect_pe_executable() {
        let (ct, cat) = detect_content_type(&[0x4D, 0x5A, 0x90, 0x00]).unwrap();
        assert_eq!(ct, "application/x-dosexec");
        assert_eq!(cat, UploadCategory::File);
    }

    #[test]
    fn detect_unknown() {
        assert_eq!(detect_content_type(&[0, 0, 0, 0]), None);
    }

    #[test]
    fn blocked_types_are_rejected() {
        assert!(BLOCKED_TYPES.contains(&"application/x-executable"));
        assert!(BLOCKED_TYPES.contains(&"application/x-dosexec"));
    }

    #[test]
    fn category_size_limits() {
        let policy = annex_types::ServerPolicy {
            max_image_size_mb: 5,
            max_video_size_mb: 10,
            max_file_size_mb: 3,
            ..Default::default()
        };
        assert_eq!(
            max_size_for_category(&policy, UploadCategory::Image),
            5 * 1024 * 1024
        );
        assert_eq!(
            max_size_for_category(&policy, UploadCategory::Video),
            10 * 1024 * 1024
        );
        assert_eq!(
            max_size_for_category(&policy, UploadCategory::File),
            3 * 1024 * 1024
        );
    }

    #[test]
    fn category_enabled_checks() {
        let policy = annex_types::ServerPolicy {
            images_enabled: true,
            videos_enabled: false,
            files_enabled: true,
            ..Default::default()
        };
        assert!(is_category_enabled(&policy, UploadCategory::Image));
        assert!(!is_category_enabled(&policy, UploadCategory::Video));
        assert!(is_category_enabled(&policy, UploadCategory::File));
    }
}
