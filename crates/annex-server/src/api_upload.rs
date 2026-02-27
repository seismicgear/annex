//! Upload API handlers for images, videos, and files with automatic
//! metadata stripping and MIME type verification.
//!
//! Supports server image uploads (admin) and chat uploads (members).
//! All uploaded images have EXIF and other metadata stripped for privacy.
//! Video and file types are verified via magic-byte detection to prevent
//! type spoofing (e.g. zip bombs disguised as images).

use crate::{api::ApiError, middleware::IdentityContext, AppState};
use annex_channels::is_member;
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

    /// Returns the subdirectory name for file storage.
    fn subdir(self) -> &'static str {
        match self {
            Self::Image => "images",
            Self::Video => "videos",
            Self::File => "files",
        }
    }
}

// ── Allowed Content Types ──

/// Allowed image MIME types.
const ALLOWED_IMAGE_TYPES: &[&str] = &["image/jpeg", "image/png", "image/gif", "image/webp"];

/// Allowed video MIME types.
const ALLOWED_VIDEO_TYPES: &[&str] = &["video/mp4", "video/webm", "video/quicktime"];

/// Blocked MIME types that are never allowed (executables, etc.).
const BLOCKED_TYPES: &[&str] = &[
    "application/x-executable",
    "application/x-dosexec",
    "application/x-sharedlib",
];

/// Maximum upload file size for server images: 10 MiB (not configurable).
const MAX_SERVER_IMAGE_SIZE: usize = 10 * 1024 * 1024;

// ── Content Type Detection ──

