#!/usr/bin/env python3
"""Run authored graph-HUD assertions and capture its native viewport rendering."""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import subprocess
import time

from runtime import BINARY, ROOT, ProductionSession, assert_render_log_clean, wait_for_ready
from run_editor_scene_test import wait_for_scene_time_selection


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port", type=int, required=True)
    parser.add_argument("--scene", default="scenes/luncosim/solar_rover_demo.usda")
    parser.add_argument("--signal", default="power.battery_soc")
    parser.add_argument("--secondary-signal", default="power.battery_net")
    parser.add_argument("--output", type=Path, default=ROOT / "target/graph-hud-evidence")
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    os.environ.setdefault("LUNCOSIM_CONFIG", str(output / "config"))
    os.environ.setdefault("LUNCOSIM_EPHEMERAL_SETTINGS", "1")
    source = (ROOT / "assets/scripting/tests/lib/test_assert.rhai").read_text()
    source += "\n" + (ROOT / "assets/scripting/tests/test_graph_hud.rhai").read_text()

    with ProductionSession(args.port, windowed=True,
            extra_args=("--no-throttle", "--scene", args.scene),
            log_path=output / "runtime.log") as session:
        process = Path(f"/proc/{session.process.pid}")
        if process.is_dir() and ((process / "cwd").resolve() != ROOT.resolve()
                or (process / "exe").resolve() != BINARY.resolve()):
            raise RuntimeError("owned runtime does not match the checkout and binary")
        print(f"Owned runtime PID {session.process.pid}, port {args.port}, binary {BINARY}", flush=True)

        def command(command_name: str, **params):
            result = session.post({"type": "ExecuteCommand", "command": command_name, "params": params})
            if result.get("error"):
                raise RuntimeError(result)
            return result.get("data", {})

        def verdict(call: str):
            result = subprocess.run([str(BINARY), "rhai", "--api", str(args.port), "--stdout",
                "-e", source + "\n" + call], cwd=ROOT, capture_output=True, text=True, timeout=30)
            print(result.stdout.strip(), flush=True)
            if result.returncode or "TESTS_FAIL" in result.stdout or "TESTS_OK" not in result.stdout:
                raise RuntimeError(f"authored verdict failed: {result.stdout}\n{result.stderr}")

        wait_for_scene_time_selection(session, output / "runtime.log", 120)
        wait_for_ready(args.port)
        command("ActivatePerspective", id="sandbox_view")
        verdict("graph_hud_empty_prepare();")
        session.capture_screenshot(output / "empty.png")
        verdict("graph_hud_empty_rendered();")
        command("RunRhai", code='let result = play(); if !result.ok { throw result.error; }')
        deadline = time.monotonic() + 30
        while time.monotonic() < deadline:
            channels = command("ListTelemetryChannels", name=args.signal)["channels"]
            secondary = command("ListTelemetryChannels", name=args.secondary_signal)["channels"]
            if all(any(c["active"] and c["source"] is not None and c["samples"] > 2 for c in group) for group in (channels, secondary)):
                break
            time.sleep(0.1)
        else:
            raise RuntimeError(f"missing active samples: {channels}; {secondary}")
        verdict("graph_hud_prepare(" + json.dumps(args.signal) + ", " + json.dumps(args.secondary_signal) + ");")
        deadline = time.monotonic() + 10
        while time.monotonic() < deadline:
            if all(hud["rendered"] for hud in command("InspectGraphHud")["huds"]):
                break
            time.sleep(0.1)
        verdict("graph_hud_rendered();")
        session.capture_screenshot(output / "shown.png")
        reports = command("InspectGraphHud")["huds"]
        first, second = reports
        x, y, width, height = second["rect"]
        corner_x, corner_y = x + width - 3, y + height - 3
        command("InjectWindowInput", event={"PointerMove": {"x": corner_x, "y": corner_y}})
        session.capture_screenshot(output / "resize-hover.png")
        command("InjectWindowInput", event={"PointerButton": {"button": "Primary", "state": "Pressed", "x": corner_x, "y": corner_y}})
        session.capture_screenshot(output / "resize-pressed.png")
        command("InjectWindowInput", event={"PointerMove": {"x": corner_x - 50, "y": corner_y - 25}})
        session.capture_screenshot(output / "resize-drag.png")
        command("InjectWindowInput", event={"PointerButton": {"button": "Primary", "state": "Released", "x": corner_x - 50, "y": corner_y - 25}})
        session.capture_screenshot(output / "resized.png")
        verdict("graph_hud_resized(" + ", ".join(json.dumps(value) for value in
            [first["width"], first["height"], second["width"], second["height"]]) + ");")
        resized = command("InspectGraphHud")["huds"][1]
        x, y, width, _ = resized["rect"]
        command("RunRhai", code=f'input_click("primary", {x + width - 18}, {y + 18});')
        session.capture_screenshot(output / "closed.png")
        verdict("graph_hud_removed_secondary(" + json.dumps(resized["grid"]) + ");")
        command("SetGraphHud", visible=False)
        time.sleep(0.2)
        session.capture_screenshot(output / "hidden.png")
        command("SetGraphHud", visible=True)
        retired_grid = command("InspectGraphHud")["huds"][0]["grid"]
        command("ClearScene")
        wait_for_ready(args.port)
        verdict("graph_hud_retired(" + json.dumps(retired_grid) + ");")
        session.capture_screenshot(output / "retired.png")
    assert_render_log_clean(output / "runtime.log")
    print(f"Evidence: {output}; owned session exited and port released", flush=True)


if __name__ == "__main__":
    main()
