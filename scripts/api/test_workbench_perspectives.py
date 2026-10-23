#!/usr/bin/env python3
"""Run the authored workbench perspective acceptance in an owned GUI session."""
from __future__ import annotations
import argparse
import json
import time
from pathlib import Path
from runtime import BINARY, ROOT, ProductionSession, assert_render_log_clean, get_json, wait_for_authored_verdict


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port", type=int, required=True)
    parser.add_argument("--scene-free", action="store_true")
    args = parser.parse_args()
    title = "Scene-free workbench" if args.scene_free else "Workbench perspectives"
    count = 8 if args.scene_free else 23
    scenario = "workbench_scene_free" if args.scene_free else "workbench_perspectives"
    extra = ("--no-throttle", "--no-vsync") if args.scene_free else ("--no-throttle", "--no-vsync", "--scene", "scenes/fixtures/usd_query_api/site.usda")
    output = ROOT / "target" / ("workbench-scene-free" if args.scene_free else "workbench-perspectives")
    output.mkdir(parents=True, exist_ok=True)
    log = output / "runtime.log"
    with ProductionSession(args.port, extra_args=extra, windowed=True, log_path=log) as session:
        pid = session.process.pid
        if Path(f"/proc/{pid}/cwd").resolve() != ROOT or Path(f"/proc/{pid}/exe").resolve() != BINARY.resolve():
            raise RuntimeError("Runtime process does not belong to this checkout")
        print(f"OWNED pid={pid} port={args.port} cwd={ROOT}", flush=True)
        (output / "schema.json").write_text(json.dumps(get_json(args.port, "/api/commands/schema"), indent=2) + "\n")
        if args.scene_free:
            session.post({"type": "ExecuteCommand", "command": "OpenTwin", "params": {
                "path": str(ROOT / "assets/scenes/tests/workbench_perspectives_invalid"),
            }})
        defaults = (ROOT / "assets/scripting/policy/workbench_perspectives.rhai").read_text()
        offset = log.stat().st_size
        response = session.post({"type": "ExecuteCommand", "command": "RunScenarioAsset", "params": {
            "source_asset": f"lunco://scenarios/tests/{scenario}.rhai",
            "reload_policy": "Retain",
            "params": {
                "good_twin": str(ROOT / "assets/scenes/tests/workbench_perspectives"),
                "bad_twin": str(ROOT / "assets/scenes/tests/workbench_perspectives_invalid"),
            },
        }})
        if response.get("error"):
            raise RuntimeError(response)
        installed = set()
        deadline = time.monotonic() + 90
        while time.monotonic() < deadline:
            output_text = log.read_text(errors="replace")
            if "TESTS_FAIL" in output_text or "task leaf failed" in output_text or "on_tick failed" in output_text:
                raise RuntimeError(f"Authored perspective test failed; log={log}")
            for subject in ("editor-empty", "build", "analyze", "reopened-graph", "twin-power-study"):
                key = "capture:" + subject
                if f"PERSPECTIVES_CAPTURE:{subject}" in output_text and key not in installed:
                    session.capture_screenshot(output / f"{subject}.png")
                    installed.add(key)
            for marker in ("reload", "restore", "reject", "recover"):
                if f"PERSPECTIVES_POLICY_STIMULUS:{marker}" in output_text and marker not in installed:
                    source = defaults.replace("↗ Analyze", "Analysis reloaded").replace("instances: plots,", "instances: [],") if marker == "reload" else defaults
                    if marker == "reject":
                        source = defaults.replace("slot: slot }", 'slot: slot, position: "center" }')
                    # Use the production Command/REPL scope for an application policy mutation.
                    code = 'let result = bind_policy("workbench.perspectives.defaults", "assemble", ' + json.dumps(source) + '); if !result.ok { throw result.error; }'
                    result = session.post({"type": "ExecuteCommand", "command": "RunRhai", "params": {"code": code}})
                    if result.get("error"):
                        raise RuntimeError(result)
                    installed.add(marker)
            if "SCENE_FREE_TOUR_CAPTURE" in output_text and "capture" not in installed:
                session.capture_screenshot(output / "navigation-tour.png")
                installed.add("capture")
            if f"{title}: PASS" in output_text:
                break
            if session.process.poll() is not None:
                raise RuntimeError("Owned process exited before its authored verdict")
            time.sleep(0.1)
        wait_for_authored_verdict(session, title=title, check_count=count, offset=offset, timeout_s=5)
        session.capture_screenshot(output / "twin-power-study.png")
        assert_render_log_clean(log)
    print("Owned process exited and API port released", flush=True)


if __name__ == "__main__":
    main()
