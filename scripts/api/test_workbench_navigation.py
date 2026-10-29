#!/usr/bin/env python3
"""Run authored navigation acceptance in an owned production GUI session."""
from __future__ import annotations

import argparse
import os
import time
from pathlib import Path

from runtime import BINARY, ROOT, ProductionSession, assert_render_log_clean, wait_for_authored_verdict


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port", type=int, required=True)
    args = parser.parse_args()
    output = ROOT / "target/workbench-navigation"
    log = output / "runtime.log"
    os.environ.setdefault("LUNCOSIM_CONFIG", str(output / "config"))
    os.environ.setdefault("LUNCOSIM_EPHEMERAL_SETTINGS", "1")
    with ProductionSession(args.port, windowed=True, log_path=log,
                           extra_args=("--no-throttle", "--scene", "scenes/fixtures/usd_query_api/site.usda")) as session:
        pid = session.process.pid
        if Path(f"/proc/{pid}/cwd").resolve() != ROOT or Path(f"/proc/{pid}/exe").resolve() != BINARY.resolve():
            raise RuntimeError("Runtime process does not belong to this checkout")
        print(f"OWNED pid={pid} port={args.port} cwd={ROOT}", flush=True)
        offset = log.stat().st_size
        result = session.post({"type": "ExecuteCommand", "command": "RunScenarioAsset", "params": {
            "source_asset": "lunco://scenarios/tests/workbench_navigation.rhai", "reload_policy": "Retain",
            "params": {"replacement": str(ROOT / "assets/scenes/tests/workbench_perspectives")},
        }})
        if result.get("error"):
            raise RuntimeError(result)
        captured = set()
        deadline = time.monotonic() + 120
        while time.monotonic() < deadline:
            text = log.read_text(errors="replace")[offset:]
            for subject in ("preview", "library-view", "library-search", "quick-open", "actions", "scene-view"):
                if f"NAV_CAPTURE:{subject}" in text and subject not in captured:
                    time.sleep(0.2)
                    session.capture_screenshot(output / f"{subject}.png")
                    captured.add(subject)
            if "Workbench navigation: PASS" in text or "TESTS_FAIL" in text:
                break
            time.sleep(0.1)
        wait_for_authored_verdict(session, title="Workbench navigation", check_count=25, offset=offset, timeout_s=5)
        session.capture_screenshot(output / "replacement-twin.png")
        assert_render_log_clean(log)
    print("Owned process exited and API port released", flush=True)


if __name__ == "__main__":
    main()
