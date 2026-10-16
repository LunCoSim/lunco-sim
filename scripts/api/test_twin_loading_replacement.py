#!/usr/bin/env python3
"""Exercise pending-load menu replacement, queued teardown and supersession."""

import argparse
import json
from pathlib import Path
import subprocess
import time

from runtime import BINARY, ROOT, ProductionSession, get_json, wait_for_ready


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port", type=int, default=4198)
    parser.add_argument("--phase", choices=("loading", "physics", "late", "all"), default="all",
                        help="menu replacement, native physics admission, fault recovery, or all phases")
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

        def wait_for(predicate, label, timeout=30):
            deadline = time.monotonic() + timeout
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

        def prim_projected(path="/World"):
            try:
                prim = execute("QueryUsdPrim", path=path)
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

        def scene_cleared():
            if pending_load():
                return False
            try:
                execute("QueryUsdPrim", path="/SandboxScene")
            except RuntimeError as error:
                if "QueryUsdPrim: exactly one mounted live stage is required" in str(error):
                    return True
                raise
            return False

        def physics_sample_available():
            try:
                return execute("PhysicsPerformance")
            except RuntimeError as error:
                if "no completed Avian steps are available yet" in str(error):
                    return False
                raise

        execute("OpenTwin", path=str(candidate))
        wait_for(lambda: execute("ListOpenDocuments")["open_documents"], "replacement document")
        wait_for(prim_projected, "initial replacement scene projection")
        wait_for_ready(args.port)
        # The later topology query requires a real completed Avian sample.
        # Admission can fault before the first step of the scene under test.
        execute("SetTimeTransport", playing=True)
        wait_for(physics_sample_available, "initial completed physics sample")
        if args.phase in ("loading", "all"):
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
            wait_for(prim_projected, "replacement scene projection")
            wait_for_ready(args.port)
            verdict("verify_loading_replacement", outgoing, str(candidate), "/SandboxScene")
            # Let running preparation results arrive after retirement, then require
            # the same owner and absence of the outgoing scene again.
            time.sleep(1)
            verdict("verify_loading_replacement", outgoing, str(candidate), "/SandboxScene")
            session.capture_screenshot(ROOT / "target/twin-loading-replaced.png")

            # A queued restart/clear belongs to the outgoing transaction. Replacing
            # that owner must cancel both its preparation and its pending intent.
            execute("OpenTwin", path=str(outgoing_root))
            wait_for(lambda: execute("ListTwin", limit=0).get("root") == str(outgoing_root),
                     "second outgoing Twin admission")
            wait_for(pending_load, "second in-flight sandbox scene")
            outgoing = [doc["doc_id"] for doc in execute("ListOpenDocuments")["open_documents"]]
            execute("RestartScene")
            execute("ClearScene")
            execute("OpenTwin", path=str(ROOT / "target/no-such-loading-replacement-twin"))
            verdict("verify_rejected_loading_replacement", outgoing)
            execute("OpenTwin", path=str(candidate))
            wait_for(lambda: execute("ListTwin", limit=0).get("root") == str(candidate),
                     "replacement after queued clear")
            wait_for(prim_projected, "replacement after outgoing cancellation")
            wait_for_ready(args.port)
            verdict("verify_loading_replacement", outgoing, str(candidate), "/SandboxScene")
            time.sleep(1)
            verdict("verify_loading_replacement", outgoing, str(candidate), "/SandboxScene")

            # Submit replacement requests in command order without waiting for the
            # intermediate scans to commit; only the final candidate may publish.
            outgoing = [doc["doc_id"] for doc in execute("ListOpenDocuments")["open_documents"]]
            for root in (outgoing_root, candidate.parent, outgoing_root, candidate):
                execute("OpenTwin", path=str(root))
            wait_for(lambda: execute("ListTwin", limit=0).get("root") == str(candidate)
                     and not set(outgoing).intersection(
                         doc["doc_id"] for doc in execute("ListOpenDocuments")["open_documents"]),
                     "final rapid replacement admission")
            wait_for(prim_projected, "final rapid replacement projection")
            wait_for_ready(args.port)
            verdict("verify_loading_replacement", outgoing, str(candidate), "/SandboxScene")
            time.sleep(1)
            verdict("verify_loading_replacement", outgoing, str(candidate), "/SandboxScene")

            # Clear without a successor must reach an empty, usable viewport and
            # stay empty when delayed preparation completes.
            execute("OpenTwin", path=str(outgoing_root))
            wait_for(lambda: execute("ListTwin", limit=0).get("root") == str(outgoing_root),
                     "clear-only outgoing Twin admission")
            wait_for(pending_load, "clear-only in-flight scene")
            outgoing = [doc["doc_id"] for doc in execute("ListOpenDocuments")["open_documents"]]
            execute("ClearScene")
            wait_for(scene_cleared, "empty viewport after pending load clears")
            wait_for_ready(args.port)
            verdict("verify_cleared_loading", str(outgoing_root), "/SandboxScene")
            session.capture_screenshot(ROOT / "target/twin-loading-cleared.png")
            time.sleep(1)
            verdict("verify_cleared_loading", str(outgoing_root), "/SandboxScene")
            execute("OpenTwin", path=str(candidate))
            wait_for(prim_projected, "successful replacement after clear-only cancellation")
            wait_for_ready(args.port)
            verdict("verify_loading_replacement", outgoing, str(candidate), "/SandboxScene")
        if args.phase in ("physics", "all"):
            cases = [
                (ROOT / "assets/tutorials/basic/slope_test.usda", "/SlopeTest", "/Rover/HingeL"),
                (ROOT / "assets/scenes/tests/battery_mounts.usda", "/BatteryMountsTest", "/RockerBogie/HingeL"),
            ]
            for source_path, retired_root, joint in cases:
                execute("OpenFile", path=str(source_path))
                wait_for(lambda: any(doc.get("origin", {}).get("path") == str(source_path)
                                     for doc in execute("ListOpenDocuments")["open_documents"]),
                         "the exact articulated source document")

                def native_admission_pending():
                    if not prim_projected(retired_root + joint):
                        return False
                    state = get_json(args.port, "/api/ready")["data"]
                    topology = execute("PhysicsPerformance")
                    return state if (state["world_hold"] and not state["ready"]
                                     and topology["pending_joints"] > 0) else False

                state = wait_for(native_admission_pending,
                                 "projected native joints awaiting admission", timeout=90)
                retired = [doc["doc_id"] for doc in execute("ListOpenDocuments")["open_documents"]]
                print("Observed native admission: " + json.dumps(state), flush=True)
                execute("OpenTwin", path=str(candidate))
                wait_for(lambda: execute("ListTwin", limit=0).get("root") == str(candidate),
                         "replacement of articulated loading Twin")
                wait_for(prim_projected, "replacement after native admission cancellation")
                wait_for_ready(args.port)
                verdict("verify_loading_replacement", retired, str(candidate), retired_root)
                time.sleep(1)
                verdict("verify_loading_replacement", retired, str(candidate), retired_root)
            session.capture_screenshot(ROOT / "target/twin-physics-admission-replaced.png")
        if args.phase in ("late", "all"):
            cases = [
                ("late-admission", "sandbox_ramp_placement", "/SandboxRampPlacement"),
                ("terminal-fault", "modelica_initialization_failure", "/ModelicaRuntimeContracts"),
            ]
            for label, scene, retired_root in cases:
                if label == "terminal-fault":
                    execute("OpenFile", path=str(ROOT / "assets/scenes/tests" / (scene + ".usda")))
                else:
                    execute("LoadScene", path="lunco://scenes/tests/" + scene + ".usda")

                def boundary_reached():
                    state = get_json(args.port, "/api/ready")["data"]
                    kinds = {pending["kind"] for pending in state["pending"]}
                    if label == "terminal-fault":
                        return state if state.get("faulted") else False
                    return state if (state["world_hold"] and "participant_init" in kinds
                                     and "scene_load" not in kinds) else False

                state = wait_for(boundary_reached, label, timeout=90)
                if label == "terminal-fault" and state["fault"]["kind"] != "modelica-initialization-failed":
                    raise RuntimeError("unexpected terminal fault: " + str(state))
                (ROOT / ("target/twin-" + label + "-before.json")).write_text(json.dumps(state, indent=2))
                print("Observed " + label + ": " + json.dumps(state), flush=True)
                retired = [doc["doc_id"] for doc in execute("ListOpenDocuments")["open_documents"]]
                session.capture_screenshot(ROOT / ("target/twin-" + label + "-before.png"))
                cleared_twin = execute("ListTwin", limit=0)["root"]
                execute("ClearScene")
                wait_for_ready(args.port)
                verdict("verify_cleared_loading", cleared_twin, retired_root)
                session.capture_screenshot(ROOT / ("target/twin-" + label + "-cleared.png"))
                time.sleep(1)
                verdict("verify_cleared_loading", cleared_twin, retired_root)
                execute("OpenTwin", path=str(candidate))
                wait_for(lambda: execute("ListTwin", limit=0).get("root") == str(candidate)
                         and execute("ListOpenDocuments")["open_documents"]
                         and not set(retired).intersection(
                             doc["doc_id"] for doc in execute("ListOpenDocuments")["open_documents"]),
                         "fresh replacement after " + label)
                wait_for(prim_projected, "replacement projection after " + label)
                wait_for_ready(args.port)
                verdict("verify_loading_replacement", retired, str(candidate), retired_root)
                print("PASS: clear and replacement after " + label, flush=True)
        print(f"PASS: {args.phase} loading lifecycle checks; PID={session.process.pid}, port={args.port}")
    print("API Exit completed; owned process and port released")


if __name__ == "__main__":
    main()
