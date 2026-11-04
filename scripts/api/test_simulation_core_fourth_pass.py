#!/usr/bin/env python3
"""Run authored simulation-core regressions through the production API."""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import time

from runtime import BINARY, ROOT, ProductionSession, run_authored_scenario


CASES = {
    "modelica": ("simulation_core_contracts", "simulation_core_fourth_modelica", "MODELICA FOURTH PASS", 17),
    "joints": ("simulation_core_third_physics", "simulation_core_fourth_physics_joints", "SIMULATION CORE FOURTH JOINTS", 14),
    "geometry": ("simulation_core_contracts", "simulation_core_fourth_geometry", "SIMULATION CORE FOURTH GEOMETRY", 8),
    "actuator-range": ("simulation_core_contracts", "simulation_core_fourth_geometry", "SIMULATION CORE FOURTH GEOMETRY RANGE", 14),
    "signals": ("simulation_core_contracts", "simulation_core_fourth_signals", "SIMULATION CORE FOURTH SIGNALS", 12),
    "physical-tires": ("simulation_core_contracts", "simulation_core_fourth_signals_tires", "SIMULATION CORE FOURTH SIGNALS TIRES PHYSICAL", 9),
    "raycast-tires": ("simulation_core_contracts", "simulation_core_fourth_signals_tires", "SIMULATION CORE FOURTH SIGNALS TIRES RAYCAST", 9),
    "wheel-live-invalid": ("physical_rover_controls", "simulation_core_fourth_physics_wheel", "SIMULATION CORE FOURTH WHEEL", 7),
    "wheel-initial-invalid": ("physical_rover_controls", "simulation_core_fourth_physics_wheel", "SIMULATION CORE FOURTH WHEEL", 7),
}


