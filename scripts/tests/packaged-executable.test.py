#!/usr/bin/env python3
"""Regression checks for safe member selection, independent of native tools."""
import hashlib
import importlib.util
import io
from pathlib import Path
import tarfile
import unittest

spec = importlib.util.spec_from_file_location("packaged", Path(__file__).parents[1] / "packaged-executable.py")
packaged = importlib.util.module_from_spec(spec)
spec.loader.exec_module(packaged)


def archive(entries):
    output = io.BytesIO()
    with tarfile.open(fileobj=output, mode="w") as target:
        for name, data, kind in entries:
            entry = tarfile.TarInfo(name)
            entry.type = kind
            entry.size = len(data) if kind == tarfile.REGTYPE else 0
            if kind != tarfile.REGTYPE:
                entry.linkname = "../outside"
            target.addfile(entry, io.BytesIO(data))
    output.seek(0)
    return output


class PackagedExecutableTests(unittest.TestCase):
    def test_hashes_only_exact_debian_binary_without_extracting_other_paths(self):
        data = b"\x7fELF actual DEB-stamped binary"
        stream = archive([("../../outside", b"untrusted", tarfile.REGTYPE),
                          ("./usr/bin/annex-desktop", data, tarfile.REGTYPE),
                          ("usr/bin/unrelated", b"other", tarfile.REGTYPE)])
        self.assertEqual(packaged.hash_deb_tar(stream),
                         dict(size=len(data), sha256=hashlib.sha256(data).hexdigest()))

    def test_refuses_duplicate_binary(self):
        with self.assertRaisesRegex(ValueError, "Duplicate"):
            packaged.hash_deb_tar(archive([("usr/bin/annex-desktop", b"\x7fELF", tarfile.REGTYPE)] * 2))

    def test_refuses_symlink_and_hardlink_in_place_of_executable(self):
        for kind in (tarfile.SYMTYPE, tarfile.LNKTYPE):
            with self.subTest(kind=kind), self.assertRaisesRegex(ValueError, "linked"):
                packaged.hash_deb_tar(archive([("usr/bin/annex-desktop", b"", kind)]))

    def test_refuses_missing_binary_and_path_lookalikes(self):
        for name in ("usr/bin/annex", "../usr/bin/annex-desktop", "usr/bin/../bin/annex-desktop"):
            with self.subTest(name=name), self.assertRaisesRegex(ValueError, "no Annex"):
                packaged.hash_deb_tar(archive([(name, b"\x7fELF", tarfile.REGTYPE)]))

    def test_refuses_wrong_native_format(self):
        for kind in packaged.MAGIC:
            with self.subTest(kind=kind), self.assertRaisesRegex(ValueError, "native format"):
                packaged.hash_stream(io.BytesIO(b"NOT AN EXECUTABLE"), kind)

    def test_per_installer_stamp_produces_distinct_identity(self):
        data = [b"MZ__TAURI_BUNDLE_TYPE_VAR_" + stamp for stamp in (b"NSS", b"MSI")]
        digests = [packaged.hash_stream(io.BytesIO(blob), kind)["sha256"]
                   for blob, kind in zip(data, ("nsis", "msi"))]
        self.assertNotEqual(*digests)
        self.assertEqual(digests, [hashlib.sha256(blob).hexdigest() for blob in data])


if __name__ == "__main__":
    unittest.main()
