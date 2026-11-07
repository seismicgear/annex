"""Run the independent read-only browser probe outside XCTest's app container."""
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import time

evidence = Path('native-evidence').resolve()
observations = evidence / 'host-observations.jsonl'
response_path = evidence / 'public-summary-response.json'
probe = Path(__file__).resolve().parent.parent / 'public-host-summary.mjs'
node = shutil.which('node')
assert node and evidence.is_dir()
seen = set()
deadline = time.monotonic() + 20 * 60
while time.monotonic() < deadline:
    try:
        snapshot = json.loads(observations.read_text().splitlines()[-1])
        pids = snapshot.get('app_pids', [])
        servers = snapshot.get('servers', [])
        if len(pids) != 1 or len(servers) != 1:
            time.sleep(0.2)
            continue
        origin = servers[0].get('public_url', '')
        if not re.fullmatch(r'https://www\.monolithannex\.com/host/[a-f0-9]{64}', origin):
            time.sleep(0.2)
            continue
    except (FileNotFoundError, IndexError, json.JSONDecodeError):
        time.sleep(0.2)
        continue
    generation = (pids[0], origin)
    if generation in seen:
        time.sleep(0.2)
        continue
    seen.add(generation)
    # A persisted public URL appears before a restarted process has registered
    # its live introduction connection. Bound that startup race and retain
    # every failed observation; never retry cryptographic or identity failures.
    # XCTest only reads this receipt; its container needs no external writes.
    response = dict(app_pid=pids[0], origin=origin, passed=False, attempts=[])
    expires = time.monotonic() + 65
    try:
        for attempt in range(3):
            remaining = expires - time.monotonic()
            if remaining <= 0:
                raise TimeoutError('Native public host did not become reachable within 65 seconds')
            result = subprocess.run([node, str(probe), origin],
                                    capture_output=True, text=True, timeout=remaining, check=False)
            response['attempts'].append(dict(
                attempt=attempt + 1, passed=result.returncode == 0,
                error=None if result.returncode == 0 else result.stderr[-16384:]))
            if result.returncode == 0:
                assert len(result.stdout) <= 1024 * 1024
                summary = json.loads(result.stdout)
                assert isinstance(summary, dict)
                response.update(passed=True, summary=summary)
                break
            if attempt == 2 or 'Public introduction closed: 1011' not in result.stderr:
                raise RuntimeError(result.stderr[-16384:])
            time.sleep(1)
    except Exception as error:
        response['error'] = str(error)
    # Atomic replacement prevents the UI observer from reading partial JSON.
    temporary = response_path.with_suffix('.tmp')
    temporary.write_text(json.dumps(response))
    os.replace(temporary, response_path)
