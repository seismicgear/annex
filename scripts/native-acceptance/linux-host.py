"""Drive the installed Linux app through WebKit's native W3C WebDriver.

No state injection, IPC mocking, alternate frontend or compiled test application.
The browser options and TAURI_WEBVIEW_AUTOMATION environment match upstream
tauri-driver 2.1.0's Linux adapter. This script runs only on disposable CI hosts.
"""
import argparse
import base64
import json
import os
from pathlib import Path
import subprocess
import time
import urllib.error
import urllib.request

parser = argparse.ArgumentParser()
parser.add_argument("--executable", type=Path, required=True)
parser.add_argument("--profile", type=Path, required=True)
parser.add_argument("--kind", choices=("deb", "appimage"), required=True)
parser.add_argument("--phases", default="first,reopen")
parser.add_argument("--output", type=Path, required=True)
args = parser.parse_args()
assert args.executable.is_absolute() and args.executable.is_file()
assert args.profile.is_absolute()
phases = args.phases.split(",")
assert phases in (["first", "reopen"], ["reinstalled"])
args.output.mkdir(parents=True, exist_ok=True)
endpoint = "http://127.0.0.1:4445"
element_key = "element-6066-11e4-a52e-4f735466cecf"
marker = f"Annex {args.kind} native package persistence acceptance"
session = None


def request(method, route, body=None):
    data = None if body is None else json.dumps(body).encode()
    req = urllib.request.Request(endpoint + route, data=data, method=method,
                                 headers={"Content-Type": "application/json"})
    try:
        with urllib.request.urlopen(req, timeout=90) as response:
            result = json.load(response)
    except urllib.error.HTTPError as error:
        raise RuntimeError(error.read().decode()[:2000]) from error
    value = result.get("value")
    if isinstance(value, dict) and value.get("error"):
        raise RuntimeError(json.dumps(value)[:2000])
    return value


def command(method, route, body=None):
    return request(method, f"/session/{session}" + route, body)


def elements(xpath):
    return command("POST", "/elements", {"using": "xpath", "value": xpath})


def visible(xpath):
    for item in elements(xpath):
        identifier = item[element_key]
        rect = command("GET", f"/element/{identifier}/rect")
        visibility = command("GET", f"/element/{identifier}/css/visibility")
        if rect["width"] > 0 and rect["height"] > 0 and visibility not in ("hidden", "collapse"):
            return identifier
    return None


def wait(check, description, seconds=120):
    deadline = time.monotonic() + seconds
    last_error = None
    while time.monotonic() < deadline:
        try:
            value = check()
            if value:
                return value
        except (RuntimeError, urllib.error.URLError) as error:
            last_error = str(error)
        time.sleep(0.5)
    raise RuntimeError(f"Timed out waiting for {description}: {last_error}")


def click(xpath):
    identifier = wait(lambda: visible(xpath), xpath)
    command("POST", f"/element/{identifier}/click", {})


environment = dict(os.environ, TAURI_WEBVIEW_AUTOMATION="true")
with (args.output / f"{args.kind}-webdriver.log").open("a") as log:
    driver = subprocess.Popen(["WebKitWebDriver", "--host=127.0.0.1", "--port=4445"],
                              env=environment, stdout=log, stderr=log)
    try:
        wait(lambda: request("GET", "/status").get("ready"), "native WebKit driver", 30)
        for phase in phases:
            created = request("POST", "/session", {"capabilities": {"alwaysMatch": {
                "webkitgtk:browserOptions": {"binary": str(args.executable),
                                            "args": ["--profile-dir", str(args.profile)]}
            }}})
            session = created["sessionId"]
            try:
                host = "//button[normalize-space(.)='Start Hosting']"
                admin = "//button[normalize-space(.)='Admin']"
                wait(lambda: visible(host) or visible(admin), "Annex host entry")
                if visible(host):
                    click(host)
                wait(lambda: visible(admin), "real embedded server and founder", 180)
                click("//button[normalize-space(.)='Identity']")
                wait(lambda: visible("//*[normalize-space(.)='Server founder']"), "founder badge")
                click("//*[@role='dialog']//button[normalize-space(.)='Close']")
                click("//button[normalize-space(.)='Chat']")
                click("//*[contains(@class,'channel-item') and contains(.,'General')]")
                if phase == "first":
                    editor = wait(lambda: visible("//*[@placeholder='Type a message...']"), "message editor")
                    command("POST", f"/element/{editor}/value", {"text": marker, "value": list(marker)})
                    click("//button[normalize-space(.)='Send']")
                messages = f"//*[contains(@class,'message-content') and contains(.,'{marker}')]"
                wait(lambda: len(elements(messages)) == 1, "exact persisted message")
                wait(lambda: visible("//*[contains(@class,'ws-indicator') and contains(@class,'connected')]"),
                     "connected native session")
                (args.output / f"{args.kind}-{phase}.png").write_bytes(
                    base64.b64decode(command("GET", "/screenshot")))
                receipt = dict(phase=phase, kind=args.kind, nativeWebKit=True, realHostStartup=True,
                               founderBadge=True, storedMessageVisible=True, mocks=False,
                               inAppUpdaterTested=False)
                (args.output / f"{args.kind}-{phase}.json").write_text(json.dumps(receipt, indent=2) + "\n")
                print(json.dumps(receipt), flush=True)
            finally:
                # Preserve native-page diagnostics if a user journey failed.
                try:
                    (args.output / f"{args.kind}-{phase}-page.html").write_text(command("GET", "/source"))
                finally:
                    command("DELETE", "", None)
                    session = None
            time.sleep(2)
    finally:
        driver.terminate()
        try:
            driver.wait(timeout=10)
        except subprocess.TimeoutExpired:
            driver.kill()
            driver.wait()
