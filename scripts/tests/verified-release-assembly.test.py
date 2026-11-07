"""Security boundaries for resuming an authenticated unpublished assembly."""
import contextlib
import hashlib
import io
import json
import os
from pathlib import Path
import runpy
import tempfile
import unittest
from unittest.mock import patch
import zipfile

SCRIPT = Path(__file__).resolve().parents[1] / "retrieve-verified-release-assembly.py"
SOURCE = "a" * 40
REVISION = "b" * 40


class AssemblyRecoveryTests(unittest.TestCase):
    def exercise(self, *, published=False, corrupt_archive=False, unsafe=False,
                 substituted_payload=False, different_source=False):
        catalog = dict(sourceCommit=("c" * 40 if different_source else SOURCE),
                       tag="dry-run-" + SOURCE, unsignedDryRun=False, version="0.1.3")
        files = {"downloads.json": json.dumps(catalog).encode(),
                 "unit-test-payload.bin": b"unit test data, not a native installer"}
        sums = "".join(hashlib.sha256(value).hexdigest() + "  " + name + "\n"
                       for name, value in files.items()).encode()
        if substituted_payload:
            files["unit-test-payload.bin"] = b"changed after the manifest was verified"
        files["SHA256SUMS"] = sums
        blob = io.BytesIO()
        with zipfile.ZipFile(blob, "w") as archive:
            for name, value in files.items():
                archive.writestr("subject/release-assets/" + name, value)
            archive.writestr("assembly-provenance.json", json.dumps(dict(
                source_commit=SOURCE, assembly_run="123", assembly_harness_commit=REVISION,
                no_native_payload_rebuilt=True)))
            archive.writestr("RELEASE_NOTES.md", "Unit test notes, not release acceptance")
            if unsafe:
                archive.writestr("../outside-assembly.txt", b"must not be extracted")
        content = blob.getvalue()
        artifact = dict(id=7, name="verified-release-assembly", expired=False,
                        workflow_run=dict(id=123, head_sha=REVISION),
                        size_in_bytes=len(content),
                        digest="sha256:" + ("0" * 64 if corrupt_archive else hashlib.sha256(content).hexdigest()))
        release = dict(id=99, draft=not published, published_at="now" if published else None,
                       target_commitish=SOURCE, tag_name="dry-run-" + SOURCE)
        calls = []

        def output(arguments):
            self.assertEqual(arguments[:2], ["gh", "api"])
            route = arguments[2]
            calls.append(route)
            if route.endswith("/releases/99"):
                value = release
            elif route.endswith("/actions/runs/123"):
                value = dict(path=".github/workflows/release-desktop.yml", event="workflow_dispatch",
                             conclusion="success", head_sha=REVISION)
            elif route.endswith("/jobs?per_page=100"):
                value = dict(total_count=1, jobs=[dict(name="reassemble / assemble",
                                                     conclusion="success", head_sha=REVISION)])
            elif route.endswith("/artifacts?per_page=100"):
                value = dict(total_count=1, artifacts=[artifact])
            elif route.endswith("/actions/artifacts/7"):
                value = artifact
            else:
                self.fail("Unexpected API operation " + route)
            return json.dumps(value).encode()

        def download(arguments, *, stdout, check):
            self.assertTrue(check)
            self.assertEqual(arguments, ["gh", "api", "repos/seismicgear/annex/actions/artifacts/7/zip"])
            stdout.write(content)

        with tempfile.TemporaryDirectory(prefix="annex-assembly-verification-") as temporary:
            root = Path(temporary).resolve()
            self.assertEqual(root.parent, Path(tempfile.gettempdir()).resolve())
            prior = Path.cwd()
            try:
                os.chdir(root)
                with patch.dict(os.environ, ASSEMBLY_RUN="123", SUBJECT_COMMIT=SOURCE,
                                EXISTING_RELEASE_ID="99", GITHUB_REPOSITORY="seismicgear/annex",
                                RUNNER_TEMP=str(root)), patch("subprocess.check_output", output), \
                        patch("subprocess.run", download), contextlib.redirect_stdout(io.StringIO()):
                    if any((published, corrupt_archive, unsafe, substituted_payload, different_source)):
                        with self.assertRaises(AssertionError):
                            runpy.run_path(str(SCRIPT), run_name="__main__")
                        self.assertFalse((root / "assembly-acquisition.json").exists())
                        self.assertFalse((root.parent / "outside-assembly.txt").exists())
                        if published:
                            self.assertEqual(len(calls), 1)
                    else:
                        runpy.run_path(str(SCRIPT), run_name="__main__")
                        receipt = json.loads((root / "assembly-acquisition.json").read_text())
                        self.assertTrue(receipt["artifactDigestVerified"])
                        self.assertTrue(receipt["allFilesChecksumVerified"])
                        self.assertFalse(receipt["publication"])
                        self.assertEqual((root / "release-assets/unit-test-payload.bin").read_bytes(),
                                         files["unit-test-payload.bin"])
            finally:
                os.chdir(prior)

    def test_exact_authenticated_assembly_is_recovered_without_execution(self):
        self.exercise()

    def test_published_release_is_rejected_before_download(self):
        self.exercise(published=True)

    def test_corrupt_archive_is_rejected_before_extraction(self):
        self.exercise(corrupt_archive=True)

    def test_archive_cannot_write_outside_assembly(self):
        self.exercise(unsafe=True)

    def test_substituted_payload_is_rejected(self):
        self.exercise(substituted_payload=True)

    def test_mismatched_native_source_is_rejected(self):
        self.exercise(different_source=True)


if __name__ == "__main__":
    unittest.main()
