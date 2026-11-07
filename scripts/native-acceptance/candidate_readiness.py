"""Wait for an exact native build without treating pending work as a pass."""
import re
import time


def wait_for_native_candidate(api, run_id, source, job_name, timeout,
                              *, clock=time.monotonic, sleep=time.sleep):
    assert re.fullmatch(r"[1-9][0-9]*", str(run_id))
    assert re.fullmatch(r"[a-f0-9]{40}", source)
    assert isinstance(timeout, int) and 0 <= timeout <= 3600
    base = f"repos/seismicgear/annex/actions/runs/{run_id}"
    deadline = clock() + timeout
    previous = None
    while True:
        run = api(base)
        assert str(run["id"]) == str(run_id) and run["head_sha"] == source
        assert run["path"] == ".github/workflows/release-desktop.yml"
        assert run["event"] in ("workflow_dispatch", "push")
        inventory = api(base + "/jobs?per_page=100")
        rows = inventory["jobs"]
        assert inventory["total_count"] == len(rows) <= 100
        states = []
        for name in ("Pre-flight gates", job_name):
            matches = [row for row in rows if row["name"] == name]
            assert len(matches) <= 1
            if not matches:
                states.append("pending")
                continue
            job = matches[0]
            assert job["head_sha"] == source
            if job["status"] == "completed" and job["conclusion"] != "success":
                raise RuntimeError(f"Candidate gate failed: {name}: {job['conclusion']}")
            states.append("success" if job["conclusion"] == "success" else "pending")
        if states == ["success", "success"]:
            return
        if run["status"] == "completed":
            raise RuntimeError("Completed candidate run did not provide required successful native gates")
        if clock() >= deadline:
            raise TimeoutError("Exact native candidate did not finish before the qualification deadline")
        if states != previous:
            print(f"Waiting for exact candidate {source[:12]}: {job_name}", flush=True)
            previous = states
        sleep(min(30, deadline - clock()))
