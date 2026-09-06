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
 * Refuse a server address that carries credentials.
 *
 * `https://annex.trusted.example@evil.com` has host `evil.com` — the part a
 * reader takes for the hostname is the username. Every place this value is
 * shown shows the string (the invite banner, the server hub row, "Could not
 * reach server at …"), and every request goes to the host, so the address a
 * user approves and the server they reach are different machines.
 *
 * Refused rather than silently rewritten: Annex never sends credentials in a
 * URL, so there is nothing here to support, and rewriting would hand back an
 * address the user did not type.
 */
function rejectCredentials(parsed: URL): void {
  if (parsed.username || parsed.password) {
    throw new Error('Server addresses must not contain a username or password.');
  }
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
  const url = input.trim();
  if (!url) throw new Error('Empty URL');

  // If protocol is already specified, validate and return
  if (/^https?:\/\//i.test(url)) {
    const parsed = new URL(url);
    if (!['http:', 'https:'].includes(parsed.protocol)) {
      throw new Error('Only http and https URLs are supported.');
    }
    rejectCredentials(parsed);
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
  // Also on this branch: `annex.trusted.example@evil.com` has no scheme, gets
  // one prepended, and lands in exactly the same shape.
  rejectCredentials(parsed);

  return normalized;
}

/**
 * Turn a `normalizeServerUrl` rejection into a sentence for the user.
 *
 * Two dialogs ask for a server address — `StartupModeSelector` on first run
 * and `ServerHub`'s Join-a-Server — and both used to answer a bad one with a
 * bare "Invalid URL format.", naming nothing the user typed and describing no
 * address that would work. Both were fixed, separately, to the same wording;
 * this is that wording, in one place, so the next dialog that asks the
 * question inherits it instead of inventing a third answer.
 *
 * The thrown message is the interesting half — `normalizeServerUrl` says
 * "Only http and https URLs are supported." when it can tell why. When the
 * `URL` constructor is what refused, its message is a bare "Invalid URL"
 * (Chrome dresses it up as "Failed to construct 'URL': Invalid URL"), which
 * restates the failure rather than explaining it, so it is replaced.
 */
export function describeServerUrlError(input: string, err: unknown): string {
  const raw = err instanceof Error ? err.message : '';
  const uninformative = !raw || /invalid url\.?$/i.test(raw.trim());
  const why = uninformative ? 'It could not be parsed.' : raw;
  return (
    `Could not use "${input.trim()}" as a server address. ${why} ` +
    'Enter a hostname like annex.example.com, optionally with a port.'
  );
}
