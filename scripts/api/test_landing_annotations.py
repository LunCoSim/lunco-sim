#!/usr/bin/env python3
"""Run the Griffin-owned annotation gates during descent, after landing and headless."""
from __future__ import annotations

import argparse
import json
import os
import time
from pathlib import Path

from runtime import (
    BINARY, ROOT, ProductionSession, assert_render_log_clean, wait_for_authored_verdict,
)


def execute(session, command, **params):
    response = session.post({"type": "ExecuteCommand", "command": command, "params": params})
    if response.get("error"):
        raise RuntimeError(f"{command}: {response}")
    return response.get("data", {})


def gate(session, root, **params):
    headless = params.get("headless", False)
    offset = session.log_path.stat().st_size
    source_identity = execute(session, "RunRhai", code="print(twin_source_identity());").get("stdout", "").strip()
    if not source_identity:
        raise RuntimeError("active Twin has no stable source identity")
    execute(session, "RunScenarioAsset", target=root,
            source_asset=f"twin://{source_identity}/scenarios/tests/griffin_landing_annotations.rhai",
            params=params)
    if not headless:
        execute(session, "StepSimulation", steps=120)
    wait_for_authored_verdict(
        session, offset=offset,
        title="GRIFFIN HEADLESS ANNOTATIONS" if headless else "GRIFFIN LANDING ANNOTATIONS",
        check_count=3 if headless else 13 if params.get("landed") else 11,
    )


def wait_landed(session):
    deadline = time.monotonic() + 300
    while time.monotonic() < deadline:
        observed = execute(session, "RunRhai", code="griffin_surface_ops::landed_status().landed")
        if observed.get("stdout", "").strip() == "true":
            return
        time.sleep(0.5)
    raise RuntimeError("production descent did not reach qualified touchdown")


def wait_mounted(session):
    deadline = time.monotonic() + 120
    code = ('let r=find_path("/Griffin1SurfaceOps"); '
            'let m=find_path("/Griffin1SurfaceOps/Scenario"); '
            'if r<0 || m<0 {-1} else {let s=query("ScriptStatus",#{target:m}); '
            'let q=query("GetReadiness"); '
            'if s.ok==true && s.state=="ready" && q.ready==true && q.world_hold==false {r} else {-1}}')
    while time.monotonic() < deadline:
        root = int(execute(session, "RunRhai", code=code)["stdout"])
        if root >= 0:
            return root
        time.sleep(0.25)
    raise RuntimeError("Griffin mission did not mount and start")


def wait_descent_route(session):
    # The route preview is a transient USD transaction. Mission readiness and
    # footprint publication can precede its projection into the live entities.
    code = '''
        let ready=true;
        for name in ["RoverRampApproach", "RoverRampExit", "RoverTarget1", "RoverTarget2", "BaseSite"] {
            let path="/Griffin1SurfaceOps/"+name;
            let id=find_path(path);
            if id<0 {ready=false;continue;}
            let p=world_pos3(id);let h=terrain_height(p.x,p.z);
            let prim=query("QueryUsdPrim",#{path:path,attrs:["lunco:billboard:fadeEnd"]});
            if h==() || p.y-h<0.049 || prim==()
                || prim.attrs["lunco:billboard:fadeEnd"]!=griffin_spec::_sysml_attribute("surfaceRouteMarkerFadeEndM") {
                ready=false;
            }
        }
        ready
    '''
    deadline = time.monotonic() + 60
    while time.monotonic() < deadline:
        execute(session, "StepSimulation", steps=1)
        if execute(session, "RunRhai", code=code).get("stdout", "").strip() == "true":
            return
        time.sleep(0.1)
    raise RuntimeError("descent route markers did not project onto the terrain")


def headless(port, out, common):
    with ProductionSession(port, log_path=out / "headless.log", extra_args=common) as session:
        root = wait_mounted(session)
        execute(session, "SetTimeTransport", playing=True)
        wait_landed(session)
        gate(session, root, headless=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--twin", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--port", type=int, default=4791)
    parser.add_argument("--headless-only", action="store_true")
    args = parser.parse_args()
    out = args.output.resolve()
    out.mkdir(parents=True, exist_ok=True)
    os.environ["LUNCOSIM_CONFIG"] = str(out / "config")
    os.environ["LUNCOSIM_EPHEMERAL_SETTINGS"] = "1"
    scene = args.twin.resolve() / "scenes/griffin_1_surface_ops.usda"
    common = ["--scene", str(scene)]
    if args.headless_only:
        headless(args.port + 1, out, common)
        print(f"Headless landing annotations verified: {out}")
        return
    with ProductionSession(args.port, windowed=True, log_path=out / "graphics.log",
                           extra_args=common + ["--render-quality", "high", "--no-vsync", "--no-throttle"]) as session:
        pid = session.process.pid
        if Path(f"/proc/{pid}/cwd").resolve() != ROOT or Path(f"/proc/{pid}/exe").resolve() != BINARY.resolve():
            raise RuntimeError("runtime ownership does not match the checkout")
        root = wait_mounted(session)
        execute(session, "StepSimulation", hold=True)
        camera = int(execute(session, "RunRhai", code='find_path("/Griffin1SurfaceOps/Avatar")')["stdout"])
        wait_descent_route(session)
        gate(session, root)
        execute(session, "SetUserCamera", name="/Griffin1SurfaceOps/Avatar")
        execute(session, "SetCameraLookAt", camera=camera, eye=[25, 32, -40], target=[3, 15, 0])
        deadline = time.monotonic() + 60
        while time.monotonic() < deadline:
            state = execute(session, "TerrainLodStatus")
            if state["stream"]["pending"] == 0 and not state["derived"]["active"] and state["derived"]["ready"] == state["derived"]["total"]:
                break
            time.sleep(0.25)
        else:
            raise RuntimeError("terrain presentation did not settle")
        session.capture_screenshot(out / "descent.png")
        execute(session, "StepSimulation", hold=False)
        execute(session, "SetTimeTransport", playing=True)
        wait_landed(session)
        execute(session, "StepSimulation", hold=True)
        gate(session, root, landed=True)
        execute(session, "SetCameraLookAt", camera=camera, eye=[14, 9, -20], target=[2, 0.5, 0])
        session.capture_screenshot(out / "landed.png")
        diagnostics = execute(session, "RuntimeDiagnostics")
        if diagnostics["errors"] or diagnostics["warnings"]:
            raise RuntimeError(f"runtime diagnostics: {diagnostics}")
        (out / "camera.json").write_text(json.dumps(execute(session, "SceneCameraAudit"), indent=2))
    assert_render_log_clean(out / "graphics.log")
    headless(args.port + 1, out, common)
    print(f"Landing annotations verified: {out}")


if __name__ == "__main__":
    main()
