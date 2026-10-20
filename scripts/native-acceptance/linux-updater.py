"""Exercise the real installed AppImage updater using native WebKit UI."""
import argparse
import base64
import hashlib
import json
import os
from pathlib import Path
import signal
import subprocess
import time
import urllib.error
import urllib.request

parser = argparse.ArgumentParser()
parser.add_argument("--executable", type=Path, required=True)
args = parser.parse_args()
assert os.environ.get("GITHUB_ACTIONS") == "true"
executable = args.executable.resolve(strict=True)
output = Path("native-evidence").resolve()
inventory = json.loads(Path("candidate/array-installation.json").read_text())
expected = next(row for row in inventory["artifacts"] if row["kind"] == "appimage")
mode = Path(os.environ["RUNNER_TEMP"]) / "annex-updater-mode.txt"
endpoint = "http://127.0.0.1:4445"
element_key = "element-6066-11e4-a52e-4f735466cecf"
session = None
marker = "Annex real AppImage update preserves the native founder and message"


def sha(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


before = sha(executable)
assert before != expected["sha256"]


def request(method, route, body=None):
    data = None if body is None else json.dumps(body).encode()
    try:
        with urllib.request.urlopen(urllib.request.Request(
                endpoint + route, data=data, method=method,
                headers={"Content-Type": "application/json"}), timeout=90) as response:
            result = json.load(response)
    except urllib.error.HTTPError as error:
        raise RuntimeError(error.read().decode()[:2000]) from error
    value = result.get("value")
    if isinstance(value, dict) and value.get("error"):
        raise RuntimeError(json.dumps(value)[:2000])
    return value


def command(method, route, body=None):
    return request(method, f"/session/{session}" + route, body)


def visible(xpath):
    for item in command("POST", "/elements", {"using": "xpath", "value": xpath}):
        identifier = item[element_key]
        rect = command("GET", f"/element/{identifier}/rect")
        visibility = command("GET", f"/element/{identifier}/css/visibility")
        if rect["width"] > 0 and rect["height"] > 0 and visibility not in ("hidden", "collapse"):
            return identifier
    return None


def wait(check, description, seconds=120):
    expires = time.monotonic() + seconds
    last = None
    while time.monotonic() < expires:
        try:
            result = check()
            if result:
                return result
        except (RuntimeError, OSError) as error:
            last = str(error)
        time.sleep(0.5)
    raise RuntimeError(f"Timed out waiting for {description}: {last}")


def click(title):
    element = wait(lambda: visible(f"//button[normalize-space(.)='{title}']"), title)
    command("POST", f"/element/{element}/click", {})


def screenshot(name):
    (output / f"{name}.png").write_bytes(base64.b64decode(command("GET", "/screenshot")))


def start_session():
    global session
    created = request("POST", "/session", {"capabilities": {"alwaysMatch": {
        "webkitgtk:browserOptions": {"binary": str(executable), "args": []}
    }}})
    session = created["sessionId"]


def assert_founder_and_message(create=False):
    host, admin = "//button[normalize-space(.)='Start Hosting']", "//button[@title='Admin']"
    wait(lambda: visible(host) or visible(admin), "native Host entry")
    if visible(host):
        click("Start Hosting")
    wait(lambda: visible(admin), "real native founder", 180)
    click("Identity")
    wait(lambda: visible("//*[normalize-space(.)='Server founder']"), "founder badge")
    close = wait(lambda: visible("//*[@role='dialog']//button[normalize-space(.)='Close']"), "close identity")
    command("POST", f"/element/{close}/click", {})
    click("Chat")
    general = wait(lambda: visible("//button[contains(@class,'channel-select') and contains(.,'General')]"), "General")
    command("POST", f"/element/{general}/click", {})
    if create:
        editor = wait(lambda: visible("//*[@placeholder='Type a message...']"), "message editor")
        command("POST", f"/element/{editor}/value", {"text": marker, "value": list(marker)})
        click("Send")
    wait(lambda: visible(f"//*[contains(@class,'message-content') and contains(.,'{marker}')]"),
         "persisted native message")


def candidate_process():
    for directory in Path("/proc").iterdir():
        if not directory.name.isdigit():
            continue
        binary = directory / "exe"
        try:
            if binary.resolve().name == "annex-desktop" and sha(binary) == expected["executableSha256"]:
                return int(directory.name)
        except (OSError, PermissionError):
            continue
    return None


environment = dict(os.environ, TAURI_WEBVIEW_AUTOMATION="true")
with (output / "updater-webdriver.log").open("a") as log:
    driver = subprocess.Popen(["WebKitWebDriver", "--host=127.0.0.1", "--port=4445"],
                              env=environment, stdout=log, stderr=log)
    try:
        wait(lambda: request("GET", "/status").get("ready"), "native WebKit", 30)
        start_session()
        assert_founder_and_message(create=True)
        click("Updates")
        wait(lambda: visible("//*[normalize-space(.)='Installed version: 0.1.2']"), "test baseline version")
        click("Check for updates")
        wait(lambda: visible("//*[normalize-space(.)='Available version: 0.1.3']"), "real signed candidate")
        stages = []
        for transfer in ("stall", "corrupt", "normal"):
            mode.write_text(transfer)
            click("Download update")
            if transfer == "normal":
                wait(lambda: visible("//*[normalize-space(.)='Signature verified. Ready to install.']"),
                     "actual signature verification", 180)
            else:
                message = "stopped receiving data" if transfer == "stall" else "signature could not be verified"
                wait(lambda: visible(f"//*[@role='alert' and contains(.,'{message}')]"),
                     f"{transfer} rejected", 150)
            assert sha(executable) == before, "Download must never modify the installed AppImage"
            screenshot(f"updater-{transfer}")
            stages.append(dict(mode=transfer, installedBytesUnchanged=True))
        (output / "updater-downloads.json").write_text(json.dumps(stages, indent=2) + "\n")
        click("Install and restart")
        click("Confirm install and restart")
        wait(lambda: sha(executable) == expected["sha256"], "exact AppImage replacement", 180)
        restarted = wait(candidate_process, "automatic restart of the inventoried executable", 90)
        # The OS process restart is checked independently. WebKit's observer
        # session belongs to the exited process; reopen the verified app through
        # a fresh observer to check the persisted ordinary profile.
        try:
            command("DELETE", "")
        except (RuntimeError, OSError):
            pass
        session = None
        try:
            os.kill(restarted, signal.SIGTERM)
        except ProcessLookupError:
            # WebKit can reconnect its observer to the restarted process and
            # close it with DELETE. Its actual restart was already observed.
            pass
        wait(lambda: not Path(f"/proc/{restarted}/exe").exists(), "candidate shutdown", 30)
        start_session()
        assert_founder_and_message()
        click("Updates")
        wait(lambda: visible("//*[normalize-space(.)='Installed version: 0.1.3']"), "updated native version")
        screenshot("updater-recovery")
        (output / "result.json").write_text(json.dumps(dict(
            candidateSource=inventory["sourceCommit"], candidateVersion=inventory["version"],
            installedAppImageSha256=expected["sha256"], installedExecutableSha256=expected["executableSha256"],
            baselineVersion="0.1.2", baselineIsPublished012=False,
            baselineBuildOverrides=["version", "updater HTTPS fixture endpoint"],
            actualInAppUpdater=True, actualAppImageReplacement=True, automaticRestart=True,
            observedRestartPid=restarted, ordinaryProfile=True, founderAndMessagePreserved=True,
            stalledDownloadRecovered=True, corruptSignatureRejected=True, productionOriginTested=False,
            recoveryObserverReopened=True), indent=2) + "\n")
    finally:
        if session:
            try:
                (output / "final-page.html").write_text(command("GET", "/source"))
                command("DELETE", "")
            except (RuntimeError, OSError):
                pass
        driver.terminate()
        try:
            driver.wait(timeout=10)
        except subprocess.TimeoutExpired:
            driver.kill()
            driver.wait()
