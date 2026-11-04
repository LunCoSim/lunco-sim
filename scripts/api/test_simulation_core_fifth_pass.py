#!/usr/bin/env python3
"""Run authored fifth-pass simulation regressions with the production API."""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import time

from runtime import BINARY, ROOT, ProductionSession, run_authored_scenario
from test_simulation_core_fourth_pass import tire_ops


CASES = {
    "modelica": ("simulation_core_fifth_modelica", "MODELICA FIFTH PASS", 10),
    "joints": ("simulation_core_fifth_joints", "SIMULATION CORE FIFTH JOINTS", 16),
    "gravity": ("simulation_core_fifth_gravity", "SIMULATION CORE FIFTH GRAVITY", 10),
    "allocator": ("simulation_core_fifth_allocator", "SIMULATION CORE FIFTH ALLOCATOR", 14),
    "dual-actuator": ("simulation_core_fifth_actuation", "SIMULATION CORE FIFTH ACTUATION dual", 10),
    "mass-wake": ("simulation_core_fifth_actuation", "SIMULATION CORE FIFTH ACTUATION mass_wake", 8),
    "inverse-mass-overflow": ("simulation_core_fifth_actuation", "SIMULATION CORE FIFTH ACTUATION inverse_mass_overflow", 10),
    "accumulation-overflow": ("simulation_core_fifth_actuation", "SIMULATION CORE FIFTH ACTUATION accumulation_overflow", 10),
    "inverse-inertia-overflow": ("simulation_core_fifth_actuation", "SIMULATION CORE FIFTH ACTUATION inverse_inertia_overflow", 10),
    "tires-dynamic": ("simulation_core_fifth_tires", "SIMULATION CORE FIFTH TIRES dynamic", 10),
    "tires-comoving": ("simulation_core_fifth_tires", "SIMULATION CORE FIFTH TIRES comoving", 10),
    "tires-stationary": ("simulation_core_fifth_tires", "SIMULATION CORE FIFTH TIRES stationary", 10),
}


def operation(kind: str, **fields) -> dict:
    return {kind: {"edit_target": "@runtime@", **fields}}


def attribute(path: str, name: str, kind: str, value: str) -> dict:
    return operation("SetAttribute", path=path, name=name, type_name=kind, value=value)


def actuation_ops(mode: str) -> tuple[list[dict], dict]:
    """Create canonical test facts through the typed document operation owner."""
    body = "/World/FifthActuationBody"
    mount = body + "/DualMount"
    mass = "0.1" if mode == "inverse_mass_overflow" else "1"
    inertia = "(1e-10, 1e-10, 1e-10)" if mode == "inverse_inertia_overflow" else "(1, 1, 1)"
    ops = [operation("SetActive", path="/World/" + name, active=False)
           for name in ("Source", "Target", "ForceBody")]
    ops += [attribute("/World/Physics", "physics:gravityMagnitude", "float", "10" if mode == "mass_wake" else "0"),
            attribute("/World/Physics", "physics:gravityDirection", "vector3f", "(0, -1, 0)"),
            operation("AddPrim", parent_path="/World", name="FifthActuationBody", type_name="Xform"),
            operation("SetApiSchemas", path=body, schemas=["PhysicsRigidBodyAPI", "PhysicsMassAPI"]),
            operation("SetTranslate", path=body, value=[0.0, 30.0 if mode == "mass_wake" else 10.0, 0.0]),
            attribute(body, "physics:mass", "float", mass),
            attribute(body, "physics:diagonalInertia", "float3", inertia),
            attribute(body, "physics:centerOfMass", "point3f", "(0, 0, 0)"),
            attribute(body, "physxRigidBody:linearDamping", "float", "0"),
            attribute(body, "physxRigidBody:angularDamping", "float", "0")]
    if mode == "dual":
        ops += [operation("AddPrim", parent_path=body, name="DualMount", type_name="Xform"),
                operation("SetApiSchemas", path=mount, schemas=["LunCoForceActuatorAPI", "LunCoTorqueActuatorAPI"]),
                attribute(mount, "lunco:forceActuator:direction", "float3", "(0, 1, 0)"),
                attribute(mount, "lunco:forceActuator:maxForce", "double", "10"),
                attribute(mount, "lunco:torqueActuator:axis", "float3", "(0, 1, 0)"),
                attribute(mount, "lunco:torqueActuator:maxTorque", "double", "10")]
    elif mode == "mass_wake":
        joint = "/World/FifthSleepSpring"
        ops += [operation("AddPrim", parent_path="/World", name="FifthSleepSpring", type_name="PhysicsPrismaticJoint"),
                operation("SetApiSchemas", path=joint, schemas=["PhysicsDriveAPI:linear"]),
                operation("SetRelationship", path=joint, name="physics:body1", targets=[body]),
                attribute(joint, "physics:localPos0", "point3f", "(0, 30, 0)"),
                attribute(joint, "physics:axis", "token", '"Y"'),
                attribute(joint, "physics:lowerLimit", "float", "-10"),
                attribute(joint, "physics:upperLimit", "float", "10")]
        for name, kind, value in (("type", "token", '"force"'), ("stiffness", "float", "100"),
                                  ("damping", "float", "30"), ("targetPosition", "float", "0"),
                                  ("maxForce", "float", "10000")):
            ops.append(attribute(joint, "drive:linear:physics:" + name, kind, value))
    return ops, {"mode": mode, "body_path": body, "mount_path": mount}


