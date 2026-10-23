"""Use Ubuntu's HTTPS archive on disposable GitHub-hosted build runners.

The hosted mirror selector can hang while fetching indexes from the Azure
mirror, even after a successful fallback for InRelease. Package signatures,
distribution suites, trusted keys and non-Ubuntu repositories remain intact.
See actions/runner-images#14594 and #12949.
"""
import os
from pathlib import Path
import sys

if (sys.platform != 'linux' or os.environ.get('GITHUB_ACTIONS') != 'true'
        or os.environ.get('RUNNER_ENVIRONMENT') != 'github-hosted'):
    raise SystemExit('This setup is only for disposable GitHub-hosted Linux runners')

for name, mirror in [('apt-mirrors.txt', 'https://archive.ubuntu.com/ubuntu/'),
                     ('apt-security-mirrors.txt', 'https://security.ubuntu.com/ubuntu/')]:
    file = Path('/etc/apt') / name
    if file.exists():
        if file.is_symlink() or not file.is_file():
            raise SystemExit('Unexpected hosted mirror configuration')
        file.write_text(mirror + '\n')
        print(f'Using the official HTTPS Ubuntu archive for {name}')

config = Path('/etc/apt/apt.conf.d/99-annex-network-bounds')
if config.is_symlink():
    raise SystemExit('Unexpected APT configuration link')
config.write_text('Acquire::http::Timeout "30";\nAcquire::https::Timeout "30";\n'
                  'Acquire::Retries "3";\nDPkg::Lock::Timeout "120";\n')
