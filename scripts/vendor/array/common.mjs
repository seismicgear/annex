import fs from "node:fs/promises";
import path from "node:path";
import crypto from "node:crypto";
export class ArrayError extends Error {
  constructor(code, message) {
    super(message);
    this.code = code;
  }
}
export const fail = (code, message) => {
  throw new ArrayError(code, message);
};
export const sha256 = (value) =>
  crypto.createHash("sha256").update(value).digest("hex");
export function productId(value) {
  if (typeof value !== "string" || !/^[a-z][a-z0-9-]{1,63}$/.test(value))
    fail("INVALID_PRODUCT", "Choose a recognized application.");
  return value;
}
export function version(value) {
  if (typeof value !== "string" || !/^\d{1,6}\.\d{1,6}\.\d{1,6}$/.test(value))
    fail("INVALID_VERSION", "The release version is invalid.");
  return value;
}
export function compareVersions(a, b) {
  const av = version(a).split(".").map(Number),
    bv = version(b).split(".").map(Number);
  for (let i = 0; i < 3; i++) {
    if (av[i] !== bv[i]) return av[i] > bv[i] ? 1 : -1;
  }
  return 0;
}
export function safeChild(root, relative) {
  if (
    typeof relative !== "string" ||
    relative.length > 240 ||
    relative.includes("\\") ||
    relative.includes(":") ||
    relative.startsWith("/") ||
    relative
      .split("/")
      .some(
        (p) =>
          !p ||
          p === "." ||
          p === ".." ||
          /[. ]$/.test(p) ||
          /^(con|prn|aux|nul|com[0-9]|lpt[0-9])(?:\.|$)/i.test(p),
      )
  )
    fail("UNSAFE_PATH", "The package contains an unsafe file path.");
  const dest = path.resolve(root, relative);
  if (!dest.startsWith(path.resolve(root) + path.sep))
    fail("UNSAFE_PATH", "The package path leaves its installation directory.");
  return dest;
}
export async function atomicJson(file, value) {
  await fs.mkdir(path.dirname(file), { recursive: true });
  const temp = file + "." + crypto.randomUUID() + ".tmp";
  const handle = await fs.open(temp, "wx", 0o600);
  try {
    await handle.writeFile(JSON.stringify(value, null, 2));
    await handle.sync();
  } finally {
    await handle.close();
  }
  await fs.rename(temp, file);
}
export async function readJson(file, fallback) {
  try {
    return JSON.parse(await fs.readFile(file, "utf8"));
  } catch (e) {
    if (e.code === "ENOENT") return fallback;
    throw e;
  }
}
export function trustedUrl(value, hosts, { loopback = false } = {}) {
  let u;
  try {
    u = new URL(value);
  } catch {
    fail("INVALID_URL", "The service returned an invalid address.");
  }
  if (u.username || u.password || u.hash)
    fail("INVALID_URL", "The service address is not allowed.");
  if (loopback && u.protocol === "http:" && u.hostname === "127.0.0.1")
    return u;
  if (
    u.protocol !== "https:" ||
    (u.port && u.port !== "443") ||
    !hosts.includes(u.hostname)
  )
    fail(
      "UNTRUSTED_HOST",
      "The download host is not approved for this release.",
    );
  return u;
}
export async function readBounded(response, max = 2 * 1024 * 1024) {
  if (!response.ok)
    fail(
      "SERVICE_UNAVAILABLE",
      `The service could not complete the request (${response.status}). Try again later.`,
    );
  return readBoundedBody(response.body, max);
}
export async function readBoundedBody(body, max = 2 * 1024 * 1024) {
  let total = 0;
  const chunks = [];
  for await (const chunk of body) {
    total += chunk.length;
    if (total > max)
      fail("TOO_LARGE", "The service response exceeded its size limit.");
    chunks.push(chunk);
  }
  return Buffer.concat(chunks);
}
export async function fetchTrusted(
  url,
  { hosts, loopback = false, signal, headers = {}, max = 2 * 1024 * 1024 } = {},
) {
  let current = trustedUrl(url, hosts, { loopback });
  for (let i = 0; i < 4; i++) {
    const response = await fetch(current, {
      redirect: "manual",
      signal,
      headers,
    });
    if ([301, 302, 303, 307, 308].includes(response.status)) {
      const next = trustedUrl(
        new URL(response.headers.get("location"), current).href,
        hosts,
        { loopback },
      );
      if (next.origin !== current.origin) headers = {};
      current = next;
      continue;
    }
    return readBounded(response, max);
  }
  fail("REDIRECT_LIMIT", "The service redirected too many times.");
}
export function publicError(e) {
  const known = {
    ENOSPC: "There is not enough free disk space. Free some space and retry.",
    EACCES:
      "Array could not access the installation folder. Check its permissions.",
    EPERM:
      "The operation was blocked by Windows. Close the application if it is running and retry.",
    ECONNREFUSED:
      "The service is unavailable. Your installed applications are still available.",
  };
  return e instanceof ArrayError
    ? { code: e.code, message: e.message }
    : {
        code:
          e.name === "AbortError" ? "CANCELLED" : e.code || "OPERATION_FAILED",
        message:
          e.name === "AbortError"
            ? "The operation was cancelled. You can retry safely."
            : known[e.code] ||
              "The operation could not finish. Your previous installation has been kept where possible. Try again.",
      };
}
