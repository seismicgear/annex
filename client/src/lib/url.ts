/**
 * Shared URL normalization for server connection flows.
 *
 * Used by StartupModeSelector, ServerHub, and anywhere a user-entered
 * server address needs to be resolved to a full URL.
 */

/** Hostnames/IPs that should default to http:// instead of https://. */
const LOCAL_HOSTS = new Set(['localhost', '127.0.0.1', '::1', '[::1]']);

/** Check if a hostname looks like a private/LAN IP (RFC 1918, link-local). */
function isPrivateIp(host: string): boolean {
  // IPv4 private ranges
  if (/^10\./.test(host)) return true;
  if (/^172\.(1[6-9]|2\d|3[01])\./.test(host)) return true;
  if (/^192\.168\./.test(host)) return true;
  // IPv4 link-local
  if (/^169\.254\./.test(host)) return true;
  return false;
}

/**
 * Normalize a user-entered server URL to a full URL with protocol.
 *
 * - Preserves explicit http:// or https://
 * - Defaults to http:// for localhost, 127.0.0.1, and private/LAN IPs
 * - Defaults to https:// for public hostnames
 *
 * Throws if the result is not a valid URL or uses a non-HTTP protocol.
 */
export function normalizeServerUrl(input: string): string {
  let url = input.trim();
  if (!url) throw new Error('Empty URL');

  // If protocol is already specified, validate and return
  if (/^https?:\/\//i.test(url)) {
    const parsed = new URL(url);
    if (!['http:', 'https:'].includes(parsed.protocol)) {
      throw new Error('Only http and https URLs are supported.');
    }
    return url;
  }

  // Strip any accidental protocol-like prefix
  if (/^[a-z]+:\/\//i.test(url)) {
    throw new Error('Only http and https URLs are supported.');
  }

  // Extract hostname (before port or path) to decide default scheme
  const hostPart = url.split(/[:/]/)[0].toLowerCase();
  const useHttp = LOCAL_HOSTS.has(hostPart) || isPrivateIp(hostPart);
  const scheme = useHttp ? 'http' : 'https';
  const normalized = `${scheme}://${url}`;

  // Validate the result
  const parsed = new URL(normalized);
  if (!['http:', 'https:'].includes(parsed.protocol)) {
    throw new Error('Only http and https URLs are supported.');
  }

  return normalized;
}
