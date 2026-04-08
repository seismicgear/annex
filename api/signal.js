const SIGNAL_TTL_MS = 120_000;
const MAX_QUEUE_LENGTH = 128;

/** @type {Map<string, Array<{payload: any, expiresAt: number}>>} */
const signalQueues = globalThis.__annexSignalQueues || new Map();
globalThis.__annexSignalQueues = signalQueues;

function purgeExpired(slug) {
  const queue = signalQueues.get(slug);
  if (!queue) return;

  const now = Date.now();
  const live = queue.filter((item) => item.expiresAt > now);
  if (live.length === 0) {
    signalQueues.delete(slug);
    return;
  }
  signalQueues.set(slug, live);
}

function enqueueSignal(slug, payload) {
  purgeExpired(slug);
  const queue = signalQueues.get(slug) || [];
  if (queue.length >= MAX_QUEUE_LENGTH) {
    queue.shift();
  }
  queue.push({ payload, expiresAt: Date.now() + SIGNAL_TTL_MS });
  signalQueues.set(slug, queue);
}

function dequeueSignal(slug) {
  purgeExpired(slug);
  const queue = signalQueues.get(slug);
  if (!queue || queue.length === 0) {
    return null;
  }
  const next = queue.shift();
  if (queue.length === 0) {
    signalQueues.delete(slug);
  } else {
    signalQueues.set(slug, queue);
  }
  return next?.payload || null;
}

export default async function handler(req, res) {
  if (req.method === 'POST') {
    const { from_server_slug, to_server_slug, session_id, sdp_type, sdp } = req.body || {};

    if (!from_server_slug || !to_server_slug || !session_id || !sdp_type || !sdp) {
      res.status(400).json({ error: 'missing required signaling fields' });
      return;
    }
    if (sdp_type !== 'offer' && sdp_type !== 'answer') {
      res.status(400).json({ error: 'invalid sdp_type' });
      return;
    }

    enqueueSignal(to_server_slug, {
      from_server_slug,
      to_server_slug,
      session_id,
      sdp_type,
      sdp,
      created_at: new Date().toISOString(),
    });

    res.status(202).json({ ok: true });
    return;
  }

  if (req.method === 'GET') {
    const slug = String(req.query.slug || '').trim();
    const waitSecondsRaw = Number(req.query.wait ?? 25);
    const waitSeconds = Number.isFinite(waitSecondsRaw)
      ? Math.min(Math.max(waitSecondsRaw, 1), 90)
      : 25;

    if (!slug) {
      res.status(400).json({ error: 'missing slug query parameter' });
      return;
    }

    const deadline = Date.now() + waitSeconds * 1000;
    while (Date.now() < deadline) {
      const payload = dequeueSignal(slug);
      if (payload) {
        res.status(200).json(payload);
        return;
      }
      await new Promise((resolve) => setTimeout(resolve, 250));
    }

    res.status(204).end();
    return;
  }

  res.setHeader('Allow', 'GET, POST');
  res.status(405).json({ error: 'method not allowed' });
}
