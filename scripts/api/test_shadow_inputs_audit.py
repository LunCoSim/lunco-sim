#!/usr/bin/env python3
"""Exercise the opt-in native shadow audit through authored production verdicts."""

import argparse
import json
import os
from pathlib import Path
import time

from runtime import BINARY, ROOT, ProductionSession, assert_render_log_clean, run_authored_scenario
from run_editor_scene_test import wait_for_scene, wait_for_scene_time_selection


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port", type=int, default=5560)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    os.environ.update(LUNCOSIM_CONFIG=str(output / "config"),
                      LUNCOSIM_EPHEMERAL_SETTINGS="1", LUNCOSIM_ISOLATED_RUN="1")
    scene = ROOT / "assets/scenes/tests/vertex_shadow_alignment.usda"
    params = dict(camera_path="/VertexShadowAlignment/Camera",
                  sun_path="/VertexShadowAlignment/Sun", caster_path="/VertexShadowAlignment/Caster",
                  previous_clip_signature="", expect_projection_change=False,
                  previous_pose_signature="", expect_caster_change=False, empty_world=False)
    log = output / "runtime.log"
    with ProductionSession(args.port, windowed=True, log_path=log,
                           extra_args=["--scene", str(scene), "--render-quality", "high"]) as session:
        pid = session.process.pid
        assert Path(f"/proc/{pid}/cwd").resolve() == ROOT
        assert Path(f"/proc/{pid}/exe").resolve() == BINARY.resolve()
        wait_for_scene(session, str(scene), 60)
        wait_for_scene_time_selection(session, log, 60)

        def command(name, **arguments):
            reply = session.post({"type": "ExecuteCommand", "command": name, "params": arguments})
            if reply.get("error"):
                raise RuntimeError(reply)
            return reply["data"]

        command("SetTimeTransport", playing=False)
        time.sleep(.5)
        run_authored_scenario(session, "lunco://scenarios/tests/shadow_inputs_audit.rhai",
                              title="SHADOW INPUT AUDIT", check_count=12, params=params)
        before = command("SceneCameraAudit", shadow_inputs=True)
        (output / "before.json").write_text(json.dumps(before, indent=2) + "\n")
        camera = next(row["entity"] for row in before["candidates"] if row["usd_path"] == params["camera_path"])

        def signature(snapshot):
            return "".join(str(bits) + "," for light in snapshot["shadow_inputs"]["lights"]
                           if light["usd_path"] == params["sun_path"] for cascade in light["maps"]
                           if cascade["camera"] == camera for bits in cascade["clip_from_world_bits"])

        params.update(previous_clip_signature=signature(before), expect_projection_change=True)
        command("SetEnvironmentLight", sun_pitch=-.4, sun_yaw=.3)
        deadline = time.monotonic() + 10
        while time.monotonic() < deadline:
            after = command("SceneCameraAudit", shadow_inputs=True)
            if signature(after) != params["previous_clip_signature"]:
                break
            time.sleep(.1)
        else:
            raise RuntimeError("native cascade matrices did not follow the light change")
        run_authored_scenario(session, "lunco://scenarios/tests/shadow_inputs_audit.rhai",
                              title="SHADOW INPUT AUDIT", check_count=12, params=params)
        (output / "after.json").write_text(json.dumps(after, indent=2) + "\n")

        def pose_signature(snapshot):
            return "".join(cascade["caster_fingerprints"]["poses"] + ","
                           for light in snapshot["shadow_inputs"]["lights"]
                           if light["usd_path"] == params["sun_path"] for cascade in light["maps"]
                           if cascade["camera"] == camera)

        params.update(expect_projection_change=False, previous_clip_signature=signature(after),
                      expect_caster_change=True, previous_pose_signature=pose_signature(after))
        caster = int(command("RunRhai", code='print(find_path("/VertexShadowAlignment/Caster"));')["stdout"].strip())
        pose = command("QueryEntity", id=caster)
        position = pose["position"]
        position[0] += .125
        command("TransformEntity", entity_id=caster, translation=position, rotation=pose["rotation"])
        deadline = time.monotonic() + 10
        while time.monotonic() < deadline:
            moved = command("SceneCameraAudit", shadow_inputs=True)
            if pose_signature(moved) != params["previous_pose_signature"]:
                break
            time.sleep(.1)
        else:
            raise RuntimeError("native caster pose fingerprint did not follow the pose change")
        run_authored_scenario(session, "lunco://scenarios/tests/shadow_inputs_audit.rhai",
                              title="SHADOW INPUT AUDIT", check_count=12, params=params)
        (output / "caster-after.json").write_text(json.dumps(moved, indent=2) + "\n")
        assert command("RuntimeDiagnostics") == {"errors": 0, "warnings": 0, "findings": []}
        assert_render_log_clean(log)
    with ProductionSession(args.port, log_path=output / "empty-world.log") as session:
        # An empty host has no scene lease for a retained scenario. Execute the
        # same authored callback in the command context and retain its verdict.
        source = (ROOT / "assets/scenarios/tests/shadow_inputs_audit.rhai").read_text()
        reply = session.post({"type": "ExecuteCommand", "command": "RunRhai",
                              "params": {"code": source + "\non_start(0, #{empty_world: true});"}})
        if reply.get("error"):
            raise RuntimeError(reply)
        stdout = reply["data"]["stdout"]
        (output / "empty-world-verdict.txt").write_text(stdout)
        assert "TESTS_OK 3" in stdout and "EMPTY SHADOW INPUT AUDIT: PASS" in stdout, stdout
        assert_render_log_clean(output / "empty-world.log")
    print("SHADOW INPUT AUDIT: PASS (39 authored checks; owned sessions closed)", flush=True)


if __name__ == "__main__":
    main()
