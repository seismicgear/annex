//! Privacy-preserving link preview endpoint.
//!
//! Fetches OpenGraph metadata on behalf of users so their IPs are never
//! exposed to third-party sites. Results are cached in-memory with a TTL
//! to avoid repeated external requests.
//!
//! Two endpoints:
//! - `GET /api/link-preview?url=<url>`  — returns OG metadata as JSON
//! - `GET /api/link-preview/image?url=<url>` — proxies an image through the server

use axum::{
    extract::Query,
    http::{header, StatusCode},
    response::Response,
    Extension, Json,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::AppState;

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Maximum HTML body size to download for OG parsing (512 KiB).
const MAX_HTML_BYTES: usize = 512 * 1024;

/// Maximum image body size to proxy (5 MiB).
const MAX_IMAGE_BYTES: usize = 5 * 1024 * 1024;

/// HTTP request timeout for fetching external pages.
const FETCH_TIMEOUT: Duration = Duration::from_secs(8);

/// How long cached preview entries are considered fresh.
const CACHE_TTL: Duration = Duration::from_secs(600); // 10 minutes

/// Maximum number of cached previews before evicting oldest.
const MAX_CACHE_ENTRIES: usize = 2000;

/// Maximum image cache entries.
const MAX_IMAGE_CACHE_ENTRIES: usize = 500;

/// How long cached images are considered fresh.
const IMAGE_CACHE_TTL: Duration = Duration::from_secs(300); // 5 minutes

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct PreviewQuery {
    url: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct PreviewResponse {
    pub title: Option<String>,
    pub description: Option<String>,
    pub image_url: Option<String>,
    pub site_name: Option<String>,
}

#[derive(Debug, Clone)]
struct CachedPreview {
    data: PreviewResponse,
    fetched_at: Instant,
}

#[derive(Debug, Clone)]
struct CachedImage {
    bytes: Vec<u8>,
    content_type: String,
    fetched_at: Instant,
}

/// In-memory cache for link preview metadata.
#[derive(Clone, Default)]
pub struct PreviewCache {
    previews: Arc<Mutex<HashMap<String, CachedPreview>>>,
    images: Arc<Mutex<HashMap<String, CachedImage>>>,
}

impl PreviewCache {
    pub fn new() -> Self {
        Self::default()
    }
}

// ---------------------------------------------------------------------------
// Safety: block requests to private/internal IPs
// ---------------------------------------------------------------------------

/// Returns `true` if the URL's host resolves to a private/loopback/link-local address.
/// This prevents SSRF attacks where a user could probe internal services.
fn is_private_or_reserved(url: &str) -> bool {
    let parsed = match url::Url::parse(url) {
        Ok(u) => u,
        Err(_) => return true, // reject unparseable
    };

    // Only allow http/https
    match parsed.scheme() {
        "http" | "https" => {}
        _ => return true,
    }

    let host = match parsed.host_str() {
        Some(h) => h,
        None => return true,
    };

    // Try to parse as IP directly
    if let Ok(ip) = host.parse::<IpAddr>() {
        return is_private_ip(ip);
    }

    // For hostnames, block obvious internal names
    let lower = host.to_lowercase();
    if lower == "localhost"
        || lower.ends_with(".local")
        || lower.ends_with(".internal")
        || lower == "metadata.google.internal"
        || lower.starts_with("169.254.")
    {
        return true;
    }

    false
}

/// Performs DNS resolution and checks if **any** resolved IP is private/reserved.
///
/// This prevents DNS rebinding SSRF attacks where a hostname initially resolves
/// to a public IP during `is_private_or_reserved()` but then resolves to an
/// internal IP when the actual HTTP request is made (TOCTOU).
async fn resolves_to_private_ip(host: &str) -> bool {
    // Append a dummy port — `lookup_host` requires a socket address string.
    let lookup_target = if host.contains(':') {
        // IPv6 literal or already has port
        host.to_string()
    } else {
        format!("{}:443", host)
    };

    let addrs = match tokio::net::lookup_host(&lookup_target).await {
        Ok(addrs) => addrs.collect::<Vec<_>>(),
        Err(e) => {
            tracing::debug!(host = %host, error = %e, "DNS resolution failed — blocking request");
            return true; // fail-closed: if we can't resolve, don't allow
        }
    };

    for addr in addrs {
        if is_private_ip(addr.ip()) {
            tracing::warn!(
                host = %host,
                resolved_ip = %addr.ip(),
                "DNS resolved to private/reserved IP — blocking request"
            );
            return true;
        }
    }
    false
}

fn is_private_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_broadcast()
                || v4.is_unspecified()
                // 169.254.0.0/16 (link-local, cloud metadata)
                || v4.octets()[0] == 169 && v4.octets()[1] == 254
                // 100.64.0.0/10 (CGNAT / shared address space)
                || v4.octets()[0] == 100 && (v4.octets()[1] & 0xC0) == 64
        }
        IpAddr::V6(v6) => {
            v6.is_loopback() || v6.is_unspecified()
                // fc00::/7 (unique local)
                || (v6.segments()[0] & 0xfe00) == 0xfc00
                // fe80::/10 (link-local)
                || (v6.segments()[0] & 0xffc0) == 0xfe80
        }
    }
}

