#!/usr/bin/env python3
"""Verify production/diagnostic terrain switches with all native prepasses."""
from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
import time
from pathlib import Path

from runtime import BINARY, ROOT, ProductionSession, assert_render_log_clean, port_is_open
from run_editor_scene_test import wait_for_scene, wait_for_scene_time_selection


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port", type=int, default=4286)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--quality", choices=["low", "balanced", "high"], default="high")
    parser.add_argument("--windowed", action="store_true")
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    os.environ.update(LUNCOSIM_EPHEMERAL_SETTINGS="1", LUNCOSIM_ISOLATED_RUN="1",
                      LUNCOSIM_CONFIG=str(output / "config"))
    scene = ROOT / "assets/scenes/tests/terrain_shader_appearance.usda"
    log = output / "runtime.log"
    render_args = ["--scene", str(scene), "--render-quality", args.quality]
    if not args.windowed:
        render_args.extend(["--offscreen", "--record-size", "640x360"])
    rows = []
    with ProductionSession(args.port, windowed=True, extra_args=render_args, log_path=log) as session:
        pid = session.process.pid
        if sys.platform.startswith("linux") and (
            Path(f"/proc/{pid}/cwd").resolve() != ROOT
            or Path(f"/proc/{pid}/exe").resolve() != BINARY.resolve()
        ):
            raise RuntimeError("session does not match this checkout and production binary")

        def execute(command, **params):
            response = session.post({"type": "ExecuteCommand", "command": command, "params": params})
            if response.get("error"):
                raise RuntimeError(f"{command}: {response['error']}")
            return response.get("data", {})

        wait_for_scene(session, str(scene), 45)
        wait_for_scene_time_selection(session, log, 30)
        execute("SetTimeTransport", playing=True)
        deadline = time.monotonic() + 45
        while time.monotonic() < deadline:
            status = execute("TerrainLodStatus")
            stream, derived = status.get("stream") or {}, status.get("derived") or {}
            if (stream.get("wanted", 0) > 0 and stream.get("resident") == stream["wanted"]
                and stream.get("pending") == 0 and stream.get("focus_resident") == stream.get("focus_wanted")
                and derived.get("total", 0) > 0 and derived.get("ready") == derived["total"]
                and derived.get("pending") == 0 and not derived.get("active")):
                break
            time.sleep(.1)
        else:
            raise RuntimeError(f"terrain did not settle: {status}")
        if "TERRAIN PREPASS VARIANTS: PASS" not in log.read_text():
            raise RuntimeError("authored fixture did not enable depth, normal, and motion passes")

        for name, params in [
            ("production", {"enabled": False}),
            ("slope", {"enabled": True, "lod_depth": False, "opacity": 1.0}),
            ("lod", {"enabled": True, "lod_depth": True}),
            ("restored", {"enabled": False}),
        ]:
            execute("SetTerrainOverlay", **params)
            time.sleep(.5)
            path = output / f"{name}.png"
            session.capture_screenshot(path)
            assert_render_log_clean(log)
            # A fresh PNG can contain a stale frame after GPU submission fails.
            # Require visible colour from both diagnostics as well as a clean log.
            # The central region excludes window chrome and transient test toasts.
            expression = "max(r,max(g,b)) - min(r,min(g,b)) > 0.15"
            chromatic = float(subprocess.check_output([
                "convert", str(path), "-gravity", "center", "-crop", "50%x50%+0+0", "+repage",
                "-fx", expression, "-format", "%[fx:mean]", "info:"
            ], text=True))
            valid = chromatic > .1 if params["enabled"] else chromatic < .01
            rows.append({"phase": name, "chromatic": chromatic, "pass": valid})
            print(json.dumps(rows[-1]), flush=True)
        diagnostics = execute("RuntimeDiagnostics")
    assert_render_log_clean(log)
    if port_is_open(args.port):
        raise RuntimeError("owned API port was not released")
    (output / "validation.json").write_text(json.dumps({
        "phases": rows, "diagnostics": diagnostics, "pid": pid, "port_released": True
    }, indent=2))
    if any(not row["pass"] for row in rows) or diagnostics["errors"]:
        raise RuntimeError("terrain diagnostic GPU verdict failed; inspect validation.json")
    print("PASS four terrain material phases; all native prepasses; owned Exit", flush=True)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
