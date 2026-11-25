#!/usr/bin/env python3
"""Exercise GPU Sun material boundaries through one owned production session."""
from __future__ import annotations

import argparse
import json
import math
import os
import subprocess
import sys
import time
from pathlib import Path

from runtime import BINARY, ROOT, ProductionSession, assert_render_log_clean, port_is_open
from run_editor_scene_test import wait_for_scene, wait_for_scene_time_selection


def execute(session, command, **params):
    response = session.post({"type": "ExecuteCommand", "command": command, "params": params})
    if response.get("error"):
        raise RuntimeError(f"{command}: {response['error']}")
    return response.get("data", {})


def coverage(path, expression):
    return float(subprocess.check_output([
        "convert", str(path), "-fx", expression, "-format", "%[fx:mean]", "info:"
    ], text=True))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port", type=int, default=4286)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--windowed", action="store_true", help="Inspect the same GPU gate in a desktop window")
    parser.add_argument("--scene", type=Path, default=ROOT / "assets/scenes/tests/sun_material_precision.usda")
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    os.environ.update(LUNCOSIM_EPHEMERAL_SETTINGS="1", LUNCOSIM_ISOLATED_RUN="1",
                      LUNCOSIM_CONFIG=str(output / "config"))
    scene = args.scene.resolve()
    log = output / "runtime.log"
    rows = []
    render_args = ["--scene", str(scene), "--render-quality", "high"]
    if not args.windowed:
        render_args.extend(["--offscreen", "--record-size", "640x360"])
    with ProductionSession(args.port, windowed=True, extra_args=render_args, log_path=log) as session:
        pid = session.process.pid
        if sys.platform.startswith("linux") and (
            Path(f"/proc/{pid}/cwd").resolve() != ROOT
            or Path(f"/proc/{pid}/exe").resolve() != BINARY.resolve()
        ):
            raise RuntimeError("session does not match this checkout and production binary")
        wait_for_scene(session, str(scene), 30)
        wait_for_scene_time_selection(session, log, 30)
        execute(session, "SetTimeTransport", playing=True)

        def wait_cache(predicate):
            deadline = time.monotonic() + 20
            while time.monotonic() < deadline:
                status = execute(session, "TerrainLodStatus")
                caches = status.get("shadow_caches") or []
                if len(caches) == 1 and not caches[0]["pending"] and predicate(caches[0]):
                    return caches[0]
                time.sleep(.05)
            raise RuntimeError(f"cache did not settle: {status}")

        def capture(name, sky_red=False):
            # Cache publication and light projection precede GPU extraction.
            time.sleep(.25)
            path = output / f"{name}.png"
            session.capture_screenshot(path)
            colors = {
                "green": coverage(path, "g > 0.2 && g > 1.5*r && g > 1.5*b"),
                "cyan": coverage(path, "g > 0.2 && b > 0.2 && g > 1.5*r && b > 1.5*r"),
                "red": coverage(path, "r > 0.2 && r > 1.5*g && r > 1.5*b"),
            }
            valid = colors["cyan"] > .1 and (
                colors["red"] > .1 and colors["green"] < .01 if sky_red
                else colors["green"] > .1 and colors["red"] < .0001
            )
            rows.append({"phase": name, "pixels": colors, "pass": valid})
            print(json.dumps(rows[-1]), flush=True)
            return valid

        wait_cache(lambda cache: cache["sampled_sun_local"][1] > .2)
        if scene.stem == "sun_material_precision_dem":
            deadline = time.monotonic() + 30
            while time.monotonic() < deadline:
                status = execute(session, "TerrainLodStatus")
                stream, derived = status["stream"], status["derived"]
                if (stream["wanted"] > 0 and stream["resident"] == stream["wanted"]
                    and stream["pending"] == 0 and stream["focus_resident"] == stream["focus_wanted"]
                    and derived["total"] > 0 and derived["ready"] == derived["total"]
                    and derived["pending"] == 0 and not derived["active"]):
                    break
                time.sleep(.05)
            else:
                raise RuntimeError(f"DEM visual cover did not settle: {status}")
        if not capture("initial"):
            raise RuntimeError("initial native-light/terrain/sky GPU control failed")
        doc = execute(session, "QueryUsdPrim", path="/World/Sun")["doc_id"]
        lighting_test = (ROOT / "assets/scenarios/tests/sun_material_precision.rhai").read_text()
        checked = execute(session, "RunRhai", code=lighting_test + f"\ncheck_lighting_rejection({doc});")
        assert "LIGHT_REJECTION_PASS" in checked["stdout"], checked

        def rhai(expression):
            return execute(session, "RunRhai", code=lighting_test + "\n" + expression)["stdout"].strip()

        def wait_light(expected):
            deadline = time.monotonic() + 15
            while time.monotonic() < deadline:
                if rhai(f"light_matches({json.dumps(expected)})") == "true":
                    return
                time.sleep(.05)
            raise RuntimeError(f"expected Sun {expected}, observed {rhai('light_sample()')}")

        def ray(yaw, pitch):
            return [-math.sin(yaw)*math.cos(pitch), math.sin(pitch), -math.cos(yaw)*math.cos(pitch)]

        execute(session, "ApplyUsdTransientOps", doc_id=doc, label="Mounted light child", ops=[{
            "AddPrim": {"edit_target": "@view@", "parent_path": "/World/Sun", "name": "AttachedScope",
                        "type_name": "Scope", "reference": None}
        }])
        deadline = time.monotonic() + 15
        while time.monotonic() < deadline:
            child = int(rhai('find_path("/World/Sun/AttachedScope")'))
            if child >= 0:
                break
            time.sleep(.05)
        else:
            raise RuntimeError("attached light child was not projected")
        initial = json.loads(rhai("light_sample()"))
        root = int(rhai('find_path("/World")'))
        root_rotation = json.loads(rhai(f"world_rotation({root})"))
        # A live parent orientation differs from USD stage-world axes.
        # The authored Sun command still speaks in the active physics frame.
        phase = .25
        execute(session, "RotateEntity", entity_id=root, rotation=[0., math.sin(phase), 0., math.cos(phase)])
        rotated_ray = [initial[1]*math.cos(2*phase) + initial[3]*math.sin(2*phase),
                       initial[2], -initial[1]*math.sin(2*phase) + initial[3]*math.cos(2*phase)]
        original = [initial[0], *rotated_ray]
        wait_light(original)
        generation = int(rhai(f"usd_document_generation({doc})"))
        execute(session, "SetEnvironmentLight", sun_pitch=-.12, sun_yaw=.18, illuminance=14000.,
                sun_color=[.25,.5,.75], shadow_maps_enabled=False)
        live = ray(.18, -.12)
        wait_light([14000., *live])
        if int(rhai(f"usd_document_generation({doc})")) != generation:
            raise RuntimeError("Live light adjustment changed the USD document")
        execute(session, "SetEnvironmentLight", intent="Persistent", illuminance=32000.)
        wait_light([32000., *live])
        execute(session, "SetEnvironmentLight", intent="Persistent", sun_pitch=-.27, sun_yaw=.42)
        wait_light([32000., *ray(.42, -.27)])
        execute(session, "UndoDocument", doc_id=doc)
        wait_light([32000., *original[1:]])
        execute(session, "UndoDocument", doc_id=doc)
        wait_light(original)
        if rhai(f"live_channels_retained({child})") != "true":
            raise RuntimeError("Authored Sun edits replaced a child or overwrote unrelated live channels")
        execute(session, "RotateEntity", entity_id=root, rotation=root_rotation)
        wait_light(initial)
        print("PASS static Sun live, authored and undo channels", flush=True)
        execute(session, "SetEnvironmentLight", sun_pitch=.00001, sun_yaw=.5)
        if not capture("night"):
            raise RuntimeError("night GPU control failed")
        execute(session, "SetEnvironmentLight", sun_pitch=-.00001)
        wait_cache(lambda cache: abs(cache["sampled_sun_local"][1]) < .0001 and cache["uploads"])
        capture("tiny-sunrise")
        for name, angle, sky_red in [("positive-angle", "0.000001", True), ("zero-angle", "0", False)]:
            execute(session, "ApplyUsdTransientOps", doc_id=doc, label="GPU angular-size boundary", ops=[{
                "SetAttribute": {"edit_target": "@view@", "path": "/World/Sun",
                                 "name": "inputs:angle", "type_name": "float", "value": angle}
            }])
            capture(name, sky_red)
        diagnostics = execute(session, "RuntimeDiagnostics")
    if port_is_open(args.port):
        raise RuntimeError("owned API port was not released")
    assert_render_log_clean(log)
    if diagnostics["errors"]:
        raise RuntimeError(f"production diagnostics contain errors: {diagnostics}")
    (output / "validation.json").write_text(json.dumps({
        "phases": rows, "diagnostics": diagnostics, "pid": pid, "port_released": True
    }, indent=2))
    failed = any(not row["pass"] for row in rows)
    if failed:
        raise RuntimeError("unexpected GPU verdict; inspect validation.json")
    print("PASS five GPU material phases; owned Exit", flush=True)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
