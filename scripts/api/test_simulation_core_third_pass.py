#!/usr/bin/env python3
"""Production authored gates for physical frames, units and command outcomes."""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import time

from runtime import BINARY, ROOT, ProductionSession, get_json, run_authored_scenario

CASES = {
    "mounts": ("simulation_core_contracts", "simulation_core_third_mounts", "SIMULATION CORE THIRD MOUNTS", 20),
    "physics": ("simulation_core_third_physics", "simulation_core_third_physics", "SIMULATION CORE THIRD PHYSICS", 22),
    "slip-invalid": ("simulation_core_third_physics", "simulation_core_third_physics", "SIMULATION CORE THIRD PHYSICS", 22),
    "modelica": ("simulation_core_contracts", "modelica_third_pass", "MODELICA THIRD PASS", 12),
    "mount-invalid": ("simulation_core_contracts", "simulation_core_third_mount_boundary", "SIMULATION CORE MOUNT BOUNDARY", 5),
    "torque-z": ("simulation_core_contracts_z_up", "simulation_core_third_mount_boundary", "SIMULATION CORE MOUNT BOUNDARY", 3),
    "allocator": ("simulation_core_contracts", "simulation_core_third_allocator", "SIMULATION CORE LIVE ALLOCATOR", 6),
}


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port", type=int, default=4923)
    parser.add_argument("--case", action="append", choices=list(CASES))
    args = parser.parse_args()
    evidence = ROOT / ".cache/codex-evidence/simulation-core-third-fixes-20261104"
    evidence.mkdir(parents=True, exist_ok=True)
    os.environ.update(LUNCOSIM_ISOLATED_RUN="1", LUNCOSIM_EPHEMERAL_SETTINGS="1")
    results = []
    for case in args.case or CASES:
        fixture, scenario, title, checks = CASES[case]
        scene = ROOT / "assets/scenes/tests" / (fixture + ".usda")
        source_hash = hashlib.sha256(scene.read_bytes()).hexdigest()
        os.environ["LUNCOSIM_CONFIG"] = str(evidence / (case + "-config"))
        with ProductionSession(args.port, windowed=True, extra_args=["--scene", str(scene)],
                               log_path=evidence / (case + ".log")) as session:
            pid = session.process.pid
            assert Path(f"/proc/{pid}/cwd").resolve() == ROOT
            assert Path(f"/proc/{pid}/exe").resolve() == BINARY.resolve()

            def execute(command, **params):
                response = session.post({"type": "ExecuteCommand", "command": command, "params": params})
                if response.get("error"):
                    raise RuntimeError(response)
                return response.get("data", {})

            subject = "/World/Drive" if case in ("physics", "slip-invalid") else "/World/Source"
            deadline = time.monotonic() + 90
            while True:
                if case in ("physics", "slip-invalid"):
                    participants = execute("CosimStatus", include_values=False)["entities"]
                    admitted = any(row["name"] == subject
                                   and row.get("modelica_construction_profile") is not None
                                   and not row.get("modelica_error_present") for row in participants)
                    ready = execute("GetReadiness").get("pending_count") == 0
                else:
                    admitted = execute("RunRhai", code=f'find_path({json.dumps(subject)})>=0')["stdout"] == "true"
                    ready = execute("GetReadiness").get("ready")
                if admitted and ready:
                    break
                if time.monotonic() > deadline:
                    raise RuntimeError(f"exact fixture participant did not become ready: {subject}")
                time.sleep(.1)
            docs = execute("ListOpenDocuments")["open_documents"]
            doc = next(d["doc_id"] for d in docs if d.get("origin", {}).get("path") == str(scene))
            twin = int(execute("RunRhai", code='query("ReadActiveTwinContract").twin_id')["stdout"])
            execute("SetTimeTransport", playing=True)
            params = {"doc_id": doc, "negative": case == "mount-invalid", "invalid_slip": case == "slip-invalid"}
            if case == "modelica":
                (evidence / "gui-schema.json").write_text(json.dumps(
                    get_json(session.port, "/api/commands/schema"), indent=2) + "\n")
                destination = evidence / f"modelica-save-{pid}-{time.time_ns()}"
                destination.mkdir()
                params.update(save_path=str(destination / "ThirdPassGood.mo"), invalid_save_path=str(destination))
            run_authored_scenario(session, f"lunco://scenarios/tests/{scenario}.rhai", title=title,
                                 check_count=checks, params=params, owner_twin_id=twin,
                                 allow_runtime_fault=case in ("physics", "slip-invalid", "mount-invalid"), timeout_s=120)
            session.capture_screenshot(evidence / (case + ".png"))
            results.append({"case": case, "checks": checks, "pid": pid, "port": args.port,
                            "readiness": execute("GetReadiness"), "diagnostics": execute("RuntimeDiagnostics"),
                            "source_sha256": source_hash})
        assert hashlib.sha256(scene.read_bytes()).hexdigest() == source_hash
        print(f"PASS {case}: authored verdict, unchanged source, owned API Exit", flush=True)
    (evidence / ("results-" + "-".join(r["case"] for r in results) + ".json")).write_text(
        json.dumps(results, indent=2) + "\n")


if __name__ == "__main__":
    main()
