#!/usr/bin/env python3
"""Exercise the File menu during a pending load; Rhai owns retirement verdicts."""

import argparse
import json
from pathlib import Path
import subprocess
import time

from runtime import BINARY, ROOT, ProductionSession, get_json, wait_for_ready


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port", type=int, default=4198)
    # Logical pixel positions measured from the production screenshot at the
    # default desktop scale. Override them for another window/font scale.
    parser.add_argument("--file", type=float, nargs=2, default=(18, 14))
    parser.add_argument("--recent", type=float, nargs=2, default=(100, 120))
    parser.add_argument("--replacement", type=float, nargs=2, default=(250, 140))
    args = parser.parse_args()
    candidate = ROOT / "assets/scenes/tests/twin_session_retirement/replacement"
    source = (ROOT / "assets/scenarios/tests/twin_loading_replacement.rhai").read_text()

    with ProductionSession(args.port, windowed=True,
                           log_path=ROOT / "target/twin-loading-replacement.log") as session:
        process = Path("/proc") / str(session.process.pid)
        if process.exists():
            if (process.joinpath("cwd").resolve() != ROOT or
                    process.joinpath("exe").resolve() != BINARY.resolve()):
                raise RuntimeError("the owned runtime does not match this checkout and binary")
        print(f"Owned runtime: {BINARY}, PID={session.process.pid}, port={args.port}", flush=True)

        def execute(command, **params):
            response = session.post({"type": "ExecuteCommand", "command": command, "params": params})
            if response.get("error"):
                raise RuntimeError(f"{command}: {response}")
            return response.get("data", {})

        def wait_for(predicate, label):
            deadline = time.monotonic() + 30
            while time.monotonic() < deadline:
                result = predicate()
                if result:
                    return result
                time.sleep(.01)
            raise RuntimeError(f"timed out waiting for {label}; log={session.log_path}")

        def verdict(function, *values):
            call = function + "(" + ",".join(json.dumps(v) for v in values) + ")"
            result = subprocess.run([str(BINARY), "rhai", "--api", str(args.port), "--stdout",
                                     "-e", source + '\nif !' + call + ' { throw "loading replacement failed"; }'],
                                    cwd=ROOT, capture_output=True, text=True, timeout=30)
            print(result.stdout.strip(), flush=True)
            if result.returncode or "TESTS_OK" not in result.stdout or "TESTS_FAIL" in result.stdout:
                raise RuntimeError(result.stdout + result.stderr)

        def move(point):
            execute("InjectWindowInput", event={"PointerMove": dict(zip(("x", "y"), point))})
            time.sleep(.025)

        def click(point):
            move(point)
            for state in ("Pressed", "Released"):
                execute("InjectWindowInput", event={"PointerButton": {
                    "button": "Primary", "state": state, "x": point[0], "y": point[1]}})
                time.sleep(.025)

        def pending_load():
            ready = get_json(args.port, "/api/ready")["data"]
            return any(item["kind"] == "scene_load" for item in ready["pending"])

        def replacement_projected():
            try:
                prim = execute("QueryUsdPrim", path="/World")
            except RuntimeError as error:
                # Between retirement and the replacement mount there is no
                # live stage to query. Require a uniquely mounted stage and
                # its projected root before evaluating the Rhai verdict.
                if ("HTTP 404" in str(error) or
                        "QueryUsdPrim: no USD stage loaded" in str(error) or
                        "QueryUsdPrim: exactly one mounted live stage is required" in str(error)):
                    return False
                raise
            return prim.get("spawned") is True

        execute("OpenTwin", path=str(candidate))
        wait_for(lambda: execute("ListOpenDocuments")["open_documents"], "replacement document")
        wait_for(replacement_projected, "initial replacement scene projection")
        wait_for_ready(args.port)
        retained = [doc["doc_id"] for doc in execute("ListOpenDocuments")["open_documents"]]
        execute("OpenTwin", path=str(ROOT / "target/no-such-loading-replacement-twin"))
        verdict("verify_rejected_loading_replacement", retained)

        execute("ActivatePerspective", id="sandbox_view")
        outgoing_root = ROOT / "assets/scenes/luncosim"
        execute("OpenTwin", path=str(outgoing_root))
        wait_for(lambda: execute("ListTwin", limit=0).get("root") == str(outgoing_root),
                 "outgoing Twin admission")
        wait_for(lambda: any(doc["origin"].get("path") == str(outgoing_root / "sandbox_scene.usda")
                             for doc in execute("ListOpenDocuments")["open_documents"]),
                 "outgoing scene document")
        wait_for(pending_load, "an in-flight sandbox scene")
        outgoing = [doc["doc_id"] for doc in execute("ListOpenDocuments")["open_documents"]]
        recents = execute("ListRecentFiles")["recent_twins"]
        if [item["path"] for item in recents[:2]] != [str(ROOT / "assets/scenes/luncosim"), str(candidate)]:
            raise RuntimeError("the replacement must be the second recent Twin")
        click(args.file)
        move(args.recent)
        move(args.replacement)
        session.capture_screenshot(ROOT / "target/twin-loading-menu.png")
        if not pending_load():
            raise RuntimeError("load completed before the menu replacement gesture; no cancellation proof")
        click(args.replacement)
        wait_for(lambda: execute("ListTwin", limit=0).get("root") == str(candidate), "menu-selected replacement")
        wait_for(replacement_projected, "replacement scene projection")
        wait_for_ready(args.port)
        verdict("verify_loading_replacement", outgoing, str(candidate))
        # Let running preparation results arrive after retirement, then require
        # the same owner and absence of the outgoing scene again.
        time.sleep(1)
        verdict("verify_loading_replacement", outgoing, str(candidate))
        session.capture_screenshot(ROOT / "target/twin-loading-replaced.png")
        print(f"PASS: File menu replaced a loading Twin; PID={session.process.pid}, port={args.port}")
    print("API Exit completed; owned process and port released")


if __name__ == "__main__":
    main()
