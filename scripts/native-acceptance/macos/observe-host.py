"""Observe the installed test app's actual open database; never seed it."""
import datetime
import json
import pathlib
import sqlite3
import subprocess
import time

INSTALLED_EXECUTABLE = '/Applications/Annex.app/Contents/MacOS/annex-desktop'


def app_processes(process_listing):
    """Match executable identity, not a substring in some other command."""
    result = []
    for line in process_listing.splitlines():
        fields = line.strip().split(None, 1)
        if len(fields) == 2 and fields[0].isdigit() and fields[1] == INSTALLED_EXECUTABLE:
            result.append(int(fields[0]))
    return result


def open_database_paths(file_listing):
    return sorted({line[1:] for line in file_listing.splitlines()
                   if line.startswith('n/') and line.endswith('/annex.db')})


def observe_process():
    listing = subprocess.run(['ps', '-axo', 'pid=,comm='], capture_output=True,
                             text=True, check=True, timeout=5)
    pids = app_processes(listing.stdout)
    record = {'app_pids': pids, 'database_present': False}
    if len(pids) != 1:
        return record, None
    files = subprocess.run(['/usr/sbin/lsof', '-a', '-p', str(pids[0]), '-Fn'],
                           capture_output=True, text=True, timeout=5)
    paths = open_database_paths(files.stdout)
    record['database_candidates'] = paths
    record['file_observer_exit'] = files.returncode
    if len(paths) != 1:
        return record, None
    database = pathlib.Path(paths[0])
    record['database_path'] = str(database)
    record['database_present'] = database.is_file()
    return record, database if database.is_file() else None


def main():
    output = pathlib.Path('native-evidence/host-observations.jsonl')
    deadline = time.monotonic() + 900
    while time.monotonic() < deadline:
        record = {'at': datetime.datetime.now(datetime.timezone.utc).isoformat()}
        try:
            process, database = observe_process()
            record.update(process)
            if database is not None:
                with sqlite3.connect(database.as_uri() + '?mode=ro', uri=True, timeout=1) as conn:
                    conn.row_factory = sqlite3.Row
                    record['servers'] = [dict(row) for row in conn.execute('SELECT id,slug,label,public_url FROM servers')]
                    record['identities'] = [dict(row) for row in conn.execute('SELECT id,server_id,participant_type,can_moderate,can_invite,active FROM platform_identities ORDER BY id')]
        except (OSError, sqlite3.Error, subprocess.SubprocessError) as error:
            record['observer_error'] = str(error)
        with output.open('a') as file:
            file.write(json.dumps(record) + '\n')
        time.sleep(1)


if __name__ == '__main__':
    main()
