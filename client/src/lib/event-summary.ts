/**
 * Human-readable summaries for public event-log entries.
 *
 * The event log is the server's signed, hash-chained audit trail, and the
 * Detail column was rendering `JSON.stringify(payload).slice(0, 80)` — the
 * raw payload, cut at 80 characters regardless of where that landed. Two
 * things were wrong with that:
 *
 *   1. It told an operator nothing. Twelve of the thirteen payload variants
 *      lead with `{"event":"IDENTITY_VERIFIED",` — the event name, already
 *      shown verbatim in the Type column immediately to the left — and the
 *      80-character budget was then spent on an id whose truncated form is
 *      already in the Entity column. The only genuinely new field in the
 *      payload usually fell off the end.
 *   2. The cut landed mid-token, so rows ended in things like
 *      `"commitment_hex":"a3f2b1` — visibly broken JSON that reads as
 *      corruption rather than truncation.
 *
 * So each variant gets a summary of what the *other* fields say: the topic a
 * proof was verified against, the role a registrant claimed, which remote a
 * federation event concerns, what an alignment changed from and to. Ids are
 * deliberately omitted — the Entity column already carries them, and
 * repeating a 64-character hex twice per row is what pushed the useful part
 * off the end in the first place.
 */

/** Payload shape shared by every event: serde tags the variant as `event`. */
type Payload = Record<string, unknown>;

const ROLE_LABELS: Record<number, string> = {
  1: 'Human',
  2: 'AI agent',
  3: 'Collective',
  4: 'Bridge',
  5: 'Service',
};

/** Renders a SCREAMING_SNAKE_CASE enum as prose: `FULLY_ALIGNED` → `Fully aligned`. */
function humanize(value: unknown): string {
  if (typeof value !== 'string' || value === '') return '';
  const spaced = value.replace(/_/g, ' ').toLowerCase();
  return spaced.charAt(0).toUpperCase() + spaced.slice(1);
}

function str(payload: Payload, key: string): string {
  const v = payload[key];
  return typeof v === 'string' ? v : '';
}

/**
 * Fallback for a payload this build does not recognise — a newer server
 * emitting an event type the client predates, most likely.
 *
 * Still not a raw JSON dump: scalar fields are listed as `key: value` pairs,
 * skipping the `event` tag (duplicated by the Type column) and anything
 * id-shaped (duplicated by the Entity column). Objects and arrays are
 * summarised by shape rather than expanded, so one nested blob cannot crowd
 * out every other field.
 */
function describeUnknown(payload: Payload): string {
  const parts: string[] = [];
  for (const [key, value] of Object.entries(payload)) {
    if (key === 'event') continue;
    if (/(^|_)(id|hex)$/.test(key)) continue;
    if (value === null || value === undefined) continue;
    if (typeof value === 'object') {
      parts.push(`${key}: ${Array.isArray(value) ? `${value.length} items` : 'object'}`);
    } else {
      parts.push(`${key}: ${String(value)}`);
    }
  }
  return parts.join(' · ');
}

/**
 * Summarise one event-log payload for the Detail column.
 *
 * `payloadJson` is the raw `payload_json` column. Returns an empty string when
 * there is genuinely nothing to add beyond the Type and Entity columns — the
 * caller renders an em dash — rather than padding the row with filler.
 */
export function summarizeEventPayload(payloadJson: string): string {
  let payload: Payload;
  try {
    const parsed: unknown = JSON.parse(payloadJson);
    if (typeof parsed !== 'object' || parsed === null) return String(parsed);
    payload = parsed as Payload;
  } catch {
    // Not JSON at all. Show it, but bounded, and say it was cut rather than
    // ending mid-token.
    const raw = payloadJson.trim();
    return raw.length > 80 ? `${raw.slice(0, 79)}…` : raw;
  }

  const event = str(payload, 'event');
  const alignment = () => humanize(payload.alignment_status);
  const previous = () => humanize(payload.previous_status);

  switch (event) {
    case 'IDENTITY_REGISTERED': {
      const code = payload.role_code;
      const role = typeof code === 'number' ? ROLE_LABELS[code] : undefined;
      return role ? `Role: ${role}` : `Role code ${String(code ?? '?')}`;
    }
    case 'IDENTITY_VERIFIED':
    case 'PSEUDONYM_DERIVED': {
      const topic = str(payload, 'topic');
      return topic ? `Topic: ${topic}` : '';
    }
    case 'NODE_ADDED': {
      const nodeType = humanize(payload.node_type);
      return nodeType ? `Joined as ${nodeType}` : 'Joined';
    }
    case 'NODE_PRUNED':
      return 'Pruned for inactivity';
    case 'NODE_REACTIVATED':
      return 'Reactivated after inactivity';

    case 'FEDERATION_ESTABLISHED': {
      const remote = str(payload, 'remote_url');
      const status = alignment();
      return status ? `${remote} · ${status}` : remote;
    }
    case 'FEDERATION_REALIGNED': {
      const remote = str(payload, 'remote_url');
      const from = previous();
      const to = alignment();
      return from && to ? `${remote} · ${from} → ${to}` : `${remote} · ${to || from}`;
    }
    case 'FEDERATION_SEVERED': {
      const remote = str(payload, 'remote_url');
      const reason = str(payload, 'reason');
      return reason ? `${remote} · ${reason}` : remote;
    }

    case 'AGENT_CONNECTED': {
      const status = alignment();
      return status ? `Connected · ${status}` : 'Connected';
    }
    case 'AGENT_REALIGNED': {
      const from = previous();
      const to = alignment();
      return from && to ? `${from} → ${to}` : to || from;
    }
    case 'AGENT_DISCONNECTED': {
      const reason = str(payload, 'reason');
      return reason ? `Disconnected · ${reason}` : 'Disconnected';
    }

    case 'MODERATION_ACTION': {
      // The only variant carrying prose written for a human. Prefer it, and
      // fall back to the action verb when a caller left it blank.
      const description = str(payload, 'description');
      if (description) return description;
      const action = humanize(payload.action_type);
      return action || 'Moderation action';
    }

    default:
      return describeUnknown(payload);
  }
}
