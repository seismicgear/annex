import test from 'node:test';
import assert from 'node:assert/strict';
import { checkReleaseWriteEligibility } from '../check-release-write-eligibility.mjs';

const source = 'a'.repeat(40);
const input = { repository: 'seismicgear/annex', tag: 'v0.1.0', commit: source, token: 'test-token' };
const draft = { id: 42, tag_name: input.tag, draft: true, published_at: null, target_commitish: source };
const response = (status, body) => new Response(JSON.stringify(body), { status });
function reader(replies, calls = []) {
  return async (url, options) => {
    calls.push({ url, options });
    assert.ok(replies.length, 'Unexpected API request');
    return replies.shift();
  };
}
const repo = () => response(200, { full_name: input.repository });
test('allows only an authenticated missing release or exact-source draft', async () => {
  for (const [reply, state] of [[response(404, {}), 'absent'], [response(200, draft), 'draft']]) {
    const calls = [];
    const result = await checkReleaseWriteEligibility({ ...input, fetchImpl: reader([repo(), reply, ...(state === 'absent' ? [response(200, [])] : [])], calls) });
    assert.equal(result.state, state);
    assert.equal(calls.length, state === 'absent' ? 3 : 2);
    assert.equal(calls[1].options.headers.Authorization, 'Bearer test-token');
    assert.equal(calls[1].options.redirect, 'error');
    assert.equal(calls[1].url, 'https://api.github.com/repos/seismicgear/annex/releases/tags/v0.1.0');
  }
});
test('published releases cannot be rewritten even when their source matches', async () => {
  for (const release of [{ ...draft, draft: false }, { ...draft, published_at: '2026-10-15T00:00:00Z' }, { ...draft, draft: null }]) {
    await assert.rejects(checkReleaseWriteEligibility({ ...input, fetchImpl: reader([repo(), response(200, release)]) }), /published or ambiguous/);
  }
});
test('refuses mismatched source and release identity', async () => {
  for (const change of [{ target_commitish: 'main' }, { target_commitish: 'b'.repeat(40) }, { tag_name: 'v0.2.0' }, { id: 0 }]) {
    await assert.rejects(checkReleaseWriteEligibility({ ...input, fetchImpl: reader([repo(), response(200, { ...draft, ...change })]) }), /different release source|identity mismatch/);
  }
});
test('repository access and all uncertain HTTP outcomes fail closed', async () => {
  for (const status of [401, 403, 404, 429, 500]) {
    const calls = [];
    await assert.rejects(checkReleaseWriteEligibility({ ...input, fetchImpl: reader([response(status, {})], calls) }), /Cannot verify release repository/);
    assert.equal(calls.length, 1);
  }
  for (const status of [401, 403, 429, 500]) {
    await assert.rejects(checkReleaseWriteEligibility({ ...input, fetchImpl: reader([repo(), response(status, {})]) }), /Cannot verify release state/);
  }
});
test('rejects redirects, transport failure, malformed JSON and wrong repository', async () => {
  await assert.rejects(checkReleaseWriteEligibility({ ...input, fetchImpl: async () => { throw new Error('sensitive transport detail'); } }), /request failed/);
  await assert.rejects(checkReleaseWriteEligibility({ ...input, fetchImpl: reader([response(200, { full_name: 'someone/else' })]) }), /identity mismatch/);
  await assert.rejects(checkReleaseWriteEligibility({ ...input, fetchImpl: reader([repo(), new Response('{', { status: 200 })]) }), SyntaxError);
});
test('invalid input never reaches network', async () => {
  for (const change of [{ repository: '../annex' }, { repository: 'seismicgear/..' }, { repository: './annex' }, { tag: 'refs/tags/v0.1.0' }, { commit: 'main' }, { token: '' }]) {
    let calls = 0;
    await assert.rejects(checkReleaseWriteEligibility({ ...input, ...change, fetchImpl: () => { ++calls; throw new Error('Unexpected network request'); } }));
    assert.equal(calls, 0, 'Invalid input must be rejected before any request');
  }
});
test('exact dry-run names are supported without an alternate mutation path', async () => {
  const tag = `dry-run-${source}`;
  const result = await checkReleaseWriteEligibility({ ...input, tag, fetchImpl: reader([repo(), response(200, { ...draft, tag_name: tag })]) });
  assert.equal(result.state, 'draft');
});
test('a tag-endpoint404 still validates an untagged draft from the authenticated collection', async () => {
  const result = await checkReleaseWriteEligibility({ ...input, fetchImpl: reader([repo(), response(404, {}), response(200, [draft])]) });
  assert.equal(result.state, 'draft');
  assert.equal(result.releaseId, draft.id);
  for (const change of [{ draft: false }, { target_commitish: 'main' }]) {
    await assert.rejects(checkReleaseWriteEligibility({ ...input, fetchImpl: reader([repo(), response(404, {}), response(200, [{ ...draft, ...change }])]) }));
  }
});
test('draft enumeration follows every page and refuses errors or ambiguous matches', async () => {
  const firstPage = Array.from({ length: 100 }, (_, i) => ({ ...draft, id: i + 1, tag_name: `v9.0.${i}` }));
  const calls = [];
  const result = await checkReleaseWriteEligibility({ ...input, fetchImpl: reader([repo(), response(404, {}), response(200, firstPage), response(200, [draft])], calls) });
  assert.equal(result.state, 'draft');
  assert.ok(calls[3].url.endsWith('page=2'));
  for (const listed of [response(403, {}), response(200, {}), response(200, [draft, draft]), response(200, [null])]) {
    await assert.rejects(checkReleaseWriteEligibility({ ...input, fetchImpl: reader([repo(), response(404, {}), listed]) }));
  }
});