def raycast_ops(mode: str) -> tuple[list[dict], dict]:
    """Reuse the admitted rover/deck fixture, selecting raycast realization."""
    ops, _ = tire_ops("physical")
    rover = "/World/FourthTire"
    deck = "/World/MovingDeck"
    adjusted = []
    for entry in ops:
        kind, fields = next(iter(entry.items()))
        if mode == "stationary" and (fields.get("path") == "/World/DeckSlide"
                                     or fields.get("name") == "DeckSlide"):
            continue
        if kind == "SetVariantSelection":
            fields["variant"] = "raycast"
        if kind == "SetTranslate" and fields["path"] == rover:
            fields["value"] = [0.0, 1.051, 0.0]
        if mode == "stationary" and kind == "SetApiSchemas" and fields["path"] == deck:
            fields["schemas"] = ["PhysicsCollisionAPI"]
        adjusted.append(entry)
    if mode == "comoving":
        for path in (rover, deck):
            adjusted.append(attribute(path, "physics:velocity", "vector3f", "(0, 0, 2)"))
    bodies = [rover] if mode == "stationary" else [rover, deck]
    return adjusted, {"mode": mode, "rover_path": rover, "deck_path": deck, "body_paths": bodies,
                      "momentum_tolerance": 0.1, "velocity_tolerance": 0.01,
                      "force_tolerance": 0.01, "spin_tolerance": 0.01}


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port", type=int, default=4943)
    parser.add_argument("--case", action="append", choices=list(CASES))
    args = parser.parse_args()
    evidence = ROOT / ".cache/codex-evidence/simulation-core-fifth-fixes-20261104"
    evidence.mkdir(parents=True, exist_ok=True)
    os.environ.update(LUNCOSIM_ISOLATED_RUN="1", LUNCOSIM_EPHEMERAL_SETTINGS="1")
    results = []
    scene = ROOT / "assets/scenes/tests/simulation_core_contracts.usda"
    for case in args.case or CASES:
        scenario, title, checks = CASES[case]
        source_hash = hashlib.sha256(scene.read_bytes()).hexdigest()
        scenario_file = ROOT / "assets/scenarios/tests" / (scenario + ".rhai")
        scenario_hash = hashlib.sha256(scenario_file.read_bytes()).hexdigest()
        os.environ["LUNCOSIM_CONFIG"] = str(evidence / (case + "-config"))
        process = None
        with ProductionSession(args.port, windowed=True, extra_args=["--scene", str(scene)],
                               log_path=evidence / (case + ".log")) as session:
            process = session.process
            pid = process.pid
            if Path(f"/proc/{pid}/cwd").resolve() != ROOT or Path(f"/proc/{pid}/exe").resolve() != BINARY.resolve():
                raise RuntimeError("owned process identity does not match the requested production checkout")

            def execute(command, **params):
                response = session.post({"type": "ExecuteCommand", "command": command, "params": params})
                if response.get("error"):
                    raise RuntimeError(response)
                return response.get("data", {})

            def wait(predicate, description):
                deadline = time.monotonic() + 100
                while time.monotonic() < deadline:
                    value = predicate()
                    if value:
                        return value
                    if process.poll() is not None:
                        raise RuntimeError(f"production exited while waiting for {description}")
                    time.sleep(.1)
                raise RuntimeError(f"timed out waiting for {description}")

            def admitted():
                state = execute("GetReadiness")
                return state.get("ready") is True and state.get("world_hold") is False

            wait(lambda: execute("RunRhai", code='find_path("/World/Source")>=0')["stdout"] == "true"
                 and admitted(), "exact source fixture admission")
            docs = execute("ListOpenDocuments")["open_documents"]
            doc = next(d["doc_id"] for d in docs if d.get("origin", {}).get("path") == str(scene))
            twin = int(execute("RunRhai", code='query("ReadActiveTwinContract").twin_id')["stdout"])
            params = {"doc_id": doc}
            ops = []
            if case.startswith("tires-"):
                ops, fixture_params = raycast_ops(case.removeprefix("tires-"))
                params.update(fixture_params)
            elif case not in ("modelica", "joints", "allocator", "gravity"):
                mode = {"dual-actuator": "dual", "mass-wake": "mass_wake"}.get(case, case.replace("-", "_"))
                ops, fixture_params = actuation_ops(mode)
                params.update(fixture_params)
            if ops:
                execute("SetTimeTransport", playing=False)
                execute("ApplyUsdOps", doc_id=doc, ops=ops)
                execute("SetTimeTransport", playing=True)
                subject = params.get("rover_path", params.get("body_path"))
                wait(lambda: execute("RunRhai", code=f"find_path({json.dumps(subject)})>=0")["stdout"] == "true"
                     and admitted(), "authored runtime fixture admission")
                (evidence / (case + "-operations.json")).write_text(json.dumps(ops, indent=2) + "\n")
            execute("SetTimeTransport", playing=True)
            negative = case in ("modelica", "joints", "allocator", "gravity") or case.endswith("overflow")
            run_authored_scenario(session, f"lunco://scenarios/tests/{scenario}.rhai", title=title,
                                  check_count=checks, params=params, owner_twin_id=twin,
                                  allow_runtime_fault=negative, timeout_s=150)
            results.append({"case": case, "title": title, "authored_verdict": "PASS",
                            "checks": checks, "pid": pid, "port": args.port,
                            "readiness": execute("GetReadiness"), "diagnostics": execute("RuntimeDiagnostics"),
                            "scene_sha256": source_hash, "scenario_sha256": scenario_hash,
                            "binary": str(BINARY.resolve())})
        exit_code = process.returncode
        if exit_code != 0:
            raise RuntimeError(f"production exited with {exit_code} after {case}")
        if hashlib.sha256(scene.read_bytes()).hexdigest() != source_hash:
            raise RuntimeError("authored source fixture was modified")
        if hashlib.sha256(scenario_file.read_bytes()).hexdigest() != scenario_hash:
            raise RuntimeError("authored scenario changed during validation")
        results[-1]["exit_code"] = exit_code
        results[-1]["owned_api_exit"] = True
        (evidence / ("result-" + case + ".json")).write_text(json.dumps(results[-1], indent=2) + "\n")
        print(f"PASS {case}: authored verdict, source unchanged, production exit={exit_code}", flush=True)
    (evidence / ("results-" + "-".join(item["case"] for item in results) + ".json")).write_text(
        json.dumps(results, indent=2) + "\n")


if __name__ == "__main__":
    main()
