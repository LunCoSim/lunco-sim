#!/usr/bin/env python3
"""Exercise catalog continuation and restart through the shipped tutorial tools."""
from __future__ import annotations

import argparse
import json
import time
from pathlib import Path

from runtime import BINARY, ROOT, ProductionSession, assert_render_log_clean, wait_for_authored_verdict


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port", type=int, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    with ProductionSession(args.port, windowed=True, extra_args=("--no-throttle",),
                           log_path=output / "runtime.log") as session:
        pid = session.process.pid
        if Path(f"/proc/{pid}/cwd").resolve() != ROOT or Path(f"/proc/{pid}/exe").resolve() != BINARY.resolve():
            raise RuntimeError("session does not belong to this checkout")
        print(f"OWNED pid={pid} port={args.port}", flush=True)
        response = session.post({"type": "ExecuteCommand", "command": "OpenTwin", "params": {
            "path": str(ROOT / "assets/scenes/tests/workbench_perspectives"),
        }})
        if response.get("error"):
            raise RuntimeError(response)
        # Admit the authored test only once the requested Twin owns the workspace.
        deadline = time.monotonic() + 30
        while time.monotonic() < deadline:
            contract = session.post({"type": "ExecuteCommand", "command": "ReadActiveTwinContract", "params": {}})
            if contract.get("data", {}).get("name") == "workbench_perspectives":
                break
            time.sleep(0.1)
        else:
            raise RuntimeError("navigation test Twin was not admitted")
        offset = session.log_path.stat().st_size
        # Two shipped presentation guides under an authored test-only catalog.
        # Neither entry requests a scene: restart must work without one.
        catalog = {"kind": "lunco.tutorial-catalog.v1", "tutorials": [
            {"track": "Navigation test", "title": "Perspectives", "blurb": "Review perspectives",
             "difficulty": "beginner", "source_asset": "tutorials/perspectives/overview.rhai",
             "next_source_asset": "tutorials/luncosim/welcome.rhai"},
            {"track": "Navigation test", "title": "Read the UI", "blurb": "Review the UI",
             "difficulty": "beginner", "source_asset": "tutorials/luncosim/welcome.rhai"},
        ]}
        response = session.post({"type": "ExecuteCommand", "command": "RunScenarioAsset", "params": {
            "source_asset": "lunco://scenarios/tests/tutorial_navigation.rhai", "reload_policy": "Restart",
            "params": {"catalog": json.dumps(catalog)},
        }})
        if response.get("error"):
            raise RuntimeError(response)
        wait_for_authored_verdict(session, title="Tutorial navigation", check_count=11, offset=offset, timeout_s=60)
        session.capture_screenshot(output / "navigation.png")
        assert_render_log_clean(session.log_path)
    print("Tutorial navigation: PASS; owned port released", flush=True)


if __name__ == "__main__":
    main()
