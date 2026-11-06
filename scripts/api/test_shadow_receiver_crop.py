#!/usr/bin/env python3
"""Verify receiver raster admission, source retirement and unchanged GPU output."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import time

from runtime import (BINARY, ROOT, ProductionSession, assert_render_log_clean,
                     port_is_open, run_authored_scenario, wait_for_authored_verdict)
from run_editor_scene_test import wait_for_scene, wait_for_scene_time_selection
from test_sun_material_precision import coverage, execute


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port", type=int, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    os.environ.update(LUNCOSIM_CONFIG=str(output / "config"),
                      LUNCOSIM_EPHEMERAL_SETTINGS="1", LUNCOSIM_ISOLATED_RUN="1")
    scene = ROOT / "assets/scenes/tests/vertex_shadow_alignment.usda"
    shader = ROOT / "assets/shaders/tests/vertex_shadow_receiver.wesl"
    source = shader.read_text()
    originals = {str(p): hashlib.sha256(p.read_bytes()).hexdigest() for p in [scene, shader]}
    log = output / "runtime.log"
    with ProductionSession(args.port, windowed=True, log_path=log, extra_args=[
        "--scene", str(scene), "--render-quality", "high", "--offscreen",
        "--record-size", "1280x720", "--no-throttle",
    ]) as session:
        pid = session.process.pid
        assert Path(f"/proc/{pid}/cwd").resolve() == ROOT
        assert Path(f"/proc/{pid}/exe").resolve() == BINARY.resolve()
        wait_for_scene(session, str(scene), 45)
        wait_for_scene_time_selection(session, log, 30)
        execute(session, "SetTimeTransport", playing=True)
        wait_for_authored_verdict(session, title="VERTEX PREPASS VARIANTS", check_count=3)
        execute(session, "SetTimeTransport", playing=False)
        time.sleep(3)
        params = dict(camera_path="/VertexShadowAlignment/Camera", bounded=True,
                      reason="receiver-bounded")
        run_authored_scenario(session, "lunco://scenarios/tests/shadow_receiver_crop.rhai",
                              title="SHADOW RECEIVER RASTER", check_count=4, params=params)
        session.capture_screenshot(output / "bounded.png")
        control_source = (ROOT / "assets/scenarios/tests/shadow_receiver_crop.rhai").read_text()
        def control(full):
            result = execute(session, "RunRhai", code=control_source +
                             "\nset_shadow_test_control(" + json.dumps(params["camera_path"]) +
                             "," + str(full).lower() + ");")
            assert result["stdout"].strip() == "true", result
            time.sleep(.5)
        control(True)
        params.update(bounded=False, reason="temporal-jitter")
        run_authored_scenario(session, "lunco://scenarios/tests/shadow_receiver_crop.rhai",
                              title="SHADOW RECEIVER RASTER", check_count=4, params=params)
        control(False)
        params.update(bounded=True, reason="receiver-bounded")
        run_authored_scenario(session, "lunco://scenarios/tests/shadow_receiver_crop.rhai",
                              title="SHADOW RECEIVER RASTER", check_count=4, params=params)
        # Source text is a transient Shader asset override; no file is authored.
        try:
            execute(session, "SetShaderSource", path="lunco://shaders/tests/vertex_shadow_receiver.wesl",
                    source=source.replace("//!@capability lunco.shadow-receiver.surface.v1\n", ""))
            time.sleep(3)
            params.update(bounded=False, reason="unbounded-material")
            run_authored_scenario(session, "lunco://scenarios/tests/shadow_receiver_crop.rhai",
                                  title="SHADOW RECEIVER RASTER", check_count=4, params=params)
            session.capture_screenshot(output / "unbounded.png")
        finally:
            execute(session, "SetShaderSource", path="lunco://shaders/tests/vertex_shadow_receiver.wesl", source=source)
        time.sleep(3)
        # An old unbounded pipeline can remain compiled. Re-adding an annotation
        # cannot grant a proof for that asset's existing lifetime.
        run_authored_scenario(session, "lunco://scenarios/tests/shadow_receiver_crop.rhai",
                              title="SHADOW RECEIVER RASTER", check_count=4, params=params)
        pixels = {name: coverage(output / "bounded.png", expression) for name, expression in {
            "green": "g > 0.8 && r < 0.2 && b < 0.2",
            "cyan": "g > 0.8 && b > 0.8 && r < 0.2",
            "red": "r > 0.8 && g < 0.2",
        }.items()}
        equality = subprocess.run(["compare", "-metric", "AE", str(output / "bounded.png"),
                                   str(output / "unbounded.png"), "null:"], capture_output=True, text=True)
        diagnostics = execute(session, "RuntimeDiagnostics")
        # Source admission is application scoped; projected rectangles belong
        # to the scene and must retire when its camera is removed.
        execute(session, "ClearScene")
        deadline = time.monotonic() + 15
        while time.monotonic() < deadline:
            retired = execute(session, "RunRhai", code='find_path("/VertexShadowAlignment/Camera") < 0')
            if retired["stdout"].strip() == "true":
                break
            time.sleep(.1)
        retired = execute(session, "RunRhai", code='report_verdict([t_true(find_path("/VertexShadowAlignment/Camera") < 0, "camera must retire with its scene")], "SHADOW CAMERA RETIREMENT", "SHADOW_CAMERA_RETIREMENT");')
        assert "SHADOW CAMERA RETIREMENT: PASS" in retired["stdout"], retired
    assert not port_is_open(args.port)
    assert_render_log_clean(log)
    assert originals == {path: hashlib.sha256(Path(path).read_bytes()).hexdigest() for path in originals}
    result = dict(pid=pid, binary=str(BINARY), pixels=pixels, pixel_difference=equality.stderr.strip(),
                  diagnostics=diagnostics, port_released=True, source_unchanged=True)
    (output / "validation.json").write_text(json.dumps(result, indent=2) + "\n")
    assert equality.returncode == 0 and equality.stderr.strip() == "0", result
    assert pixels["green"] > .0001 and pixels["cyan"] > .0001 and pixels["red"] < .00001, pixels
    assert diagnostics["errors"] == 0, diagnostics
    print("PASS receiver raster, unbounded source, pipeline lifetime, camera retirement and identical pixels")


if __name__ == "__main__":
    main()
