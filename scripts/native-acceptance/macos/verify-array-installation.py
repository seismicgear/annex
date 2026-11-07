"""Require the installed app to match its source-bound Array inventory."""
import hashlib
import json
import os
from pathlib import Path
import platform
import plistlib
import stat

evidence = Path("native-evidence")
inventory = json.loads((evidence / "array-installation.json").read_text())
assert inventory["sourceCommit"] == os.environ["SUBJECT_COMMIT"]
assert inventory["platform"] == "darwin"
assert inventory["arch"] == {"arm64": "arm64", "x86_64": "x64"}[platform.machine()]
bundle = Path("/Applications/Annex.app")
with (bundle / "Contents/Info.plist").open("rb") as stream:
    info = plistlib.load(stream)
assert info["CFBundleIdentifier"] == inventory["nativeId"] == "com.annex.desktop"
assert info["CFBundleShortVersionString"] == inventory["version"]
binary = bundle / "Contents/MacOS/annex-desktop"
assert stat.S_ISREG(binary.lstat().st_mode)
with binary.open("rb") as stream:
    digest = hashlib.file_digest(stream, "sha256").hexdigest()
assert len(inventory["artifacts"]) == 1
assert digest == inventory["artifacts"][0]["executableSha256"]
(evidence / "array-installed-identity.json").write_text(json.dumps(dict(
    passed=True, sourceCommit=inventory["sourceCommit"], version=inventory["version"],
    arch=inventory["arch"], installedExecutableSha256=digest), indent=2) + "\n")
print("Actual installed macOS executable matches the Array release inventory.")
