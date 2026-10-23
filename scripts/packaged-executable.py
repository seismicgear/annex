#!/usr/bin/env python3
"""Hash the executable actually shipped in an Annex package, without running it.

Tauri patches __TAURI_BUNDLE_TYPE_VAR_ for each installer format. The compiler
output is therefore NOT the identity of the installed NSIS, MSI or DEB binary.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import stat
import subprocess
import sys
import tarfile
import tempfile

MAX_EXECUTABLE = 1024 ** 3
MAGIC = {
    "nsis": (b"MZ",), "msi": (b"MZ",), "deb": (b"\x7fELF",),
    "dmg": (b"\xcf\xfa\xed\xfe", b"\xfe\xed\xfa\xcf",
            b"\xca\xfe\xba\xbe", b"\xca\xfe\xba\xbf"),
}


def hash_stream(stream, kind):
    first = stream.read(4096)
    if not any(first.startswith(magic) for magic in MAGIC[kind]):
        raise ValueError("Packaged executable has the wrong native format")
    digest, size = hashlib.sha256(first), len(first)
    for block in iter(lambda: stream.read(1024 * 1024), b""):
        size += len(block)
        if size > MAX_EXECUTABLE:
            raise ValueError("Packaged executable exceeds the size limit")
        digest.update(block)
    return dict(sha256=digest.hexdigest(), size=size)


def hash_regular(path, kind):
    metadata = path.lstat()
    if not stat.S_ISREG(metadata.st_mode) or not 0 < metadata.st_size <= MAX_EXECUTABLE:
        raise ValueError("Expected a regular packaged executable")
    with path.open("rb") as stream:
        result = hash_stream(stream, kind)
    if result["size"] != metadata.st_size:
        raise ValueError("Packaged executable changed while hashing")
    return result


def hash_deb_tar(stream):
    result = None
    with tarfile.open(fileobj=stream, mode="r|") as archive:
        for entry in archive:
            if PurePosixPath(entry.name) != PurePosixPath("usr/bin/annex-desktop"):
                continue
            if result is not None or not entry.isfile() or not 0 < entry.size <= MAX_EXECUTABLE:
                raise ValueError("Duplicate, linked or oversized Debian executable")
            with archive.extractfile(entry) as executable:
                result = hash_stream(executable, "deb")
            if result["size"] != entry.size:
                raise ValueError("Truncated Debian executable")
    if result is None:
        raise ValueError("Debian package has no Annex executable")
    return result


def run(*args):
    return subprocess.run(args, check=True, stdout=subprocess.DEVNULL,
                          stderr=subprocess.PIPE, timeout=180,
                          creationflags=subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0)


def inspect(package, kind):
    package = package.resolve(strict=True)
    if kind == "deb":
        # Stream only the exact regular member. Never extract archive paths,
        # links, install hooks or other package contents onto the filesystem.
        child = subprocess.Popen(["dpkg-deb", "--fsys-tarfile", str(package)],
                                 stdout=subprocess.PIPE)
        try:
            result = hash_deb_tar(child.stdout)
            if child.wait(timeout=30) != 0:
                raise ValueError("Debian package reader failed")
            return result
        finally:
            child.stdout.close()
            if child.poll() is None:
                child.kill()
            child.wait()
    with tempfile.TemporaryDirectory(prefix="annex-packaged-executable-") as temporary:
        # macOS exposes /var as an alias of /private/var. Compare canonical
        # paths on both sides of the containment check, including temp roots.
        root = Path(temporary).resolve(strict=True)
        if kind in ("nsis", "msi"):
            reader = os.environ.get("ANNEX_7ZIP_BINARY", "7z")
            member = "annex-desktop.exe" if kind == "nsis" else "app.cab"
            run(reader, "e", str(package), member, "-o" + str(root), "-y", "-bso0", "-bsp0")
            executable = root / member
            if kind == "msi":
                # Tauri's WiX template uses File Id="Path" for the main binary.
                # Refuse template drift instead of guessing a different member.
                run(reader, "e", str(executable), "Path", "-o" + str(root), "-y", "-bso0", "-bsp0")
                executable = root / "Path"
            return hash_regular(executable, kind)
        if kind == "dmg":
            mount = root / "mounted"
            mount.mkdir()
            run("hdiutil", "attach", str(package), "-readonly", "-nobrowse",
                "-mountpoint", str(mount))
            try:
                executable = mount / "Annex.app/Contents/MacOS/annex-desktop"
                if not executable.resolve(strict=True).is_relative_to(mount):
                    raise ValueError("DMG executable escapes the mounted package")
                return hash_regular(executable, kind)
            finally:
                run("hdiutil", "detach", str(mount))
        raise ValueError("Unsupported native package format")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--package", type=Path, required=True)
    parser.add_argument("--kind", choices=MAGIC, required=True)
    args = parser.parse_args()
    try:
        print(json.dumps(inspect(args.package, args.kind)))
    except (OSError, ValueError, subprocess.SubprocessError, tarfile.TarError) as error:
        print(str(error), file=sys.stderr)
        sys.exit(1)
