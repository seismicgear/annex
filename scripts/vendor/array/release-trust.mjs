import crypto from "node:crypto";
import os from "node:os";
import {
  fail,
  productId,
  version,
  compareVersions,
  trustedUrl,
  sha256,
} from "./common.mjs";
/** Signed payload bytes are verified before parsing or using any release data. */
export function verifyRelease(
  envelope,
  {
    keys,
    product,
    platform = process.platform,
    arch = process.arch,
    channel = "stable",
    currentVersion = null,
    highWater = 0,
    highWaterDigest = null,
    arrayVersion = "0.1.0",
    now = Date.now(),
    hosts = [],
    loopback = false,
    runtimeOSVersion = os.release(),
  },
) {
  if (
    !envelope ||
    envelope.schemaVersion !== 1 ||
    typeof envelope.keyId !== "string" ||
    typeof envelope.payload !== "string" ||
    typeof envelope.signature !== "string" ||
    envelope.payload.length > 256000
  )
    fail("INVALID_MANIFEST", "The release metadata is invalid.");
  const key = keys[envelope.keyId];
  if (
    !key ||
    !key.products.includes(product) ||
    !key.channels.includes(channel)
  )
    fail("UNTRUSTED_RELEASE", "This release has no trusted publisher key.");
  const payload = Buffer.from(envelope.payload, "base64"),
    signature = Buffer.from(envelope.signature, "base64");
  if (
    signature.length !== 64 ||
    !crypto.verify(null, payload, key.publicKey, signature)
  )
    fail(
      "BAD_SIGNATURE",
      "The release signature could not be verified. Nothing was installed.",
    );
  let m;
  try {
    m = JSON.parse(payload);
  } catch {
    fail("INVALID_MANIFEST", "The signed release document is unreadable.");
  }
  if (
    m.schemaVersion !== 1 ||
    productId(m.product) !== product ||
    m.platform !== platform ||
    m.arch !== arch ||
    m.channel !== channel
  )
    fail(
      "WRONG_RELEASE",
      "This release is for a different application, channel, or computer.",
    );
  version(m.version);
  version(m.minArrayVersion);
  if (compareVersions(arrayVersion, m.minArrayVersion) < 0)
    fail(
      "ARRAY_UPDATE_REQUIRED",
      "Update Array before installing this release.",
    );
  if (
    !Number.isSafeInteger(m.sequence) ||
    m.sequence < 1 ||
    m.sequence < highWater
  )
    fail("REPLAYED_RELEASE", "An older release record was rejected.");
  const digest = sha256(payload);
  if (m.sequence === highWater && highWaterDigest && digest !== highWaterDigest)
    fail(
      "REPLAYED_RELEASE",
      "The publisher changed a previously observed release sequence.",
    );
  if (currentVersion && compareVersions(m.version, currentVersion) < 0)
    fail(
      "DOWNGRADE_BLOCKED",
      "This release would downgrade the installed application. Use product-supported recovery instead.",
    );
  const published = Date.parse(m.publishedAt),
    expires = Date.parse(m.expiresAt);
  if (
    !Number.isFinite(published) ||
    !Number.isFinite(expires) ||
    published > now + 300000 ||
    expires < now ||
    expires <= published
  )
    fail(
      "EXPIRED_RELEASE",
      "The release record has expired or has an invalid publication time. Check again online.",
    );
  if (m.minOSVersion) {
    const actual = String(runtimeOSVersion || '0.0.0')
      .split(".")
      .slice(0, 3)
      .map((n) => Number(n) || 0);
    const minimum = String(m.minOSVersion).split(".").map(Number);
    if (
      minimum.length !== 3 ||
      minimum.some((n) => !Number.isSafeInteger(n) || n < 0)
    )
      fail("INVALID_MANIFEST", "The OS compatibility declaration is invalid.");
    for (let i = 0; runtimeOSVersion !== null && i < 3; i++) {
      if (actual[i] > minimum[i]) break;
      if (actual[i] < minimum[i])
        fail("INCOMPATIBLE_OS", "This release needs a newer operating system.");
    }
  }
  const a = m.artifact;
  if (
    !a ||
    !/^[a-f0-9]{64}$/.test(a.sha256) ||
    !Number.isSafeInteger(a.size) ||
    a.size < 1 ||
    a.size > 16 * 1024 ** 3 ||
    !["bundle", "nsis", "msi", "appimage", "deb", "pkg", "dmg"].includes(a.kind)
  )
    fail("INVALID_ARTIFACT", "The release artifact is incomplete.");
  trustedUrl(a.url, hosts, { loopback });
  if (
    typeof a.executable !== "string" ||
    !/^[a-f0-9]{64}$/.test(a.executableSha256 || "")
  )
    fail("INVALID_ARTIFACT", "The installed application identity is missing.");
  return { ...m, payloadDigest: digest };
}
