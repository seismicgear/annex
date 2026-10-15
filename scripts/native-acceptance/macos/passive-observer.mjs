import fs from 'node:fs';
import path from 'node:path';
import { createHash } from 'node:crypto';
import { DatabaseSync } from 'node:sqlite';
const [database, directory, mode] = process.argv.slice(2);
if (process.platform !== 'darwin' || process.env.GITHUB_ACTIONS !== 'true' || database !== path.join(process.env.RUNNER_TEMP, 'annex-native-destination', 'annex.db') || !path.isAbsolute(directory) || path.basename(directory) !== 'native-evidence') throw new Error('Observer restricted to ephemeral native Mac destination.');
const db = new DatabaseSync(database, { readOnly: true });
db.exec('PRAGMA query_only=ON; PRAGMA busy_timeout=1000');
const all = sql => db.prepare(sql).all();
const sha = value => createHash('sha256').update(String(value)).digest('hex');
let previous = '';
let lastRecord = 0;
function observe() {
  try {
    const sample = {
      servers: all('SELECT id,slug,label,public_url,policy_json FROM servers').map(row => ({...row, policy_json: JSON.parse(row.policy_json)})),
      registered_commitments: all('SELECT COUNT(*) AS count FROM vrp_leaves')[0].count,
      admitted_identities: all('SELECT id,server_id,pseudonym_id,participant_type,active,token_epoch,created_at,updated_at FROM platform_identities').map(({pseudonym_id,...row}) => ({...row,pseudonym_sha256:sha(pseudonym_id)})),
      invites: all('SELECT id,server_id,code,max_uses,use_count,expires_at,created_at FROM invite_codes').map(({code,...row})=>({...row,code_sha256:sha(code)})),
      channels: all('SELECT server_id,channel_id,name FROM channels'),
      memberships: all('SELECT server_id,channel_id,pseudonym_id FROM channel_members').map(({pseudonym_id,...row})=>({...row,pseudonym_sha256:sha(pseudonym_id)})),
      messages: all('SELECT server_id,channel_id,message_id,sender_pseudonym,content,created_at FROM messages ORDER BY id').map(({sender_pseudonym,content,...row})=>({...row,sender_pseudonym_sha256:sha(sender_pseudonym),content_sha256:sha(content),content_bytes:Buffer.byteLength(content)})),
      limitation:'SQLite has no session table. Session issuance is counted separately from passive HTTP responses; active session count is not inferred.'
    };
    const serialized = JSON.stringify(sample);
    if (serialized !== previous || Date.now() - lastRecord >= 5000) {
      fs.appendFileSync(path.join(directory,'database-observations.jsonl'),JSON.stringify({observed_at:new Date().toISOString(),read_only:true,...sample})+'\n');
      previous = serialized;
      lastRecord = Date.now();
    }
    return true;
  } catch (error) { fs.appendFileSync(path.join(directory,'observer-errors.jsonl'),JSON.stringify({at:new Date().toISOString(),error:String(error)})+'\n'); return false; }
}
const firstObservationSucceeded = observe();
if (mode === '--once') { db.close(); process.exit(firstObservationSucceeded ? 0 : 1); }
const interval = setInterval(observe,500);
function stop(){clearInterval(interval);observe();db.close();process.exit(0)}
process.on('SIGTERM',stop);process.on('SIGINT',stop);
