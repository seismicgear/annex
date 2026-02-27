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
use std::net::{IpAddr, SocketAddr};
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

    // Use the typed host() enum to correctly handle IPv4, IPv6, and domains.
    // host_str() returns brackets around IPv6 which won't parse with IpAddr.
    match parsed.host() {
        Some(url::Host::Ipv4(v4)) => return is_private_ip(IpAddr::V4(v4)),
        Some(url::Host::Ipv6(v6)) => return is_private_ip(IpAddr::V6(v6)),
        Some(url::Host::Domain(domain)) => {
            let lower = domain.to_lowercase();
            if lower == "localhost"
                || lower.ends_with(".local")
                || lower.ends_with(".internal")
                || lower == "metadata.google.internal"
                || lower.starts_with("169.254.")
            {
                return true;
            }
        }
        None => return true,
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
            // IPv4-mapped IPv6 addresses (::ffff:x.x.x.x) — delegate to
            // the IPv4 check so private ranges aren't bypassed.
            if let Some(mapped_v4) = v6.to_ipv4_mapped() {
                return is_private_ip(IpAddr::V4(mapped_v4));
            }
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
        let sel_prop = Selector::parse(&format!(r#"meta[property="og:{}"]"#, prop)).ok()?;
        if let Some(el) = document.select(&sel_prop).next() {
            if let Some(content) = el.value().attr("content") {
                let trimmed = content.trim();
                if !trimmed.is_empty() {
                    return Some(trimmed.to_string());
                }
            }
        }

        // Fallback: name attribute
        let sel_name = Selector::parse(&format!(r#"meta[name="og:{}"]"#, prop)).ok()?;
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
    let image_url = og("image")
        .or_else(|| {
            // Twitter card image
            let sel = Selector::parse(r#"meta[name="twitter:image"]"#).ok()?;
            document
                .select(&sel)
                .next()
                .and_then(|el| el.value().attr("content"))
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        })
        .or_else(|| {
            // <link rel="image_src">
            let sel = Selector::parse(r#"link[rel="image_src"]"#).ok()?;
            document
                .select(&sel)
                .next()
                .and_then(|el| el.value().attr("href"))
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        })
        .or_else(|| {
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
// Build a shared reqwest client with SSRF-safe redirect policy
// ---------------------------------------------------------------------------

/// Maximum number of redirect hops to follow.
const MAX_REDIRECT_HOPS: usize = 5;

/// Resolve a hostname to a socket address and validate that ALL resolved IPs
/// are public. Returns a validated address for DNS-pinned requests.
async fn resolve_and_validate(host: &str, port: u16) -> Result<SocketAddr, ()> {
    let lookup = format!("{}:{}", host, port);
    let addrs: Vec<SocketAddr> = tokio::net::lookup_host(&lookup)
        .await
        .map_err(|e| {
            tracing::debug!(host = %host, error = %e, "DNS resolution failed — blocking");
        })?
        .collect();

    if addrs.is_empty() {
        return Err(());
    }

    // Reject if ANY resolved address is private
    for addr in &addrs {
        if is_private_ip(addr.ip()) {
            tracing::warn!(
                host = %host,
                resolved_ip = %addr.ip(),
                "DNS resolved to private/reserved IP — blocking request"
            );
            return Err(());
        }
    }

    // Return first validated address for pinning
    Ok(addrs[0])
}

/// Build an HTTP client pinned to a validated IP address.
///
/// The `resolve()` call ensures reqwest connects to the pre-validated IP
/// instead of performing its own DNS resolution, closing the TOCTOU gap.
/// Redirects are disabled — callers must follow redirects manually via
/// `fetch_with_redirect_validation` to DNS-validate each hop.
fn build_pinned_http_client(
    host: &str,
    pinned_addr: SocketAddr,
    user_agent: &str,
) -> Result<reqwest::Client, reqwest::Error> {
    reqwest::Client::builder()
        .timeout(FETCH_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .resolve(host, pinned_addr)
        .user_agent(user_agent)
        .build()
}

/// Build a non-pinned HTTP client (for IP-literal URLs that don't need DNS).
/// Redirects are disabled — callers must follow redirects manually via
/// `fetch_with_redirect_validation` to DNS-validate each hop.
fn build_http_client(user_agent: &str) -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(FETCH_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .user_agent(user_agent)
        .build()
        .unwrap_or_default()
}

/// Performs an HTTP GET request, manually following redirects with full DNS
/// validation at each hop. This closes the SSRF gap where redirected domains
/// could resolve to private IPs without being checked.
async fn fetch_with_redirect_validation(
    initial_url: &str,
    accept_header: &str,
    user_agent: &str,
) -> Result<reqwest::Response, StatusCode> {
    let mut current_url = initial_url.to_string();

    for hop in 0..=MAX_REDIRECT_HOPS {
        if hop == MAX_REDIRECT_HOPS {
            tracing::debug!(url = %current_url, "too many redirects");
            return Err(StatusCode::BAD_GATEWAY);
        }

        // Validate URL at each hop
        if is_private_or_reserved(&current_url) {
            return Err(StatusCode::FORBIDDEN);
        }

        let parsed = url::Url::parse(&current_url).map_err(|_| StatusCode::BAD_REQUEST)?;
        let host = parsed.host_str().ok_or(StatusCode::BAD_REQUEST)?;
        let port = parsed.port_or_known_default().unwrap_or(443);

        // DNS-pinned client for each hop
        let client = if host.parse::<IpAddr>().is_ok() {
            build_http_client(user_agent)
        } else {
            let pinned_addr = resolve_and_validate(host, port)
                .await
                .map_err(|_| StatusCode::FORBIDDEN)?;
            build_pinned_http_client(host, pinned_addr, user_agent)
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        };

        let resp = client
            .get(&current_url)
            .header(header::ACCEPT, accept_header)
            .send()
            .await
            .map_err(|e| {
                tracing::debug!(url = %current_url, error = %e, "fetch failed at redirect hop {}", hop);
                StatusCode::BAD_GATEWAY
            })?;

        if resp.status().is_redirection() {
            let location = resp
                .headers()
                .get(header::LOCATION)
                .and_then(|v| v.to_str().ok())
                .ok_or(StatusCode::BAD_GATEWAY)?;

            // Resolve relative redirect URLs against the current URL
            let next_url = parsed.join(location).map_err(|_| StatusCode::BAD_GATEWAY)?;

            // Only allow http/https schemes
            match next_url.scheme() {
                "http" | "https" => {}
                _ => return Err(StatusCode::FORBIDDEN),
            }

            current_url = next_url.to_string();
            continue;
        }

        return Ok(resp);
    }

    Err(StatusCode::BAD_GATEWAY)
}

/// Reads a response body in chunks up to `max_bytes`, rejecting early if the
/// limit is exceeded. This prevents over-allocating memory for oversized
/// upstream responses (DoS mitigation).
async fn read_body_capped(
    mut resp: reqwest::Response,
    max_bytes: usize,
) -> Result<Vec<u8>, StatusCode> {
    // Fast-path: check Content-Length header if present
    if let Some(cl) = resp.content_length() {
        if cl as usize > max_bytes {
            return Err(StatusCode::BAD_GATEWAY);
        }
    }

    let mut body = Vec::new();
    while let Some(chunk) = resp.chunk().await.map_err(|_| StatusCode::BAD_GATEWAY)? {
        if body.len() + chunk.len() > max_bytes {
            return Err(StatusCode::BAD_GATEWAY);
        }
        body.extend_from_slice(&chunk);
    }

    Ok(body)
}

const PREVIEW_USER_AGENT: &str = "AnnexBot/1.0 (link-preview)";
const IMAGE_USER_AGENT: &str = "Mozilla/5.0 (compatible; AnnexImageProxy/1.0; +https://annex.chat)";

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
        let cache = state
            .preview_cache
            .previews
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(entry) = cache.get(&url) {
            if entry.fetched_at.elapsed() < CACHE_TTL {
                return Ok(Json(entry.data.clone()));
            }
        }
    }

    // Fetch with DNS validation at every redirect hop, preventing SSRF
    // via DNS rebinding on intermediate redirects.
    let resp =
        fetch_with_redirect_validation(&url, "text/html,application/xhtml+xml", PREVIEW_USER_AGENT)
            .await?;

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

    // Stream body with size limit — reject before buffering to prevent
    // memory exhaustion from oversized upstream responses.
    let bytes = read_body_capped(resp, MAX_HTML_BYTES).await?;

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
        let mut cache = state
            .preview_cache
            .previews
            .lock()
            .unwrap_or_else(|e| e.into_inner());
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
        let cache = state
            .preview_cache
            .images
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(entry) = cache.get(&url) {
            if entry.fetched_at.elapsed() < IMAGE_CACHE_TTL {
                return Ok(build_image_response(
                    &entry.content_type,
                    entry.bytes.clone(),
                ));
            }
        }
    }

    // Fetch with DNS validation at every redirect hop, preventing SSRF
    // via DNS rebinding on intermediate redirects.
    let resp = fetch_with_redirect_validation(&url, "image/*", IMAGE_USER_AGENT).await?;

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
    let is_octet_stream_with_image_ext =
        content_type == "application/octet-stream" && url_has_image_extension(&url);

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

    // Stream body with size limit — reject before buffering to prevent
    // memory exhaustion from oversized upstream responses.
    let bytes = read_body_capped(resp, MAX_IMAGE_BYTES).await?;

    let body = bytes;

    // Cache the image
    {
        let mut cache = state
            .preview_cache
            .images
            .lock()
            .unwrap_or_else(|e| e.into_inner());
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
    [
        ".jpg", ".jpeg", ".png", ".gif", ".webp", ".svg", ".ico", ".bmp", ".avif",
    ]
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
    fn blocks_ipv4_mapped_ipv6() {
        // IPv4-mapped IPv6 addresses must be checked against IPv4 private ranges
        let mapped_loopback: IpAddr = "::ffff:127.0.0.1".parse().unwrap();
        assert!(is_private_ip(mapped_loopback));

        let mapped_private: IpAddr = "::ffff:10.0.0.1".parse().unwrap();
        assert!(is_private_ip(mapped_private));

        let mapped_private_192: IpAddr = "::ffff:192.168.1.1".parse().unwrap();
        assert!(is_private_ip(mapped_private_192));

        let mapped_link_local: IpAddr = "::ffff:169.254.169.254".parse().unwrap();
        assert!(is_private_ip(mapped_link_local));

        // Public IPv4-mapped should be allowed
        let mapped_public: IpAddr = "::ffff:8.8.8.8".parse().unwrap();
        assert!(!is_private_ip(mapped_public));
    }

    #[test]
    fn blocks_ipv4_mapped_ipv6_in_url() {
        assert!(is_private_or_reserved("http://[::ffff:127.0.0.1]/admin"));
        assert!(is_private_or_reserved("http://[::ffff:10.0.0.1]/admin"));
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
