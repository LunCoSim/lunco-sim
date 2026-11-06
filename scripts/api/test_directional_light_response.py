#!/usr/bin/env python3
"""Run authored directional-response controls and inspect their real GPU output."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import time

from runtime import BINARY, ROOT, ProductionSession, assert_render_log_clean, port_is_open
from run_editor_scene_test import wait_for_scene, wait_for_scene_time_selection
from test_sun_material_precision import coverage, execute


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port", type=int, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    os.environ.update(LUNCOSIM_EPHEMERAL_SETTINGS="1", LUNCOSIM_ISOLATED_RUN="1",
                      LUNCOSIM_CONFIG=str(output / "config"))
    scene = ROOT / "assets/scenes/tests/sun_material_precision.usda"
    source_hash = hashlib.sha256(scene.read_bytes()).hexdigest()
    log = output / "runtime.log"
    with ProductionSession(args.port, windowed=True, log_path=log, extra_args=[
        "--scene", str(scene), "--render-quality", "high", "--offscreen",
        "--record-size", "640x360", "--no-throttle",
    ]) as session:
        pid = session.process.pid
        assert Path(f"/proc/{pid}/cwd").resolve() == ROOT
        assert Path(f"/proc/{pid}/exe").resolve() == BINARY.resolve()
        wait_for_scene(session, str(scene), 30)
        wait_for_scene_time_selection(session, log, 30)
        execute(session, "SetTimeTransport", playing=True)
        scope = execute(session, "ReadActiveWorkspace")
        if not scope.get("open"):
            raise RuntimeError("directional response fixture has no active workspace")
        execute(session, "RunScenarioAsset", owner_twin_id=scope["scope_id"],
                source_asset="lunco://scenarios/tests/directional_light_response.rhai")
        deadline = time.monotonic() + 30
        while time.monotonic() < deadline:
            text = log.read_text()
            if "DIRECTIONAL RESPONSE SETUP: FAIL" in text or "[rhai] TESTS_FAIL" in text:
                raise RuntimeError("authored setup failed; inspect runtime.log")
            if "DIRECTIONAL RESPONSE SETUP: PASS" in text:
                break
            time.sleep(.1)
        else:
            raise RuntimeError("authored setup produced no verdict")
        # Transient USD projection and shader compilation precede GPU readback.
        time.sleep(3)
        path = output / "controls.png"
        session.capture_screenshot(path)
        pixels = {name: coverage(path, expression) for name, expression in {
            "green": "g > 0.2 && g > 1.5*r && g > 1.5*b",
            "cyan": "g > 0.2 && b > 0.2 && g > 1.5*r && b > 1.5*r",
            "red": "r > 0.2 && r > 1.5*g && r > 1.5*b",
        }.items()}
        diagnostics = execute(session, "RuntimeDiagnostics")
    assert not port_is_open(args.port)
    assert hashlib.sha256(scene.read_bytes()).hexdigest() == source_hash
    assert_render_log_clean(log)
    result = {"pixels": pixels, "diagnostics": diagnostics, "pid": pid,
              "binary": str(BINARY), "port_released": True,
              "source_unchanged": True}
    (output / "validation.json").write_text(json.dumps(result, indent=2) + "\n")
    assert pixels["green"] > .1 and pixels["cyan"] > .1 and pixels["red"] < .0001, pixels
    assert not diagnostics["errors"], diagnostics
    print("PASS native per-light accumulation, Sun isolation and negative GPU controls; owned Exit")


if __name__ == "__main__":
    main()
