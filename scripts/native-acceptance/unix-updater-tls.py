"""Owned, ephemeral OS TLS trust for disposable native updater qualification."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys

parser = argparse.ArgumentParser()
parser.add_argument("--cleanup", action="store_true")
args = parser.parse_args()
assert os.environ.get("GITHUB_ACTIONS") == "true"
assert sys.platform in ("linux", "darwin")
directory = Path(os.environ["RUNNER_TEMP"]) / "annex-updater-tls"
ownership = directory / "ownership.json"
run_id = os.environ["GITHUB_RUN_ID"]
assert re.fullmatch(r"[0-9]+", run_id)
linux_trust = Path("/usr/local/share/ca-certificates") / f"annex-updater-ci-{run_id}.crt"


def run(*arguments):
    subprocess.run(arguments, check=True, stdout=subprocess.DEVNULL)


if args.cleanup:
    if ownership.exists():
        record = json.loads(ownership.read_text())
        assert record["run"] == run_id
        if sys.platform == "linux":
            if linux_trust.exists():
                assert hashlib.sha256(linux_trust.read_bytes()).hexdigest() == record["sha256"]
                run("sudo", "rm", "--", str(linux_trust))
                run("sudo", "update-ca-certificates")
        else:
            assert re.fullmatch(r"[A-F0-9]{40}", record["sha1"])
            run("sudo", "security", "delete-certificate", "-Z", record["sha1"],
                "/Library/Keychains/System.keychain")
        ownership.unlink()
    sys.exit(0)

directory.mkdir(exist_ok=False)
root, key = directory / "root.pem", directory / "root.key"
(directory / "root.cnf").write_text(
    "[req]\ndistinguished_name=dn\nx509_extensions=ca\n[dn]\n[ca]\n"
    "basicConstraints=critical,CA:TRUE\nkeyUsage=critical,keyCertSign,cRLSign\n")
run("openssl", "req", "-x509", "-newkey", "rsa:2048", "-sha256", "-days", "1", "-nodes",
    "-keyout", str(key), "-out", str(root), "-subj", "/CN=Annex Native Updater Fixture",
    "-config", str(directory / "root.cnf"))
run("openssl", "req", "-newkey", "rsa:2048", "-sha256", "-nodes",
    "-keyout", str(directory / "server.key"), "-out", str(directory / "server.csr"), "-subj", "/CN=localhost")
(directory / "server.ext").write_text(
    "basicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature,keyEncipherment\n"
    "extendedKeyUsage=serverAuth\nsubjectAltName=DNS:localhost,IP:127.0.0.1\n")
run("openssl", "x509", "-req", "-sha256", "-days", "1", "-in", str(directory / "server.csr"),
    "-CA", str(root), "-CAkey", str(key), "-CAcreateserial",
    "-extfile", str(directory / "server.ext"), "-out", str(directory / "server.pem"))
run("openssl", "verify", "-CAfile", str(root), str(directory / "server.pem"))
fingerprint = subprocess.check_output(["openssl", "x509", "-in", str(root), "-noout", "-sha1", "-fingerprint"], text=True)
thumbprint = fingerprint.strip().split("=")[1].replace(":", "").upper()
assert re.fullmatch(r"[A-F0-9]{40}", thumbprint)
record = dict(run=run_id, sha1=thumbprint, sha256=hashlib.sha256(root.read_bytes()).hexdigest())
if sys.platform == "linux":
    assert not linux_trust.exists(), "Refusing to claim existing runner trust"
    ownership.write_text(json.dumps(record))
    run("sudo", "cp", "--", str(root), str(linux_trust))
    run("sudo", "update-ca-certificates")
else:
    ownership.write_text(json.dumps(record))
    run("sudo", "security", "add-trusted-cert", "-d", "-r", "trustRoot", "-k",
        "/Library/Keychains/System.keychain", str(root))
Path("native-evidence/tls-boundary.json").write_text(json.dumps(dict(
    fixtureRootSha1=thumbprint, ephemeralRunnerTrust=True,
    productionOriginTested=False, insecureTlsBypass=False), indent=2) + "\n")
