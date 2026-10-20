#!/usr/bin/env python3
"""Fail closed if a sealed app loses its shipped sandbox or gains bypasses."""
import importlib.util
from pathlib import Path
import plistlib
import unittest

spec = importlib.util.spec_from_file_location(
    "bundle", Path(__file__).parents[1] / "verify-macos-bundle.py")
bundle = importlib.util.module_from_spec(spec)
spec.loader.exec_module(bundle)


class SignedEntitlementsTests(unittest.TestCase):
    def test_accepts_the_shipped_entitlements_and_publisher_identity(self):
        shipped = plistlib.loads((Path(__file__).parents[2] /
                                  "crates/annex-desktop/Entitlements.plist").read_bytes())
        shipped["com.apple.developer.team-identifier"] = "TESTTEAM12"
        self.assertEqual(bundle.verify_entitlements(plistlib.dumps(shipped)), shipped)

    def test_rejects_missing_or_changed_required_claims(self):
        for key, expected in bundle.REQUIRED_ENTITLEMENTS.items():
            for value in (None, not expected, int(expected), str(expected)):
                with self.subTest(key=key, value=value):
                    changed = dict(bundle.REQUIRED_ENTITLEMENTS)
                    if value is None:
                        del changed[key]
                    else:
                        changed[key] = value
                    with self.assertRaisesRegex(RuntimeError, "entitlement changed"):
                        bundle.verify_entitlements(plistlib.dumps(changed))

    def test_rejects_debugger_and_runtime_bypasses(self):
        for key in bundle.FORBIDDEN_ENTITLEMENTS:
            with self.subTest(key=key):
                changed = dict(bundle.REQUIRED_ENTITLEMENTS, **{key: True})
                with self.assertRaisesRegex(RuntimeError, "Unsafe"):
                    bundle.verify_entitlements(plistlib.dumps(changed))

    def test_rejects_malformed_or_non_dictionary_output(self):
        for data in (b"", b"codesign failed", plistlib.dumps(["sandbox"])):
            with self.subTest(data=data), self.assertRaises(RuntimeError):
                bundle.verify_entitlements(data)


if __name__ == "__main__":
    unittest.main()