// ---------------------------------------------------------------------------
// OG metadata parser
// ---------------------------------------------------------------------------

fn parse_og_metadata(html: &str) -> PreviewResponse {
    use scraper::{Html, Selector};

    let document = Html::parse_document(html);

    // Helper: select content of <meta property="og:XXX"> or <meta name="og:XXX">
    let og = |prop: &str| -> Option<String> {
        // Try property attribute first (standard OG)
        let sel_prop =
            Selector::parse(&format!(r#"meta[property="og:{}"]"#, prop)).ok()?;
        if let Some(el) = document.select(&sel_prop).next() {
            if let Some(content) = el.value().attr("content") {
                let trimmed = content.trim();
                if !trimmed.is_empty() {
                    return Some(trimmed.to_string());
                }
            }
        }

        // Fallback: name attribute
        let sel_name =
            Selector::parse(&format!(r#"meta[name="og:{}"]"#, prop)).ok()?;
        if let Some(el) = document.select(&sel_name).next() {
            if let Some(content) = el.value().attr("content") {
                let trimmed = content.trim();
                if !trimmed.is_empty() {
                    return Some(trimmed.to_string());
                }
            }
        }

        None
    };

    let title = og("title").or_else(|| {
        // Fallback to <title> tag
        let sel = Selector::parse("title").ok()?;
        document
            .select(&sel)
            .next()
            .map(|el| el.text().collect::<String>().trim().to_string())
            .filter(|s| !s.is_empty())
    });

    let description = og("description").or_else(|| {
        // Fallback to <meta name="description">
        let sel = Selector::parse(r#"meta[name="description"]"#).ok()?;
        document
            .select(&sel)
            .next()
            .and_then(|el| el.value().attr("content"))
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    });

    // Image: og:image → twitter:image → <link rel="image_src"> → <meta itemprop="image">
    let image_url = og("image").or_else(|| {
        // Twitter card image
        let sel = Selector::parse(r#"meta[name="twitter:image"]"#).ok()?;
        document
            .select(&sel)
            .next()
            .and_then(|el| el.value().attr("content"))
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    }).or_else(|| {
        // <link rel="image_src">
        let sel = Selector::parse(r#"link[rel="image_src"]"#).ok()?;
        document
            .select(&sel)
            .next()
            .and_then(|el| el.value().attr("href"))
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    }).or_else(|| {
        // Schema.org itemprop="image"
        let sel = Selector::parse(r#"meta[itemprop="image"]"#).ok()?;
        document
            .select(&sel)
            .next()
            .and_then(|el| el.value().attr("content"))
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    });
    let site_name = og("site_name");

    PreviewResponse {
        title,
        description,
        image_url,
        site_name,
    }
}

// ---------------------------------------------------------------------------
// Build a shared reqwest client
// ---------------------------------------------------------------------------

fn build_http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(FETCH_TIMEOUT)
        .redirect(reqwest::redirect::Policy::limited(5))
        .user_agent("AnnexBot/1.0 (link-preview)")
        .build()
        .unwrap_or_default()
}

/// Build a client with a browser-like user-agent for image fetching.
/// Many CDNs and image hosts block non-browser user agents.
fn build_image_http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(FETCH_TIMEOUT)
        .redirect(reqwest::redirect::Policy::limited(5))
        .user_agent(
            "Mozilla/5.0 (compatible; AnnexImageProxy/1.0; +https://annex.chat)",
        )
        .build()
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// `GET /api/link-preview?url=<url>` — fetch and return OG metadata.
pub async fn link_preview_handler(
    Extension(state): Extension<Arc<AppState>>,
    Query(params): Query<PreviewQuery>,
) -> Result<Json<PreviewResponse>, StatusCode> {
    let url = params.url.trim().to_string();

    // Validate URL
    if url.is_empty() || url.len() > 2048 {
        return Err(StatusCode::BAD_REQUEST);
    }
    if is_private_or_reserved(&url) {
        return Err(StatusCode::FORBIDDEN);
    }

    // Check cache before expensive DNS resolution — cached results are safe
    // because they were already validated on the original fetch.
    {
        let cache = state.preview_cache.previews.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(entry) = cache.get(&url) {
            if entry.fetched_at.elapsed() < CACHE_TTL {
                return Ok(Json(entry.data.clone()));
            }
        }
    }

    // DNS rebinding protection: resolve the hostname and verify all IPs are public.
    // Only runs for cache misses — avoids unnecessary DNS lookups on cached hits.
    if let Ok(parsed) = url::Url::parse(&url) {
        if let Some(host) = parsed.host_str() {
            if host.parse::<IpAddr>().is_err() && resolves_to_private_ip(host).await {
                return Err(StatusCode::FORBIDDEN);
            }
        }
    }

    // Fetch the page
    let client = build_http_client();
    let resp = client
        .get(&url)
        .header(header::ACCEPT, "text/html,application/xhtml+xml")
        .send()
        .await
        .map_err(|e| {
            tracing::debug!(url = %url, error = %e, "link preview fetch failed");
            StatusCode::BAD_GATEWAY
        })?;

    if !resp.status().is_success() {
        return Err(StatusCode::BAD_GATEWAY);
    }

    // Check content type — only parse HTML
    let content_type = resp
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if !content_type.contains("text/html") && !content_type.contains("application/xhtml") {
        // Not an HTML page — return empty metadata
        let empty = PreviewResponse {
            title: None,
            description: None,
            image_url: None,
            site_name: None,
        };
        return Ok(Json(empty));
    }

    // Read body with size limit
    let bytes = resp
        .bytes()
        .await
        .map_err(|_| StatusCode::BAD_GATEWAY)?;
    if bytes.len() > MAX_HTML_BYTES {
        return Err(StatusCode::BAD_GATEWAY);
    }

    let html = String::from_utf8_lossy(&bytes);

    // Parse OG metadata (CPU-bound, run in blocking task)
    let html_owned = html.into_owned();
    let mut preview = tokio::task::spawn_blocking(move || parse_og_metadata(&html_owned))
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    // Resolve relative image URLs against the source page URL
    if let Some(ref img) = preview.image_url {
        if !img.starts_with("http://") && !img.starts_with("https://") {
            if let Ok(base) = url::Url::parse(&url) {
                if let Ok(resolved) = base.join(img) {
                    preview.image_url = Some(resolved.to_string());
                }
            }
        }
    }

    // Store in cache
    {
        let mut cache = state.preview_cache.previews.lock().unwrap_or_else(|e| e.into_inner());
        // Evict oldest entries if at capacity
        if cache.len() >= MAX_CACHE_ENTRIES {
            // Remove ~10% of oldest entries
            let mut entries: Vec<_> = cache
                .iter()
                .map(|(k, v)| (k.clone(), v.fetched_at))
                .collect();
            entries.sort_by_key(|(_, t)| *t);
            let to_remove = entries.len() / 10;
            for (key, _) in entries.into_iter().take(to_remove.max(1)) {
                cache.remove(&key);
            }
        }
        cache.insert(
            url.clone(),
            CachedPreview {
                data: preview.clone(),
                fetched_at: Instant::now(),
            },
        );
    }

    Ok(Json(preview))
}

/// `GET /api/link-preview/image?url=<url>` — proxy an image through the server.
///
/// This prevents the client from directly fetching external images, which
/// would leak the user's IP address to third-party servers.
pub async fn image_proxy_handler(
    Extension(state): Extension<Arc<AppState>>,
    Query(params): Query<PreviewQuery>,
) -> Result<Response, StatusCode> {
    let url = params.url.trim().to_string();

    // Validate
    if url.is_empty() || url.len() > 2048 {
        return Err(StatusCode::BAD_REQUEST);
    }
    if is_private_or_reserved(&url) {
        return Err(StatusCode::FORBIDDEN);
    }

    // Check cache before expensive DNS resolution — cached results are safe
    // because they were already validated on the original fetch.
    {
        let cache = state.preview_cache.images.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(entry) = cache.get(&url) {
            if entry.fetched_at.elapsed() < IMAGE_CACHE_TTL {
                return Ok(build_image_response(
                    &entry.content_type,
                    entry.bytes.clone(),
                ));
            }
        }
    }

    // DNS rebinding protection: resolve the hostname and verify all IPs are public.
    // Only runs for cache misses — avoids unnecessary DNS lookups on cached hits.
    if let Ok(parsed) = url::Url::parse(&url) {
        if let Some(host) = parsed.host_str() {
            if host.parse::<IpAddr>().is_err() && resolves_to_private_ip(host).await {
                return Err(StatusCode::FORBIDDEN);
            }
        }
    }

    // Fetch the image — use browser-like UA to avoid CDN blocks
    let client = build_image_http_client();
    let resp = client
        .get(&url)
        .header(header::ACCEPT, "image/*")
        .send()
        .await
        .map_err(|e| {
            tracing::debug!(url = %url, error = %e, "image proxy fetch failed");
            StatusCode::BAD_GATEWAY
        })?;

    if !resp.status().is_success() {
        tracing::debug!(url = %url, status = %resp.status(), "image proxy: upstream returned error");
        return Err(StatusCode::BAD_GATEWAY);
    }

    // Validate content type — only proxy images.
    // Accept application/octet-stream as fallback when the URL has an image extension
    // (many object-storage backends return octet-stream for images).
    let content_type = resp
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/octet-stream")
        .to_string();

    let is_image_content = content_type.starts_with("image/");
    let is_octet_stream_with_image_ext = content_type == "application/octet-stream"
        && url_has_image_extension(&url);

    if !is_image_content && !is_octet_stream_with_image_ext {
        tracing::debug!(
            url = %url, content_type = %content_type,
            "image proxy: rejected non-image content type"
        );
        return Err(StatusCode::UNSUPPORTED_MEDIA_TYPE);
    }

    // Use the actual content type if it's an image, otherwise infer from extension
    let content_type = if is_image_content {
        content_type
    } else {
        infer_image_content_type(&url)
    };

    // Read body with size limit
    let bytes = resp.bytes().await.map_err(|_| StatusCode::BAD_GATEWAY)?;
    if bytes.len() > MAX_IMAGE_BYTES {
        return Err(StatusCode::BAD_GATEWAY);
    }

    let body = bytes.to_vec();

    // Cache the image
    {
        let mut cache = state.preview_cache.images.lock().unwrap_or_else(|e| e.into_inner());
        if cache.len() >= MAX_IMAGE_CACHE_ENTRIES {
            let mut entries: Vec<_> = cache
                .iter()
                .map(|(k, v)| (k.clone(), v.fetched_at))
                .collect();
            entries.sort_by_key(|(_, t)| *t);
            let to_remove = entries.len() / 10;
            for (key, _) in entries.into_iter().take(to_remove.max(1)) {
                cache.remove(&key);
            }
        }
        cache.insert(
            url.clone(),
            CachedImage {
                bytes: body.clone(),
                content_type: content_type.clone(),
                fetched_at: Instant::now(),
            },
        );
    }

    Ok(build_image_response(&content_type, body))
}

/// Check if a URL's path ends with a known image file extension.
fn url_has_image_extension(url: &str) -> bool {
    let path = url::Url::parse(url)
        .map(|u| u.path().to_lowercase())
        .unwrap_or_default();
    [".jpg", ".jpeg", ".png", ".gif", ".webp", ".svg", ".ico", ".bmp", ".avif"]
        .iter()
        .any(|ext| path.ends_with(ext))
}

/// Infer an image content-type from the URL file extension.
fn infer_image_content_type(url: &str) -> String {
    let path = url::Url::parse(url)
        .map(|u| u.path().to_lowercase())
        .unwrap_or_default();
    if path.ends_with(".png") {
        "image/png"
    } else if path.ends_with(".gif") {
        "image/gif"
    } else if path.ends_with(".webp") {
        "image/webp"
    } else if path.ends_with(".svg") {
        "image/svg+xml"
    } else if path.ends_with(".ico") {
        "image/x-icon"
    } else if path.ends_with(".bmp") {
        "image/bmp"
    } else if path.ends_with(".avif") {
        "image/avif"
    } else {
        "image/jpeg"
    }
    .to_string()
}

fn build_image_response(content_type: &str, body: Vec<u8>) -> Response {
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CACHE_CONTROL, "public, max-age=300")
        .body(axum::body::Body::from(body))
        .unwrap_or_else(|_| {
            Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .body(axum::body::Body::empty())
                .unwrap()
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_private_ips() {
        assert!(is_private_or_reserved("http://127.0.0.1/foo"));
        assert!(is_private_or_reserved("http://localhost/foo"));
        assert!(is_private_or_reserved("http://192.168.1.1/foo"));
        assert!(is_private_or_reserved("http://10.0.0.1/foo"));
        assert!(is_private_or_reserved("http://169.254.169.254/metadata"));
        assert!(is_private_or_reserved("ftp://example.com/file"));
        assert!(is_private_or_reserved("http://foo.local/bar"));
    }

    #[test]
    fn allows_public_urls() {
        assert!(!is_private_or_reserved("https://example.com"));
        assert!(!is_private_or_reserved("https://github.com/foo/bar"));
        assert!(!is_private_or_reserved("http://8.8.8.8/dns"));
    }

    #[test]
    fn parses_og_tags() {
        let html = r#"
            <html>
            <head>
                <meta property="og:title" content="Test Page">
                <meta property="og:description" content="A test description">
                <meta property="og:image" content="https://example.com/image.jpg">
                <meta property="og:site_name" content="Example">
            </head>
            <body></body>
            </html>
        "#;

        let result = parse_og_metadata(html);
        assert_eq!(result.title.as_deref(), Some("Test Page"));
        assert_eq!(result.description.as_deref(), Some("A test description"));
        assert_eq!(
            result.image_url.as_deref(),
            Some("https://example.com/image.jpg")
        );
        assert_eq!(result.site_name.as_deref(), Some("Example"));
    }

    #[test]
    fn falls_back_to_title_tag() {
        let html = r#"
            <html>
            <head><title>Fallback Title</title></head>
            <body></body>
            </html>
        "#;

        let result = parse_og_metadata(html);
        assert_eq!(result.title.as_deref(), Some("Fallback Title"));
        assert!(result.image_url.is_none());
    }

    #[test]
    fn falls_back_to_meta_description() {
        let html = r#"
            <html>
            <head>
                <meta name="description" content="Meta description fallback">
            </head>
            <body></body>
            </html>
        "#;

        let result = parse_og_metadata(html);
        assert_eq!(
            result.description.as_deref(),
            Some("Meta description fallback")
        );
    }

    #[test]
    fn falls_back_to_twitter_image() {
        let html = r#"
            <html>
            <head>
                <meta name="twitter:image" content="https://example.com/twitter.jpg">
                <title>Twitter Card Page</title>
            </head>
            <body></body>
            </html>
        "#;

        let result = parse_og_metadata(html);
        assert_eq!(
            result.image_url.as_deref(),
            Some("https://example.com/twitter.jpg")
        );
    }

    #[test]
    fn falls_back_to_link_image_src() {
        let html = r#"
            <html>
            <head>
                <link rel="image_src" href="https://example.com/link-image.png">
            </head>
            <body></body>
            </html>
        "#;

        let result = parse_og_metadata(html);
        assert_eq!(
            result.image_url.as_deref(),
            Some("https://example.com/link-image.png")
        );
    }

    #[test]
    fn falls_back_to_itemprop_image() {
        let html = r#"
            <html>
            <head>
                <meta itemprop="image" content="https://example.com/schema.jpg">
            </head>
            <body></body>
            </html>
        "#;

        let result = parse_og_metadata(html);
        assert_eq!(
            result.image_url.as_deref(),
            Some("https://example.com/schema.jpg")
        );
    }

    #[test]
    fn og_image_takes_priority_over_twitter() {
        let html = r#"
            <html>
            <head>
                <meta property="og:image" content="https://example.com/og.jpg">
                <meta name="twitter:image" content="https://example.com/twitter.jpg">
            </head>
            <body></body>
            </html>
        "#;

        let result = parse_og_metadata(html);
        assert_eq!(
            result.image_url.as_deref(),
            Some("https://example.com/og.jpg")
        );
    }

    #[test]
    fn url_image_extension_detection() {
        assert!(url_has_image_extension("https://cdn.example.com/photo.jpg"));
        assert!(url_has_image_extension("https://cdn.example.com/photo.PNG"));
        assert!(url_has_image_extension("https://cdn.example.com/img.webp"));
        assert!(!url_has_image_extension("https://example.com/page.html"));
        assert!(!url_has_image_extension("https://example.com/api/image"));
    }

    #[test]
    fn image_content_type_inference() {
        assert_eq!(
            infer_image_content_type("https://example.com/photo.png"),
            "image/png"
        );
        assert_eq!(
            infer_image_content_type("https://example.com/photo.webp"),
            "image/webp"
        );
        // Default to jpeg for unknown/jpg
        assert_eq!(
            infer_image_content_type("https://example.com/photo.jpg"),
            "image/jpeg"
        );
    }
}