/// Determines file extension from content type.
fn ext_from_content_type(ct: &str) -> &str {
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
fn detect_content_type(data: &[u8]) -> Option<(&'static str, UploadCategory)> {
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
fn max_size_for_category(policy: &annex_types::ServerPolicy, category: UploadCategory) -> usize {
    let mb = match category {
        UploadCategory::Image => policy.max_image_size_mb,
        UploadCategory::Video => policy.max_video_size_mb,
        UploadCategory::File => policy.max_file_size_mb,
    };
    mb as usize * 1024 * 1024
}

/// Checks whether a given upload category is enabled in the server policy.
fn is_category_enabled(policy: &annex_types::ServerPolicy, category: UploadCategory) -> bool {
    match category {
        UploadCategory::Image => policy.images_enabled,
        UploadCategory::Video => policy.videos_enabled,
        UploadCategory::File => policy.files_enabled,
    }
}

// ── EXIF / Metadata Stripping ──

/// Strips EXIF and other metadata from JPEG files without re-encoding.
///
/// Removes APP1 (EXIF/XMP), APP12 (Ducky), APP13 (IPTC), and COM segments
/// while preserving image data and quality.
fn strip_jpeg_metadata(data: &[u8]) -> Vec<u8> {
    if data.len() < 2 || data[0] != 0xFF || data[1] != 0xD8 {
        return data.to_vec();
    }

    let mut result = vec![0xFF, 0xD8];
    let mut i = 2;

    while i < data.len().saturating_sub(1) {
        if data[i] != 0xFF {
            // Not at a marker — copy remaining data as-is
            result.extend_from_slice(&data[i..]);
            break;
        }

        let marker = data[i + 1];

        // SOS (Start of Scan) — everything after this is image data
        if marker == 0xDA {
            result.extend_from_slice(&data[i..]);
            break;
        }

        // Markers without length (RST0-RST7, SOI, EOI)
        if marker == 0xD8 || marker == 0xD9 || (0xD0..=0xD7).contains(&marker) {
            result.push(0xFF);
            result.push(marker);
            i += 2;
            continue;
        }

        // Read segment length (2 bytes, big-endian)
        if i + 3 >= data.len() {
            break;
        }
        let seg_len = ((data[i + 2] as usize) << 8) | (data[i + 3] as usize);
        let total_len = seg_len + 2; // includes the FF xx marker bytes

        if i + total_len > data.len() {
            // Corrupted segment — copy rest as-is
            result.extend_from_slice(&data[i..]);
            break;
        }

        // Strip metadata markers:
        // APP1  (0xE1) = EXIF, XMP
        // APP12 (0xEC) = Ducky
        // APP13 (0xED) = IPTC / Photoshop
        // COM   (0xFE) = Comment
        let strip = matches!(marker, 0xE1 | 0xEC | 0xED | 0xFE);

        if !strip {
            result.extend_from_slice(&data[i..i + total_len]);
        }

        i += total_len;
    }

    result
}

/// Strips metadata chunks from PNG files without re-encoding.
///
/// Removes tEXt, iTXt, zTXt, and eXIf chunks while preserving
/// all image data chunks.
fn strip_png_metadata(data: &[u8]) -> Vec<u8> {
    let png_sig: [u8; 8] = [137, 80, 78, 71, 13, 10, 26, 10];

    if data.len() < 8 || data[..8] != png_sig {
        return data.to_vec();
    }

    let mut result = png_sig.to_vec();
    let mut i = 8;

    while i + 12 <= data.len() {
        let length = u32::from_be_bytes([data[i], data[i + 1], data[i + 2], data[i + 3]]) as usize;
        let chunk_type = &data[i + 4..i + 8];
        let total = 12 + length; // 4 length + 4 type + data + 4 CRC

        if i + total > data.len() {
            // Partial chunk — copy rest
            result.extend_from_slice(&data[i..]);
            break;
        }

        // Strip metadata chunks
        let strip = chunk_type == b"tEXt"
            || chunk_type == b"iTXt"
            || chunk_type == b"zTXt"
            || chunk_type == b"eXIf";

        if !strip {
            result.extend_from_slice(&data[i..i + total]);
        }

        i += total;
    }

    // Copy any trailing bytes
    if i < data.len() {
        result.extend_from_slice(&data[i..]);
    }

    result
}

/// Strips metadata from an image based on its content type.
/// Non-image types are returned as-is.
fn strip_metadata(data: &[u8], content_type: &str) -> Vec<u8> {
    match content_type {
        "image/jpeg" => strip_jpeg_metadata(data),
        "image/png" => strip_png_metadata(data),
        // GIF, WebP, videos, files: pass through
        _ => data.to_vec(),
    }
}

/// Hard ceiling for streaming multipart reads (50 MiB). Prevents any
/// single upload field from consuming unbounded memory before category-
/// specific limits are checked. The per-category limits may be lower.
const UPLOAD_STREAM_CEILING: usize = 50 * 1024 * 1024;

/// Reads a multipart field in chunks, enforcing a byte limit during
/// streaming. Returns `Err` as soon as the limit is exceeded, preventing
/// oversized payloads from being fully buffered into RAM.
async fn read_field_capped(
    field: &mut axum::extract::multipart::Field<'_>,
    max_bytes: usize,
) -> Result<Vec<u8>, ApiError> {
    let mut buf = Vec::new();
    while let Some(chunk) = field
        .chunk()
        .await
        .map_err(|e| ApiError::BadRequest(format!("failed to read upload chunk: {}", e)))?
    {
        if buf.len() + chunk.len() > max_bytes {
            return Err(ApiError::BadRequest(format!(
                "upload exceeds maximum size of {} bytes",
                max_bytes,
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
    if !identity.can_moderate {
        return Err(ApiError::Forbidden(
            "insufficient permissions to upload server image".to_string(),
        ));
    }

    // Extract the file field from multipart
    let mut field = multipart
        .next_field()
        .await
        .map_err(|e| ApiError::BadRequest(format!("multipart error: {}", e)))?
        .ok_or_else(|| ApiError::BadRequest("no file provided".to_string()))?;

    // Client-declared content type is intentionally ignored in favour of
    // magic-byte detection (see detected_ct below).
    let _declared_ct = field
        .content_type()
        .unwrap_or("application/octet-stream")
        .to_string();

    let original_filename = field.file_name().unwrap_or("image").to_string();

    // Stream with size enforcement — rejects before fully buffering
    let data = read_field_capped(&mut field, MAX_SERVER_IMAGE_SIZE).await?;
    let data = axum::body::Bytes::from(data);

    // Detect actual content type from magic bytes (server images must be images)
    let (detected_ct, category) = detect_content_type(&data)
        .ok_or_else(|| ApiError::BadRequest("unsupported file format".to_string()))?;

    if category != UploadCategory::Image || !ALLOWED_IMAGE_TYPES.contains(&detected_ct) {
        return Err(ApiError::BadRequest(
            "server image must be JPEG, PNG, GIF, or WebP".to_string(),
        ));
    }

    // Strip metadata
    let cleaned = strip_metadata(&data, detected_ct);

    // Save to disk
    let ext = ext_from_content_type(detected_ct);
    let upload_id = Uuid::new_v4().to_string();
    let filename = format!("server_{}.{}", upload_id, ext);
    let upload_dir = state.upload_dir.clone();
    let server_dir = format!("{}/server", upload_dir);

    tokio::fs::create_dir_all(&server_dir).await.map_err(|e| {
        ApiError::InternalServerError(format!("failed to create upload dir: {}", e))
    })?;

    let file_path = format!("{}/{}", server_dir, filename);
    tokio::fs::write(&file_path, &cleaned)
        .await
        .map_err(|e| ApiError::InternalServerError(format!("failed to write file: {}", e)))?;

    let image_url = format!("/uploads/server/{}", filename);

    // Update database
    let state_clone = state.clone();
    let image_url_clone = image_url.clone();
    let upload_id_clone = upload_id.clone();
    let original_filename_clone = original_filename.clone();
    // Store the magic-byte-detected MIME type, not the client-declared one,
    // to ensure downstream consumers see a consistent, verified content type.
    let content_type_clone = detected_ct.to_string();
    let size = cleaned.len() as i64;
    let moderator = identity.pseudonym_id.clone();

    tokio::task::spawn_blocking(move || {
        let conn = state_clone.pool.get().map_err(|e| {
            ApiError::InternalServerError(format!("db connection failed: {}", e))
        })?;

        // Update server image URL
        conn.execute(
            "UPDATE servers SET image_url = ?1 WHERE id = ?2",
            rusqlite::params![image_url_clone, state_clone.server_id],
        )
        .map_err(|e| ApiError::InternalServerError(format!("failed to update server: {}", e)))?;

        // Record upload
        conn.execute(
            "INSERT INTO uploads (server_id, upload_id, uploader_pseudonym, original_filename, content_type, size_bytes, purpose, category)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'server_image', 'image')",
            rusqlite::params![
                state_clone.server_id,
                upload_id_clone,
                moderator,
                original_filename_clone,
                content_type_clone,
                size,
            ],
        )
        .map_err(|e| ApiError::InternalServerError(format!("failed to record upload: {}", e)))?;

        let observe_payload = EventPayload::ModerationAction {
            moderator_pseudonym: moderator.clone(),
            action_type: "server_image_upload".to_string(),
            target_pseudonym: None,
            description: "Server image updated".to_string(),
        };
        crate::emit_and_broadcast(
            &conn,
            state_clone.server_id,
            &moderator,
            &observe_payload,
            &state_clone.observe_tx,
        );

        Ok::<(), ApiError>(())
    })
    .await
    .map_err(|e| ApiError::InternalServerError(format!("task join error: {}", e)))??;

    Ok(AxumJson(serde_json::json!({
        "status": "ok",
        "upload_id": upload_id,
        "url": image_url,
    }))
    .into_response())
}

/// Handler for `POST /api/channels/{channelId}/upload`.
///
/// Uploads an image, video, or file to a channel. Requires channel membership.
/// Automatically strips EXIF and other metadata from images for privacy.
/// Enforces per-category size limits and enabled/disabled toggles from server policy.
pub async fn upload_chat_handler(
    Extension(state): Extension<Arc<AppState>>,
    Extension(IdentityContext(identity)): Extension<IdentityContext>,
    Path(channel_id): Path<String>,
    mut multipart: Multipart,
) -> Result<Response, ApiError> {
    // Check channel membership
    let state_clone = state.clone();
    let channel_id_clone = channel_id.clone();
    let pseudonym = identity.pseudonym_id.clone();

    let is_member_result = tokio::task::spawn_blocking(move || {
        let conn = state_clone
            .pool
            .get()
            .map_err(|e| ApiError::InternalServerError(format!("db connection failed: {}", e)))?;
        is_member(&conn, state_clone.server_id, &channel_id_clone, &pseudonym)
            .map_err(|e| ApiError::InternalServerError(format!("membership check failed: {}", e)))
    })
    .await
    .map_err(|e| ApiError::InternalServerError(format!("task join error: {}", e)))??;

    if !is_member_result {
        return Err(ApiError::Forbidden(
            "not a member of this channel".to_string(),
        ));
    }

    // Read current server policy for limits
    let policy = state
        .policy
        .read()
        .map_err(|_| ApiError::InternalServerError("policy lock poisoned".to_string()))?
        .clone();

    // Extract file field
    let mut field = multipart
        .next_field()
        .await
        .map_err(|e| ApiError::BadRequest(format!("multipart error: {}", e)))?
        .ok_or_else(|| ApiError::BadRequest("no file provided".to_string()))?;

    let _declared_ct = field
        .content_type()
        .unwrap_or("application/octet-stream")
        .to_string();

    let original_filename = field.file_name().unwrap_or("upload").to_string();

    // Stream with size enforcement — rejects before fully buffering.
    // Uses a hard ceiling; per-category limits are enforced after detection.
    let data = read_field_capped(&mut field, UPLOAD_STREAM_CEILING).await?;
    let data = axum::body::Bytes::from(data);

    // Detect actual content type from magic bytes.
    // When magic bytes are unrecognized, treat the file as a generic binary
    // regardless of the declared Content-Type header. This prevents:
    // (1) memory leaks from the previous Box::leak approach, and
    // (2) declared-MIME bypass of magic byte verification.
    let (detected_ct, category) =
        detect_content_type(&data).unwrap_or(("application/octet-stream", UploadCategory::File));

    // Check for blocked types
    if BLOCKED_TYPES.contains(&detected_ct) {
        return Err(ApiError::BadRequest(format!(
            "file type not allowed: {}",
            detected_ct
        )));
    }

    // Check if this category is enabled
    if !is_category_enabled(&policy, category) {
        return Err(ApiError::BadRequest(format!(
            "{} uploads are disabled on this server",
            category.as_str()
        )));
    }

    // Enforce per-category size limit
    let max_size = max_size_for_category(&policy, category);
    if data.len() > max_size {
        return Err(ApiError::BadRequest(format!(
            "file too large: {} bytes (max {} bytes for {})",
            data.len(),
            max_size,
            category.as_str()
        )));
    }

    // Validate specific types within categories
    if category == UploadCategory::Image && !ALLOWED_IMAGE_TYPES.contains(&detected_ct) {
        return Err(ApiError::BadRequest(format!(
            "unsupported image type: {}",
            detected_ct
        )));
    }
    if category == UploadCategory::Video && !ALLOWED_VIDEO_TYPES.contains(&detected_ct) {
        return Err(ApiError::BadRequest(format!(
            "unsupported video type: {}",
            detected_ct
        )));
    }

    // Strip metadata for images; videos and files pass through
    let cleaned = strip_metadata(&data, detected_ct);
    let stripped_bytes = data.len() - cleaned.len();

    // Save to disk (category-specific subdirectory)
    let ext = ext_from_content_type(detected_ct);
    let upload_id = Uuid::new_v4().to_string();
    let safe_filename = format!("{}.{}", upload_id, ext);
    let upload_dir = state.upload_dir.clone();
    let chat_dir = format!("{}/chat/{}", upload_dir, category.subdir());

    tokio::fs::create_dir_all(&chat_dir).await.map_err(|e| {
        ApiError::InternalServerError(format!("failed to create upload dir: {}", e))
    })?;

    let file_path = format!("{}/{}", chat_dir, safe_filename);
    tokio::fs::write(&file_path, &cleaned)
        .await
        .map_err(|e| ApiError::InternalServerError(format!("failed to write file: {}", e)))?;

    let upload_url = format!("/uploads/chat/{}/{}", category.subdir(), safe_filename);

    // Record in database
    let state_clone = state.clone();
    let upload_id_clone = upload_id.clone();
    let original_filename_clone = original_filename.clone();
    let detected_ct_str = detected_ct.to_string();
    let category_str = category.as_str().to_string();
    let size = cleaned.len() as i64;
    let uploader = identity.pseudonym_id.clone();
    let channel_id_db = channel_id.clone();

    tokio::task::spawn_blocking(move || {
        let conn = state_clone.pool.get().map_err(|e| {
            ApiError::InternalServerError(format!("db connection failed: {}", e))
        })?;

        conn.execute(
            "INSERT INTO uploads (server_id, upload_id, uploader_pseudonym, original_filename, content_type, size_bytes, purpose, channel_id, category)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'chat', ?7, ?8)",
            rusqlite::params![
                state_clone.server_id,
                upload_id_clone,
                uploader,
                original_filename_clone,
                detected_ct_str,
                size,
                channel_id_db,
                category_str,
            ],
        )
        .map_err(|e| ApiError::InternalServerError(format!("failed to record upload: {}", e)))?;

        Ok::<(), ApiError>(())
    })
    .await
    .map_err(|e| ApiError::InternalServerError(format!("task join error: {}", e)))??;

    tracing::info!(
        upload_id = %upload_id,
        channel_id = %channel_id,
        uploader = %identity.pseudonym_id,
        original_filename = %original_filename,
        content_type = %detected_ct,
        category = %category.as_str(),
        size_bytes = size,
        metadata_stripped_bytes = stripped_bytes,
        "chat upload completed"
    );

    Ok(AxumJson(serde_json::json!({
        "status": "ok",
        "upload_id": upload_id,
        "url": upload_url,
        "filename": original_filename,
        "content_type": detected_ct,
        "category": category.as_str(),
        "size": size,
        "metadata_stripped_bytes": stripped_bytes,
    }))
    .into_response())
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
            .map_err(|e| ApiError::InternalServerError(format!("db connection failed: {}", e)))?;

        let url: Option<String> = conn
            .query_row(
                "SELECT image_url FROM servers WHERE id = ?1",
                rusqlite::params![state_clone.server_id],
                |row| row.get(0),
            )
            .map_err(|e| ApiError::InternalServerError(format!("failed to query server: {}", e)))?;

        Ok::<_, ApiError>(url)
    })
    .await
    .map_err(|e| ApiError::InternalServerError(format!("task join error: {}", e)))??;

    Ok(AxumJson(serde_json::json!({
        "image_url": image_url,
    }))
    .into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_jpeg_preserves_image_data() {
        // Minimal JPEG: SOI + APP0 + SOS + EOI
        let mut jpeg = vec![0xFF, 0xD8]; // SOI
                                         // APP0 (keep)
        jpeg.extend_from_slice(&[0xFF, 0xE0, 0x00, 0x04, 0x00, 0x00]);
        // APP1/EXIF (strip)
        jpeg.extend_from_slice(&[0xFF, 0xE1, 0x00, 0x06, 0x45, 0x78, 0x69, 0x66]);
        // SOS + image data
        jpeg.extend_from_slice(&[0xFF, 0xDA, 0x00, 0x02, 0x01, 0x02, 0x03]);

        let stripped = strip_jpeg_metadata(&jpeg);

        // Should contain SOI
        assert_eq!(stripped[0], 0xFF);
        assert_eq!(stripped[1], 0xD8);
        // Should contain APP0
        assert!(stripped.windows(2).any(|w| w == [0xFF, 0xE0]));
        // Should NOT contain APP1
        assert!(!stripped.windows(2).any(|w| w == [0xFF, 0xE1]));
        // Should contain SOS
        assert!(stripped.windows(2).any(|w| w == [0xFF, 0xDA]));
    }

    #[test]
    fn strip_png_removes_text_chunks() {
        let sig = [137, 80, 78, 71, 13, 10, 26, 10];
        let mut png = sig.to_vec();

        // IHDR chunk (keep) - 13 bytes data
        let ihdr_data = [0u8; 13];
        png.extend_from_slice(&(13u32).to_be_bytes()); // length
        png.extend_from_slice(b"IHDR");
        png.extend_from_slice(&ihdr_data);
        png.extend_from_slice(&[0, 0, 0, 0]); // CRC placeholder

        // tEXt chunk (strip) - 5 bytes data
        let text_data = [0u8; 5];
        png.extend_from_slice(&(5u32).to_be_bytes());
        png.extend_from_slice(b"tEXt");
        png.extend_from_slice(&text_data);
        png.extend_from_slice(&[0, 0, 0, 0]);

        // IEND chunk (keep) - 0 bytes data
        png.extend_from_slice(&(0u32).to_be_bytes());
        png.extend_from_slice(b"IEND");
        png.extend_from_slice(&[0, 0, 0, 0]);

        let stripped = strip_png_metadata(&png);

        // Should start with PNG signature
        assert_eq!(&stripped[..8], &sig);
        // Should contain IHDR and IEND
        assert!(stripped.windows(4).any(|w| w == *b"IHDR"));
        assert!(stripped.windows(4).any(|w| w == *b"IEND"));
        // Should NOT contain tEXt
        assert!(!stripped.windows(4).any(|w| w == *b"tEXt"));
        // Should be smaller
        assert!(stripped.len() < png.len());
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
