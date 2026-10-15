#!/usr/bin/env python3
"""Inspect actual Linux bundles and cold installed protocol discovery; no app launch."""
import argparse
import configparser
import hashlib
import json
import os
import pathlib
import re
import subprocess
import tarfile
import tempfile


def run(*args, **kwargs):
    return subprocess.check_output(args, **kwargs)


def digest(path):
    with pathlib.Path(path).open('rb') as stream:
        checksum = hashlib.sha256()
        for block in iter(lambda: stream.read(1024 * 1024), b''):
            checksum.update(block)
        return checksum.hexdigest()


def validate_desktop(text):
    desktop = configparser.ConfigParser(interpolation=None, strict=True)
    desktop.read_string(text)
    entry = desktop['Desktop Entry']
    # This app accepts one URL. A MIME declaration alone does not pass it to
    # the process, and quoting the field code is undefined in the desktop spec.
    if entry.get('Exec') != 'annex-desktop %u':
        raise ValueError('Annex desktop Exec must pass one URL: annex-desktop %u')
    if entry.get('Type') != 'Application' or entry.get('Terminal') != 'false':
        raise ValueError('Unexpected desktop application type')
    if 'x-scheme-handler/annex' not in entry.get('MimeType', '').split(';'):
        raise ValueError('Annex protocol MIME declaration is missing')
    return dict(exec=entry['Exec'], mime_type=entry['MimeType'])


def validate_dependencies(text):
    dependencies = [part.strip() for part in text.split(',')]
    # A fallback alternative can leave the desktop cache provider uninstalled.
    if not any(re.fullmatch(r'desktop-file-utils(?:\s*\([^()]+\))?', part)
               for part in dependencies):
        raise ValueError('DEB must require desktop-file-utils for MIME cache triggers')


def inspect_deb(path):
    if run('dpkg-deb', '-f', str(path), 'Package', text=True).strip() != 'annex':
        raise ValueError('Unexpected Debian package identity')
    dependencies = run('dpkg-deb', '-f', str(path), 'Depends', text=True).strip()
    validate_dependencies(dependencies)
    child = subprocess.Popen(['dpkg-deb', '--fsys-tarfile', str(path)], stdout=subprocess.PIPE)
    desktop = None
    with tarfile.open(fileobj=child.stdout, mode='r|') as archive:
        for entry in archive:
            name = pathlib.PurePosixPath(entry.name)
            if name == pathlib.PurePosixPath('usr/share/applications/Annex.desktop'):
                if desktop is not None or not entry.isfile():
                    raise ValueError('Duplicate or non-regular desktop entry')
                desktop = archive.extractfile(entry).read().decode('utf-8')
    if child.wait() != 0 or desktop is None:
        raise ValueError('Debian desktop entry was not readable')
    result = validate_desktop(desktop)
    return dict(path=str(path), sha256=digest(path), depends=dependencies,
                desktop_sha256=hashlib.sha256(desktop.encode()).hexdigest(), **result)


def inspect_appimage(path):
    # This runs only the just-built AppImage runtime's offset query, not Annex.
    offset = run(str(path.resolve()), '--appimage-offset', text=True).strip()
    if not offset.isdecimal():
        raise ValueError('AppImage returned an invalid SquashFS offset')
    desktop = run('unsquashfs', '-cat', '-o', offset, str(path),
                  'usr/share/applications/Annex.desktop', text=True)
    utilities = inspect_appimage_utilities(path, offset, desktop)
    return dict(path=str(path), sha256=digest(path), utilities=utilities,
                desktop_sha256=hashlib.sha256(desktop.encode()).hexdigest(),
                **validate_desktop(desktop))


