// Disposable session maintenance only. No application rows or state are seeded.
import assert from 'node:assert/strict';
import { readFileSync, writeFileSync, statSync, existsSync, realpathSync, unlinkSync } from 'node:fs';
import { dirname, basename, join, relative, isAbsolute } from 'node:path';
import { execFileSync } from 'node:child_process';
import { DatabaseSync } from 'node:sqlite';
const [recordPath, action] = process.argv.slice(2);
assert(recordPath && ['prepare','size','checkpoint','restore'].includes(action));
const record = JSON.parse(readFileSync(recordPath, 'utf8').replace(/^\uFEFF/, ''));
const work = realpathSync(dirname(recordPath));
const sessions = realpathSync(join(work, 'sessions'));
const data = realpathSync(record.data);
const inside = relative(sessions, data);
assert(inside && !inside.startsWith('..') && !isAbsolute(inside), 'Only explicitly isolated acceptance data is allowed');
const name = basename(recordPath, '.json').replace(/^session-/, '');
assert(/^[a-z0-9-]+$/.test(name));
const config = join(data, 'config.toml');
const backup = join(data, 'storage-acceptance-config.original');
const capPath = join(data, 'storage-acceptance-cap.json');
const database = join(data, 'annex.db');
function checkpoint() {
  const db = new DatabaseSync(database);
  try { db.exec('PRAGMA busy_timeout=5000'); const row = db.prepare('PRAGMA wal_checkpoint(TRUNCATE)').get(); assert.equal(Number(row.busy), 0, 'Real WAL maintenance must finish'); }
  finally { db.close(); }
}
function size() {
  return ['', '-wal', '-shm'].reduce((sum, suffix) => sum + (existsSync(database + suffix) ? statSync(database + suffix).size : 0), 0);
}
function restart() {
  execFileSync(process.env.ANNEX_TEST_POWERSHELL ?? 'pwsh.exe', ['-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', join(work, 'restart-session.ps1'), '-Name', name], { stdio: 'ignore', timeout: 60_000 });
}
if (action === 'prepare') {
  checkpoint();
  const original = readFileSync(config, 'utf8');
  // Exclusive creation is the check: a prior backup or a racing replacement
  // cannot be overwritten between an existence check and this write.
  try { writeFileSync(backup, original, { flag: 'wx' }); }
  catch (error) {
    if (error.code === 'EEXIST') throw new Error('Restore the prior controlled storage run before starting another', { cause: error });
    throw error;
  }
  const cap = size() + 2 * 1024 * 1024;
  // Replace only this isolated profile's documented operator configuration.
  // The production probe will measure actual database/WAL growth itself.
  const section = `[storage]\nmax_db_bytes = ${cap}\nwarn_free_bytes = 2048\nblock_free_bytes = 1024\nmaintenance_enabled = false\n`;
  const updated = /^\[storage\]\s*$/m.test(original)
    ? original.replace(/^\[storage\][\s\S]*?(?=^\[|$(?![\s\S]))/m, section)
    : `${original}\n${section}`;
  writeFileSync(config, updated); writeFileSync(capPath, JSON.stringify({ cap })); restart();
} else if (action === 'checkpoint') checkpoint();
else if (action === 'restore' && existsSync(backup)) {
  writeFileSync(config, readFileSync(backup)); restart(); unlinkSync(backup); unlinkSync(capPath);
}
const cap = existsSync(capPath) ? JSON.parse(readFileSync(capPath, 'utf8')).cap : null;
console.log(JSON.stringify({ action, bytes: size(), cap, session: name }));
