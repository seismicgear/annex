"""Correlate actual XCTest UI, passive HTTP and read-only database evidence."""
import hashlib
import json
from pathlib import Path

root = Path('native-evidence')
result = json.loads((root / 'native-invite-result.json').read_text())
fixture = json.loads((root / 'fixture.json').read_text())
download = json.loads((root / 'published-dmg-verification.json').read_text())
signature = json.loads((root / 'installed-signature-verification.json').read_text())
observations = [json.loads(line) for line in (root / 'database-observations.jsonl').read_text().splitlines()]
http = [json.loads(line) for line in (root / 'http-observations.jsonl').read_text().splitlines()]
events = json.loads((root / 'ui-events.json').read_text())
assert result['passed'] and result['explicit_join'] and result['cold_launch_by_public_link']
assert result['independent_founder_received_message'] and result['warm_delivery_same_process']
assert not result['native_app_launched_directly_by_test']
assert result['server_slug'] == fixture['slug']
assert result['destination'] == fixture['origin']
assert fixture['source_commit'] == download['source_commit']
assert signature['strict_bundle_verification']
assert signature['version'] == download['version']
assert observations[0]['admitted_identities'] == []
latest = observations[-1]
assert len(latest['admitted_identities']) == 2
assert len(latest['invites']) == 1
assert latest['servers'][0]['slug'] == result['server_slug']
message_sha = hashlib.sha256(result['message'].encode()).hexdigest()
messages = [row for row in latest['messages'] if row['content_sha256'] == message_sha]
assert len(messages) == 1
assert messages[0]['server_id'] == latest['servers'][0]['id']
founder = next(row['admitted_identities'][0]['pseudonym_sha256'] for row in observations
              if len(row['admitted_identities']) == 1)
recipients = [row['pseudonym_sha256'] for row in latest['admitted_identities']
              if row['pseudonym_sha256'] != founder]
assert len(recipients) == 1
assert messages[0]['sender_pseudonym_sha256'] == recipients[0]
assert any(row['path'] == '/api/invites/redeem' and row['status'] == 200
           and row['request_origin'] == 'tauri://localhost' for row in http)
assert sum(row['path'] == '/api/zk/verify-membership' and row['status'] == 200 for row in http) == 2
phases = {row['phase'] for row in events}
required = {'founder-generated-and-copied-invite', 'actual-monolithannex-public-invite-page',
            'published-app-cold-launched-by-public-invite', 'exact-destination-awaiting-explicit-join',
            'published-app-joined-correct-destination', 'independent-founder-received-native-message',
            'warm-public-invite-same-native-process'}
assert required <= phases
receipt = {'passed': True, 'source_commit': download['source_commit'], 'asset_sha256': download['sha256'],
           'journeys': 1, 'destination_slug': result['server_slug'], 'recipient_message_sha256': message_sha,
           'native_signature': signature['signing'], 'notarized': False,
           'release_was_draft': json.loads((root / 'release.json').read_text())['isDraft'],
           'browser_download_gatekeeper_acceptance': False}
(root / 'correlated-acceptance.json').write_text(json.dumps(receipt, indent=2) + '\n')
print(json.dumps(receipt, indent=2))
