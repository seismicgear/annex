"""Serve exact candidate bytes over OS-trusted test TLS on a disposable runner.

The mode file controls transport faults only. The application uses its real
updater, production verification public key, native installer and saved profile.
No private release key, application commands or application state are involved.
"""
import argparse
import datetime
import http.server
import json
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
artifact = next(row for row in inventory["artifacts"] if row["kind"] == "nsis")
payload = args.candidate / artifact["name"]
assert payload.stat().st_size == artifact["size"]
signature = (args.candidate / (artifact["name"] + ".sig")).read_text().strip()
lock = threading.Lock()


def event(**values):
    with lock, args.events.open("a", encoding="utf-8") as output:
        output.write(json.dumps(dict(at=datetime.datetime.now(datetime.timezone.utc).isoformat(),
                                    **values)) + "\n")


class Handler(http.server.BaseHTTPRequestHandler):
    def log_message(self, *_):
        pass

    def do_GET(self):
        mode = args.mode.read_text(encoding="utf-8-sig").strip()
        assert mode in ("stall", "corrupt", "normal")
        event(path=self.path, mode=mode, stage="request")
        if self.path == "/update.json":
            body = json.dumps(dict(
                version=inventory["version"],
                notes="Native qualification fixture: exact candidate with transport fault tests.",
                pub_date="2026-10-20T00:00:00Z",
                platforms={"windows-x86_64": dict(
                    url=f"https://localhost:{args.port}/payload.exe", signature=signature)},
            )).encode()
        elif self.path == "/health":
            body = b"ready"
        elif self.path == "/payload.exe":
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
