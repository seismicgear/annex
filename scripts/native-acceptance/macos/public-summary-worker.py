"""Run the independent read-only browser probe outside XCTest's app container."""
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import time

evidence = Path('native-evidence').resolve()
request_path = evidence / 'public-summary-request.json'
response_path = evidence / 'public-summary-response.json'
probe = Path(__file__).resolve().parent.parent / 'public-host-summary.mjs'
node = shutil.which('node')
assert node and evidence.is_dir()
seen = set()
deadline = time.monotonic() + 20 * 60
while time.monotonic() < deadline:
    try:
        request = json.loads(request_path.read_text())
    except (FileNotFoundError, json.JSONDecodeError):
        time.sleep(0.2)
        continue
    request_id = request.get('id', '')
    if request_id in seen:
        time.sleep(0.2)
        continue
    assert re.fullmatch(r'[A-Fa-f0-9-]{36}', request_id)
    assert re.fullmatch(r'https://www\.monolithannex\.com/host/[a-f0-9]{64}', request.get('origin', ''))
    seen.add(request_id)
    response = dict(id=request_id, passed=False)
    try:
        result = subprocess.run([node, str(probe), request['origin']],
                                capture_output=True, text=True, timeout=65, check=False)
        if result.returncode != 0:
            raise RuntimeError(result.stderr[-16384:])
        assert len(result.stdout) <= 1024 * 1024
        summary = json.loads(result.stdout)
        assert isinstance(summary, dict)
        response.update(passed=True, summary=summary)
    except Exception as error:
        response['error'] = str(error)
    # Atomic replacement prevents the UI observer from reading partial JSON.
    temporary = response_path.with_suffix('.tmp')
    temporary.write_text(json.dumps(response))
    os.replace(temporary, response_path)
