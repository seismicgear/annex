"use strict";

// Retry transport failures without weakening the caller's chain, key, round,
// randomness or BLS signature checks. Every attempt has a response-body deadline.
async function fetchBeaconJson(url, { timeoutMs = 10000, retryDelayMs = 500 } = {}) {
  let failure;
  for (let attempt = 0; attempt < 3; attempt++) {
    let response;
    try {
      response = await fetch(url, { signal: AbortSignal.timeout(timeoutMs) });
      if (response.ok) return await response.json();
    } catch (error) {
      if (error instanceof SyntaxError) throw error;
      failure = error;
    }
    if (response && !response.ok) {
      failure = new Error(`Beacon service returned HTTP ${response.status}`);
      await response.body?.cancel();
      if (![408, 425, 429].includes(response.status) && response.status < 500) throw failure;
    }
    if (attempt < 2) await new Promise(resolve => setTimeout(resolve, retryDelayMs * (attempt + 1)));
  }
  throw failure;
}

module.exports = { fetchBeaconJson };