def tire_ops(mode: str) -> tuple[list[dict], list[str]]:
    """Build the fixture using the document owner's typed USD operations."""
    ops = []

    def op(kind, **fields):
        ops.append({kind: {"edit_target": "@runtime@", **fields}})

    def attr(path, name, kind, value):
        op("SetAttribute", path=path, name=name, type_name=kind, value=value)

    for name in ("Source", "Target", "ForceBody"):
        op("SetActive", path="/World/" + name, active=False)
    attr("/World/Physics", "physics:gravityMagnitude", "float", "1.62" if mode == "physical" else "0.001")
    op("AddPrim", parent_path="/World", name="MovingDeck", type_name="Cube")
    schemas = ["PhysicsCollisionAPI"]
    if mode == "physical":
        schemas += ["PhysicsRigidBodyAPI", "PhysicsMassAPI"]
    op("SetApiSchemas", path="/World/MovingDeck", schemas=schemas)
    attr("/World/MovingDeck", "size", "double", "1")
    op("SetTranslate", path="/World/MovingDeck", value=[0.0, -0.5, 0.0])
    op("SetScale", path="/World/MovingDeck", value=[20.0, 1.0, 20.0])
    if mode == "physical":
        attr("/World/MovingDeck", "physics:mass", "float", "1000")
        op("AddPrim", parent_path="/World", name="DeckSlide", type_name="PhysicsPrismaticJoint")
        op("SetRelationship", path="/World/DeckSlide", name="physics:body1", targets=["/World/MovingDeck"])
        attr("/World/DeckSlide", "physics:axis", "token", '"Z"')
        attr("/World/DeckSlide", "physics:localPos0", "point3f", "(0, -0.5, 0)")
    rover = "/World/FourthTire"
    op("AddPrim", parent_path="/World", name="FourthTire", type_name="Xform",
       reference="lunco://vessels/rovers/skid_rover.usda", reference_prim_path="/SkidRover")
    op("SetVariantSelection", path=rover, variant_set="drivetrain", variant=mode)
    op("SetTranslate", path=rover, value=[0.0, 1.1 if mode == "physical" else 1.051, 0.0])
    if mode == "raycast":
        attr(rover, "physics:velocity", "float3", "(0, -0.02, 0)")
    bodies = [rover]
    for corner in ("FL", "FR", "RL", "RR"):
        suspension = rover + "/Suspension_" + corner
        wheel = suspension + "/Wheel_" + corner
        if mode == "physical":
            bodies += [suspension, wheel]
        else:
            attr(wheel, "physxVehicleSuspension:springStrength", "float", "1")
            attr(wheel, "physxVehicleSuspension:springDamperRate", "float", "0")
    if mode == "physical":
        bodies.append("/World/MovingDeck")
    for path in bodies:
        attr(path, "physxRigidBody:linearDamping", "float", "0")
        attr(path, "physxRigidBody:angularDamping", "float", "0")
    return ops, bodies


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port", type=int, default=4937)
    parser.add_argument("--case", action="append", choices=list(CASES))
    args = parser.parse_args()
    evidence = ROOT / ".cache/codex-evidence/simulation-core-fourth-fixes-20261104"
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

            def wait(predicate, message):
                deadline = time.monotonic() + 90
                while time.monotonic() < deadline:
                    value = predicate()
                    if value:
                        return value
                    if session.process.poll() is not None:
                        raise RuntimeError(f"production process exited while waiting for {message}")
                    time.sleep(.1)
                raise RuntimeError(f"timed out waiting for {message}")

            def admitted():
                state = execute("GetReadiness")
                return state.get("ready") is True and state.get("world_hold") is False

            subject = ("/PhysicalRoverControls/SkidPhysical/Suspension_FL/Wheel_FL"
                       if case.startswith("wheel-") else "/World/Host" if case == "joints" else "/World/Source")
            wait(lambda: execute("RunRhai", code=f"find_path({json.dumps(subject)})>=0")["stdout"] == "true"
                 and admitted(), "exact fixture admission")
            docs = execute("ListOpenDocuments")["open_documents"]
            doc = next(d["doc_id"] for d in docs if d.get("origin", {}).get("path") == str(scene))
            twin = int(execute("RunRhai", code='query("ReadActiveTwinContract").twin_id')["stdout"])
            execute("SetTimeTransport", playing=True)
            params = {"doc_id": doc, "native_invalid": case == "actuator-range"}
            if case == "signals":
                run_authored_scenario(session, "lunco://scenarios/tests/simulation_core_fourth_signals_setup.rhai",
                                      title="SIMULATION CORE FOURTH SIGNALS SETUP", check_count=1,
                                      params=params, owner_twin_id=twin)
                target = int(execute("RunRhai", code='find_path("/World/LayeredInput")')["stdout"])
                execute("SetPorts", target=target, writes=[["u", 1.0]], producer_id=args.port)
                wait(lambda: execute("GetPort", api_id=target, name="u").get("value") == 1.0,
                     "controller input commit")
            elif case in ("physical-tires", "raycast-tires"):
                mode = case.split("-")[0]
                ops, bodies = tire_ops(mode)
                execute("ApplyUsdOps", doc_id=doc, ops=ops)
                wait(lambda: execute("RunRhai", code='find_path("/World/FourthTire/Suspension_FL/Wheel_FL")>=0')["stdout"] == "true"
                     and admitted(), "tire fixture admission")
                params.update(mode=mode, body_paths=bodies, deck_path="/World/MovingDeck", momentum_tolerance=0.1)
            elif case.startswith("wheel-"):
                params.update(wheel_path=subject, initial_invalid=case == "wheel-initial-invalid")
            run_authored_scenario(session, f"lunco://scenarios/tests/{scenario}.rhai", title=title,
                                  check_count=checks, params=params, owner_twin_id=twin,
                                  allow_runtime_fault=case.startswith("wheel-") or case in ("joints", "actuator-range"), timeout_s=150)
            results.append({"case": case, "checks": checks + (1 if case == "signals" else 0),
                            "pid": pid, "port": args.port, "readiness": execute("GetReadiness"),
                            "source_sha256": source_hash})
        assert hashlib.sha256(scene.read_bytes()).hexdigest() == source_hash
        (evidence / ("result-" + case + ".json")).write_text(json.dumps(results[-1], indent=2) + "\n")
        print(f"PASS {case}: authored verdict, unchanged USD source, owned API Exit", flush=True)
    (evidence / ("results-" + "-".join(r["case"] for r in results) + ".json")).write_text(
        json.dumps(results, indent=2) + "\n")


if __name__ == "__main__":
    main()
