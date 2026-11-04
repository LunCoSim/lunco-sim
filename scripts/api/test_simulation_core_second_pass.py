#!/usr/bin/env python3
"""Run the second simulation-core regression cases through authored Rhai verdicts."""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import time

from runtime import BINARY, ROOT, ProductionSession, get_json, run_authored_scenario

CASES = {
    "mass-negative": 5, "mass-tiny": 5, "live-units": 6,
    "held-delete": 3, "axis-z": 2, "compound": 2,
    "implicit-density": 2, "child-mass": 2, "wheel-mass": 4,
    "unknown-gid": 4, "number-overflow": 4,
    "inertia-tiny": 5, "scale-tiny": 5, "mass-sentinels": 2, "empty-body": 8,
    "compound-reset": 2, "child-root-mass": 2, "child-scaled": 8,
    "scale-small-valid": 4, "wheel-live-owner": 7,
    "modelica-lifecycle": 20,
}


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port", type=int, default=4893)
    parser.add_argument("--case", action="append", choices=list(CASES))
    parser.add_argument("--windowed", action="store_true")
    args = parser.parse_args()
    evidence = ROOT / ".cache/codex-evidence/simulation-core-second-fixes-20261104"
    evidence.mkdir(parents=True, exist_ok=True)
    os.environ.update(LUNCOSIM_ISOLATED_RUN="1", LUNCOSIM_EPHEMERAL_SETTINGS="1")
    results = []
    for case in args.case or CASES:
        scene_name = ("paused_wheel_effort" if case.startswith("wheel-") else
                      "simulation_core_contracts_z_up" if case == "axis-z" else
                      "simulation_core_contracts")
        scene = ROOT / "assets/scenes/tests" / f"{scene_name}.usda"
        source_hash = hashlib.sha256(scene.read_bytes()).hexdigest()
        os.environ["LUNCOSIM_CONFIG"] = str(evidence / f"{case}-config")
        log = evidence / f"{case}.log"
        windowed = args.windowed or case=="modelica-lifecycle"
        with ProductionSession(args.port, windowed=windowed,
                               extra_args=["--scene", str(scene)], log_path=log) as session:
            pid = session.process.pid
            assert Path(f"/proc/{pid}/cwd").resolve() == ROOT
            assert Path(f"/proc/{pid}/exe").resolve() == BINARY.resolve()

            def command(name, **params):
                response = session.post({"type": "ExecuteCommand", "command": name, "params": params})
                if response.get("error"):
                    raise RuntimeError(response)
                return response.get("data", {})

            def rhai(code):
                return command("RunRhai", code=code)["stdout"]

            deadline = time.monotonic() + 90
            path = "/World/Raycast" if case.startswith("wheel-") else "/World/Target"
            while not (command("GetReadiness").get("ready") and
                       rhai(f'find_path("{path}")>=0 && clock_snapshot().sim_tick>=30') == "true"):
                if time.monotonic() > deadline or session.process.poll() is not None:
                    raise RuntimeError(f"startup fixture did not settle: {log}")
                time.sleep(.1)
            documents = command("ListOpenDocuments")["open_documents"]
            if windowed:
                (evidence / f"{case}-schema.json").write_text(
                    json.dumps(get_json(session.port, "/api/commands/schema"), indent=2) + "\n")
            doc_id = next(d["doc_id"] for d in documents if d.get("origin", {}).get("path") == str(scene))
            twin_id = int(rhai('query("ReadActiveTwinContract").twin_id'))
            scenario = ("modelica_checked_lifecycle" if case=="modelica-lifecycle"
                        else "simulation_core_second_pass")
            title = ("MODELICA CHECKED LIFECYCLE" if case=="modelica-lifecycle"
                     else "SIMULATION CORE SECOND PASS")
            params = {"test_case": case, "doc_id": doc_id}
            if case=="modelica-lifecycle":
                save_directory = evidence / f"modelica-saveas-{pid}-{time.time_ns()}"
                save_directory.mkdir()
                params["save_as_path"] = str(save_directory / "CheckedLifecycle.mo")
                params["save_as_invalid_path"] = str(save_directory)
            run_authored_scenario(session, f"lunco://scenarios/tests/{scenario}.rhai",
                                 title=title, check_count=CASES[case],
                                 owner_twin_id=twin_id,
                                 allow_runtime_fault=case in ["mass-negative", "mass-tiny", "inertia-tiny", "scale-tiny", "live-units"],
                                 params=params, timeout_s=120)
            if windowed:
                session.capture_screenshot(evidence / f"{case}.png")
            results.append({"case": case, "checks": CASES[case], "pid": pid, "port": args.port,
                            "scenario_params": params,
                            "readiness": command("GetReadiness"),
                            "diagnostics": command("RuntimeDiagnostics"), "log": str(log)})
        assert hashlib.sha256(scene.read_bytes()).hexdigest() == source_hash
        print(f"PASS {case}: authored verdict, source preservation and API Exit", flush=True)
    (evidence / ("results-" + "-".join(r["case"] for r in results) + ".json")).write_text(
        json.dumps(results, indent=2) + "\n")


if __name__ == "__main__":
    main()
