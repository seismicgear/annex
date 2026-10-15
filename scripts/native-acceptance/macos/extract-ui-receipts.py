"""Export real XCTest attachments through its supported result channel.

The UI runner cannot write into the checkout. No runner or app permission is
changed; XCTest owns its result bundle and exports these original bytes.
"""
import hashlib
import json
from pathlib import Path
import re

root = Path('native-evidence')
attachments = root / 'attachments'
manifest = json.loads((attachments / 'manifest.json').read_text())
events = []
results = []
for test in manifest:
    for row in test['attachments']:
        title = row['suggestedHumanReadableName']
        match = re.match(r'^annex-ui-events-([0-9]+)_', title)
        if match:
            events.append((int(match[1]), row['timestamp'], row))
        if title.startswith('annex-native-invite-result_'):
            results.append(row)
assert len(results) <= 1, 'Ambiguous native result attachments'
selected = []
if events:
    selected.append(('ui-events.json', max(events, key=lambda value: value[:2])[2]))
if results:
    selected.append(('native-invite-result.json', results[0]))
receipts = []
for name, row in selected:
    filename = row['exportedFileName']
    assert filename == Path(filename).name and filename not in ('.', '..')
    content = (attachments / filename).read_bytes()
    document = json.loads(content)
    assert isinstance(document, list if name == 'ui-events.json' else dict)
    (root / name).write_bytes(content)
    receipts.append(dict(name=name, exported_file=filename,
                         original_attachment_title=row['suggestedHumanReadableName'],
                         sha256=hashlib.sha256(content).hexdigest()))
(root / 'xctest-receipt-export.json').write_text(json.dumps(receipts, indent=2) + '\n')
print(json.dumps({'exported_original_xctest_receipts': len(receipts),
                  'native_success_attachment_present': bool(results)}))
