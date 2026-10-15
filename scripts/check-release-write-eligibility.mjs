#!/usr/bin/env node
// A tag created by draft promotion can trigger another build. That build must
// never turn the already published release back into a draft or replace it.
import path from 'node:path';
import { fileURLToPath } from 'node:url';

export async function checkReleaseWriteEligibility({ repository, tag, commit, token, fetchImpl = fetch }) {
  if (!/^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/.test(repository ?? '') || repository.split('/').some(part => part === '.' || part === '..')) throw new Error('Invalid release repository.');
  if (!/^(?:v[0-9][0-9A-Za-z.+-]*|dry-run-[a-f0-9]{40})$/.test(tag ?? '')) throw new Error('Invalid release tag.');
  if (!/^[a-f0-9]{40}$/.test(commit ?? '')) throw new Error('An exact release source commit is required.');
  if (typeof token !== 'string' || !token.trim()) throw new Error('An authenticated release read is required.');
  const base = `https://api.github.com/repos/${repository}`;
  async function request(url) {
    try {
      return await fetchImpl(url, { headers: { Accept: 'application/vnd.github+json',
        Authorization: `Bearer ${token}`, 'X-GitHub-Api-Version': '2022-11-28',
        'User-Agent': 'annex-release-write-eligibility' }, redirect: 'error', signal: AbortSignal.timeout(30_000) });
    } catch { throw new Error('Release eligibility request failed; no release changes are permitted.'); }
  }
  // Confirm repository visibility first, so an inaccessible repository is not
  // mistaken for a missing tag release.
  const repoResponse = await request(base);
  if (repoResponse.status !== 200) throw new Error(`Cannot verify release repository (HTTP ${repoResponse.status}).`);
  const repo = await repoResponse.json();
  if (typeof repo.full_name !== 'string' || repo.full_name.toLowerCase() !== repository.toLowerCase()) throw new Error('Release repository identity mismatch.');
  const response = await request(`${base}/releases/tags/${encodeURIComponent(tag)}`);
  let release;
  if (response.status === 404) {
    // GitHub's tag endpoint omits drafts whose tag has not been created. Its
    // authenticated collection is required before treating that 404 as absence.
    const matches = [];
    let complete = false;
    for (let page = 1; page <= 100; ++page) {
      const listed = await request(`${base}/releases?per_page=100&page=${page}`);
      if (listed.status !== 200) throw new Error(`Cannot enumerate release drafts (HTTP ${listed.status}).`);
      const releases = await listed.json();
      if (!Array.isArray(releases) || releases.length > 100 || releases.some(item => !item || typeof item.tag_name !== 'string')) throw new Error('Invalid release enumeration.');
      matches.push(...releases.filter(item => item.tag_name === tag));
      if (releases.length < 100) { complete = true; break; }
    }
    if (!complete || matches.length > 1) throw new Error('Incomplete or ambiguous release enumeration.');
    if (!matches.length) return { repository, tag, commit, state: 'absent' };
    [release] = matches;
  } else {
    if (response.status !== 200) throw new Error(`Cannot verify release state (HTTP ${response.status}).`);
    release = await response.json();
  }
  if (release.tag_name !== tag || !Number.isSafeInteger(release.id) || release.id <= 0) throw new Error('Release identity mismatch.');
  if (release.draft !== true || release.published_at != null) throw new Error('Refusing to modify an already published or ambiguous release.');
  if (release.target_commitish !== commit) throw new Error('Existing draft names a different release source.');
  return { repository, tag, commit, state: 'draft', releaseId: release.id };
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    const result = await checkReleaseWriteEligibility({ repository: process.env.GITHUB_REPOSITORY,
      tag: process.env.ANNEX_RELEASE_TAG, commit: process.env.GITHUB_SHA, token: process.env.GH_TOKEN });
    console.log(JSON.stringify(result));
  } catch (error) {
    console.error(`[release-write-eligibility] ${error.message}`);
    process.exitCode = 1;
  }
}
