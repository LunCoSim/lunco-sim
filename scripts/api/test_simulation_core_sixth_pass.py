#!/usr/bin/env python3
"""Run authored sixth-pass simulation contracts through the production API."""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import time

from runtime import BINARY, ROOT, ProductionSession, run_authored_scenario, assert_render_log_clean
from test_simulation_core_fifth_pass import operation, attribute

CASES = {
    "numeric-finite": ("numeric", "finite", 5, False),
    "numeric-overflow": ("numeric", "overflow", 8, True),
    "numeric-inertia": ("numeric", "inertia", 7, False),
    "numeric-inertia-small": ("numeric", "inertia", 7, False),
    "material-static": ("material", "static", 7, True),
    "material-terrain": ("material", "terrain", 7, True),
    "hinge": ("hinge", "", 15, True),
    "pause": ("pause", "", 11, False),
    "prescribed": ("prescribed", "", 16, False),
    "modelica": ("modelica", "", 10, True),
}


def numeric_fixture(mode: str, small: bool = False) -> tuple[list[dict], dict]:
    """Build isolated native rotors using schema-aware runtime operations."""
    ops = [operation("SetActive", path="/World/" + name, active=False)
           for name in ("Source", "Target", "ForceBody")]
    scale = 1e-30 if small else 1e-17
    moments = f"({scale}, {2*scale}, {2*scale})"
    bodies = [("SixthNumeric", moments if mode == "inertia" else "(1, 1, 1)")]
    if mode == "inertia":
        bodies.append(("SixthIsotropic", f"({scale}, {scale}, {scale})"))
    for name, inertia in bodies:
        path = "/World/" + name
        ops += [operation("AddPrim", parent_path="/World", name=name, type_name="Xform"),
                operation("SetApiSchemas", path=path, schemas=["PhysicsRigidBodyAPI", "PhysicsMassAPI"]),
                attribute(path, "physics:mass", "float", "1"),
                attribute(path, "physics:diagonalInertia", "float3", inertia),
                attribute(path, "physics:centerOfMass", "point3f", "(0, 0, 0)"),
                attribute(path, "physxRigidBody:linearDamping", "float", "0"),
                attribute(path, "physxRigidBody:angularDamping", "float", "0"),
                operation("SetTranslate", path=path, value=[0.0, 30.0, 0.0])]
    return ops, {"mode": mode, "body_path": "/World/SixthNumeric", "control_path": "/World/SixthIsotropic",
                 "torque": scale}


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port", type=int, default=4952)
    parser.add_argument("--case", action="append", choices=list(CASES))
    args = parser.parse_args()
    evidence = ROOT / ".cache/codex-evidence/simulation-core-sixth-fixes-20261104"
    evidence.mkdir(parents=True, exist_ok=True)
    os.environ.update(LUNCOSIM_ISOLATED_RUN="1", LUNCOSIM_EPHEMERAL_SETTINGS="1")
    scene = ROOT / "assets/scenes/tests/simulation_core_contracts.usda"
    binary_stat = BINARY.stat()
    with BINARY.open("rb") as executable:
        binary_hash = hashlib.file_digest(executable, "sha256").hexdigest()
    results = []
    for case in args.case or CASES:
        scenario, mode, count, negative = CASES[case]
        title = "SIMULATION CORE SIXTH " + scenario.upper() + (" " + mode if mode else "")
        if scenario == "modelica":
            title = "MODELICA SIXTH PASS"
        scenario_path = ROOT / "assets/scenarios/tests" / ("simulation_core_sixth_" + scenario + ".rhai")
        hashes = {str(path): hashlib.sha256(path.read_bytes()).hexdigest() for path in [scene, scenario_path]}
        os.environ["LUNCOSIM_CONFIG"] = str(evidence / (case + "-config"))
        with ProductionSession(args.port, windowed=True, extra_args=["--scene", str(scene)],
                               log_path=evidence / (case + ".log")) as session:
            process = session.process
            pid = process.pid
            if Path(f"/proc/{pid}/cwd").resolve() != ROOT or Path(f"/proc/{pid}/exe").resolve() != BINARY.resolve():
                raise RuntimeError("owned production process does not match checkout and binary")

            def execute(command, **params):
                result = session.post({"type": "ExecuteCommand", "command": command, "params": params})
                if result.get("error"):
                    raise RuntimeError(result)
                return result.get("data", {})

            def wait(predicate, description):
                deadline = time.monotonic() + 100
                while time.monotonic() < deadline:
                    result = predicate()
                    if result:
                        return result
                    if process.poll() is not None:
                        raise RuntimeError("production exited while waiting for " + description)
                    time.sleep(.1)
                raise RuntimeError("timed out waiting for " + description)

            def admitted():
                ready = execute("GetReadiness")
                return ready.get("ready") is True and ready.get("world_hold") is False

            wait(lambda: execute("RunRhai", code='find_path("/World/ForceBody")>=0')["stdout"] == "true"
                 and admitted(), "source fixture admission")
            doc = next(d["doc_id"] for d in execute("ListOpenDocuments")["open_documents"]
                       if d.get("origin", {}).get("path") == str(scene))
            twin = int(execute("RunRhai", code='query("ReadActiveTwinContract").twin_id')["stdout"])
            params = {"doc_id": doc, "mode": mode}
            if scenario in ("numeric", "pause", "prescribed"):
                if scenario == "numeric":
                    ops, fixture = numeric_fixture(mode, case.endswith("-small"))
                else:
                    from simulation_core_sixth_mobility_fixtures import pause_ops, prescribed_ops
                    ops, fixture = pause_ops() if scenario == "pause" else prescribed_ops()
                params.update(fixture)
                execute("SetTimeTransport", playing=False)
                execute("ApplyUsdOps", doc_id=doc, ops=ops)
                subject = fixture.get("rover_path", fixture.get("body_path"))
                wait(lambda: execute("RunRhai", code=f"find_path({json.dumps(subject)})>=0")["stdout"] == "true"
                     and admitted(), "runtime body admission")
                (evidence / (case + "-operations.json")).write_text(json.dumps(ops, indent=2) + "\n")
            execute("SetTimeTransport", playing=True)
            run_authored_scenario(session, f"lunco://scenarios/tests/simulation_core_sixth_{scenario}.rhai",
                                  title=title, check_count=count, params=params, owner_twin_id=twin,
                                  allow_runtime_fault=negative, timeout_s=150)
            result = {"case": case, "verdict": "PASS", "checks": count, "pid": pid, "port": args.port,
                      "readiness": execute("GetReadiness"), "diagnostics": execute("RuntimeDiagnostics"),
                      "source_sha256": hashes, "binary": str(BINARY.resolve()),
                      "binary_sha256": binary_hash}
        if process.returncode != 0:
            raise RuntimeError(f"production exited with {process.returncode} after {case}")
        assert_render_log_clean(evidence / (case + ".log"))
        current_stat = BINARY.stat()
        if (current_stat.st_ino, current_stat.st_mtime_ns, current_stat.st_size) != (
                binary_stat.st_ino, binary_stat.st_mtime_ns, binary_stat.st_size):
            raise RuntimeError("production binary changed during the gate")
        for path, digest in hashes.items():
            if hashlib.sha256(Path(path).read_bytes()).hexdigest() != digest:
                raise RuntimeError("authored source changed during gate: " + path)
        result.update(exit_code=process.returncode, owned_api_exit=True)
        results.append(result)
        (evidence / ("result-" + case + ".json")).write_text(json.dumps(result, indent=2) + "\n")
        print(f"PASS {case}: {count} authored assertions; production exit=0", flush=True)
    (evidence / ("results-" + "-".join(item["case"] for item in results) + ".json")).write_text(
        json.dumps(results, indent=2) + "\n")


if __name__ == "__main__":
    main()
