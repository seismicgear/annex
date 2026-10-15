"""Read only the test account's ordinary embedded-server DB; never seed it."""
import datetime
import json
import pathlib
import os
import pwd
import sqlite3
import time

# XCTest launches LaunchServices apps as the logged-in desktop user, which
# can differ from the shell runner. Read that account's ordinary data only.
console_home = pathlib.Path(pwd.getpwuid(os.stat('/dev/console').st_uid).pw_dir)
database = console_home / 'Library/Application Support/Annex/annex.db'
output = pathlib.Path('native-evidence/host-observations.jsonl')
deadline = time.monotonic() + 900
while time.monotonic() < deadline:
    record = {'at': datetime.datetime.now(datetime.timezone.utc).isoformat(), 'database_present': database.exists()}
    if database.exists():
        try:
            with sqlite3.connect(database.as_uri() + '?mode=ro', uri=True, timeout=1) as conn:
                conn.row_factory = sqlite3.Row
                record['servers'] = [dict(row) for row in conn.execute('SELECT id,slug,label,public_url FROM servers')]
                record['identities'] = [dict(row) for row in conn.execute('SELECT id,server_id,participant_type,can_moderate,can_invite,active FROM platform_identities ORDER BY id')]
        except sqlite3.Error as error:
            record['database_error'] = str(error)
    with output.open('a') as file:
        file.write(json.dumps(record) + '\n')
    time.sleep(1)