def inspect_appimage_utilities(path, offset, desktop):
    with tempfile.TemporaryDirectory(prefix='annex-appimage-utilities-') as temporary:
        root = pathlib.Path(temporary)
        extracted = root / 'image'
        subprocess.run(['unsquashfs', '-no-progress', '-no-xattrs', '-d', str(extracted),
                        '-o', offset, str(path), 'usr/bin/update-desktop-database',
                        'usr/bin/xdg-mime', 'usr/lib/*.so*',
                        'usr/share/doc/desktop-file-utils/copyright'], check=True,
                       stdout=subprocess.DEVNULL)
        utilities = [extracted/'usr/bin'/name for name in ['update-desktop-database', 'xdg-mime']]
        for utility in utilities:
            if not utility.is_file() or not os.access(utility, os.X_OK):
                raise ValueError('AppImage registration utility is absent or not executable: ' + utility.name)
        notice = extracted/'usr/share/doc/desktop-file-utils/copyright'
        if not notice.is_file() or not notice.read_text().strip():
            raise ValueError('AppImage desktop utility redistribution notice is missing')
        env = {key: value for key, value in os.environ.items() if not key.startswith('LD_')}
        env.update(PATH=str(extracted/'usr/bin') + ':/usr/bin:/bin',
                   LD_LIBRARY_PATH=str(extracted/'usr/lib'), HOME=str(root/'home'),
                   XDG_DATA_HOME=str(root/'data'), XDG_CONFIG_HOME=str(root/'config'),
                   XDG_DATA_DIRS=str(root/'empty-system-data'))
        closure = run('ldd', str(utilities[0]), env=env, text=True)
        if 'not found' in closure:
            raise ValueError('Bundled desktop cache utility has unresolved native dependencies')
        # libc and the ELF interpreter come from the supported host ABI. Every
        # other utility dependency must resolve inside the extracted AppImage.
        for line in closure.splitlines():
            match = re.search(r'^\s*(\S+)\s+=>\s+(/\S+)', line)
            if match and match[1] not in {'libc.so.6', 'libm.so.6', 'libpthread.so.0', 'libdl.so.2', 'librt.so.1'}:
                if not pathlib.Path(match[2]).resolve().is_relative_to(extracted):
                    raise ValueError('AppImage utility depends on an unbundled library: ' + match[1])
        applications = root/'data/applications'
        applications.mkdir(parents=True)
        (applications/'Annex.desktop').write_text(desktop)
        # Exercise both actual packaged utilities against a disposable profile.
        # This writes only its MIME cache, never changes a user's default or
        # starts the app. The fresh profile discovers the generated cache.
        subprocess.run([str(utilities[0]), str(applications)], env=env, check=True)
        found = run(str(utilities[1]), 'query', 'default', 'x-scheme-handler/annex',
                    env=env, text=True).strip()
        if found != 'Annex.desktop':
            raise ValueError('Bundled desktop utilities cannot discover the Annex URL handler')
        return dict(files=[dict(name=p.name, sha256=digest(p)) for p in utilities],
                    copyright_sha256=digest(notice), dependency_closure=closure,
                    disposable_profile_handler=found, application_launched=False)


def inspect_installed():
    path = pathlib.Path('/usr/share/applications/Annex.desktop')
    result = validate_desktop(path.read_text())
    cache = pathlib.Path('/usr/share/applications/mimeinfo.cache').read_text()
    parser = configparser.ConfigParser(interpolation=None)
    parser.read_string(cache)
    if 'Annex.desktop' not in parser['MIME Cache'].get('x-scheme-handler/annex', '').split(';'):
        raise ValueError('The package manager did not register Annex in the desktop MIME cache')
    # A fresh profile must discover the installed handler before Annex has ever
    # run. Never call xdg-mime default or update-desktop-database in this check.
    with tempfile.TemporaryDirectory(prefix='annex-cold-handler-') as temporary:
        env = {**os.environ, 'HOME': temporary, 'XDG_CONFIG_HOME': temporary + '/config',
               'XDG_DATA_HOME': temporary + '/data', 'XDG_DATA_DIRS': '/usr/local/share:/usr/share'}
        handler = run('xdg-mime', 'query', 'default', 'x-scheme-handler/annex',
                      env=env, text=True).strip()
    if handler != 'Annex.desktop':
        raise ValueError(f'Cold installed Annex handler was not discovered: {handler!r}')
    return dict(desktop_sha256=digest(path), discovered_handler=handler,
                fresh_profile=True, application_launched=False, **result)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--deb', type=pathlib.Path)
    parser.add_argument('--appimage', type=pathlib.Path)
    parser.add_argument('--installed', action='store_true')
    parser.add_argument('--receipt', type=pathlib.Path)
    args = parser.parse_args()
    if not (args.deb or args.appimage or args.installed):
        parser.error('Select at least one actual package or installed check')
    receipt = {}
    if args.deb:
        receipt['deb'] = inspect_deb(args.deb)
    if args.appimage:
        receipt['appimage'] = inspect_appimage(args.appimage)
    if args.installed:
        receipt['installed'] = inspect_installed()
    if args.receipt:
        args.receipt.write_text(json.dumps(receipt, indent=2) + '\n')
    print(json.dumps(receipt, indent=2))


if __name__ == '__main__':
    main()
