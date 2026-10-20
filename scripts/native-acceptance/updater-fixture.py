"""Serve exact candidate bytes over OS-trusted test TLS on a disposable runner.

The mode file controls transport faults only. The application uses its real
updater, production verification public key, native installer and saved profile.
No private release key, application commands or application state are involved.
"""
import argparse
import datetime
import hashlib
import hmac
import http.server
import json
import os
from pathlib import Path
import ssl
import threading
import time

parser = argparse.ArgumentParser()
parser.add_argument("--candidate", type=Path, required=True)
parser.add_argument("--tls", type=Path, required=True)
parser.add_argument("--mode", type=Path, required=True)
parser.add_argument("--events", type=Path, required=True)
parser.add_argument("--port", type=int, default=34567)
args = parser.parse_args()
inventory = json.loads((args.candidate / "array-installation.json").read_text())
if inventory["platform"] == "darwin":
    artifact = json.loads((args.candidate / "updater-payload.json").read_text())
    target = "darwin-" + ("aarch64" if inventory["arch"] == "arm64" else "x86_64")
    kind = "app"
    default_artifact = artifact
else:
    default_kind = "nsis" if inventory["platform"] == "win32" else "appimage"
    kind = os.environ.get("ANNEX_UPDATER_INSTALLER_KIND", default_kind)
    assert kind in (("nsis", "msi") if inventory["platform"] == "win32" else ("appimage", "deb"))
    artifact = next(row for row in inventory["artifacts"] if row["kind"] == kind)
    default_artifact = next(row for row in inventory["artifacts"] if row["kind"] == default_kind)
    target = "windows-x86_64" if inventory["platform"] == "win32" else "linux-x86_64"
payload = args.candidate / artifact["name"]
assert payload.stat().st_size == artifact["size"]
with payload.open("rb") as stream:
    assert hashlib.file_digest(stream, "sha256").hexdigest() == artifact["sha256"]
signature = (args.candidate / (artifact["name"] + ".sig")).read_text().strip()
default_signature = (args.candidate / (default_artifact["name"] + ".sig")).read_text().strip()
lock = threading.Lock()


def event(**values):
    with lock, args.events.open("a", encoding="utf-8") as output:
        output.write(json.dumps(dict(at=datetime.datetime.now(datetime.timezone.utc).isoformat(),
                                    **values)) + "\n")


class Handler(http.server.BaseHTTPRequestHandler):
    def log_message(self, *_):
        pass

    def do_POST(self):
        # XCTest cannot write into a different app container. This loopback
        # fixture control changes transport behavior only, never Annex state.
        token = os.environ.get("ANNEX_UPDATER_CONTROL_TOKEN", "")
        supplied = self.headers.get("Authorization", "")
        if (self.path != "/control" or len(token) < 32
                or not hmac.compare_digest(supplied, "Bearer " + token)):
            self.send_error(403)
            return
        length = int(self.headers.get("Content-Length", "0"))
        if not 0 < length < 1024:
            self.send_error(400)
            return
        try:
            mode = json.loads(self.rfile.read(length))["mode"]
            assert mode in ("stall", "corrupt", "normal")
        except (ValueError, KeyError, AssertionError):
            self.send_error(400)
            return
        temporary = args.mode.with_suffix(".tmp")
        temporary.write_text(mode)
        os.replace(temporary, args.mode)
        event(stage="transport-mode-selected", mode=mode)
        self.send_response(204)
        self.end_headers()

    def do_GET(self):
        mode = args.mode.read_text(encoding="utf-8-sig").strip()
        assert mode in ("stall", "corrupt", "normal")
        event(path=self.path, mode=mode, stage="request")
        if self.path == "/update.json":
            body = json.dumps(dict(
                version=inventory["version"],
                notes="Native qualification fixture: exact candidate with transport fault tests.",
                pub_date="2026-10-20T00:00:00Z",
                # Keep the real feed's legacy default and installer-specific
                # entry. MSI/DEB must choose their own entry; this fixture
                # serves only the selected format, so a fallback cannot pass.
                platforms={
                    target: dict(url=f"https://localhost:{args.port}/{default_artifact['name']}",
                                 signature=default_signature),
                    f"{target}-{kind}": dict(url=f"https://localhost:{args.port}/{payload.name}",
                                            signature=signature)},
            )).encode()
        elif self.path == "/health":
            body = b"ready"
        elif self.path == "/" + payload.name:
            self.send_response(200)
            self.send_header("Content-Type", "application/octet-stream")
            self.send_header("Content-Length", str(payload.stat().st_size))
            self.end_headers()
            try:
                with payload.open("rb") as source:
                    first = True
                    while chunk := source.read(64 * 1024):
                        if first and mode == "corrupt":
                            chunk = bytes([chunk[0] ^ 1]) + chunk[1:]
                        self.wfile.write(chunk)
                        self.wfile.flush()
                        if first and mode == "stall":
                            event(path=self.path, mode=mode, stage="stalled-after-first-chunk")
                            # Exceeds the real app's 60-second idle watchdog.
                            time.sleep(75)
                            return
                        first = False
                event(path=self.path, mode=mode, stage="complete")
            except (BrokenPipeError, ConnectionResetError, ConnectionAbortedError, ssl.SSLError):
                event(path=self.path, mode=mode, stage="client-disconnected")
            return
        else:
            self.send_error(404)
            return
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)


server = http.server.ThreadingHTTPServer(("127.0.0.1", args.port), Handler)
context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
context.minimum_version = ssl.TLSVersion.TLSv1_2
context.load_cert_chain(args.tls / "server.pem", args.tls / "server.key")
server.socket = context.wrap_socket(server.socket, server_side=True)
event(stage="ready", candidateSha256=artifact["sha256"], sourceCommit=inventory["sourceCommit"],
      endpoint=f"https://localhost:{args.port}/update.json")
server.serve_forever()
