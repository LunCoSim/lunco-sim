#!/usr/bin/env python3
"""Drive authored UI preparation verdicts through the production REPL boundary."""
from __future__ import annotations

import argparse
import sys
import time
from pathlib import Path

from runtime import BINARY, ROOT, POLL_INTERVAL_S, ProductionSession, wait_for_authored_verdict


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port", type=int, required=True)
    parser.add_argument("--windowed", action="store_true")
    parser.add_argument("--timeout", type=float, default=40)
    parser.add_argument("--log", type=Path, required=True)
    args = parser.parse_args()
    source = "lunco://scenarios/tests/runtime_ui_preparation.rhai"
    scene = ROOT / "assets/scenes/tests/authored_runtime_contracts.usda"
    try:
        with ProductionSession(args.port, windowed=args.windowed,
                               extra_args=("--no-throttle", "--scene", str(scene)),
                               log_path=args.log) as session:
            pid = session.process.pid
            if sys.platform.startswith("linux") and (
                Path(f"/proc/{pid}/cwd").resolve() != ROOT
                or Path(f"/proc/{pid}/exe").resolve() != BINARY.resolve()
            ):
                raise RuntimeError("session does not match this checkout and binary")
            print(f"OWNED pid={pid} port={args.port} cwd={ROOT}", flush=True)
            result = session.post({"type": "ExecuteCommand", "command": "RunScenarioAsset",
                                   "params": {"source_asset": source}})
            if result.get("error"):
                raise RuntimeError(result)
            installed = set()
            deadline = time.monotonic() + args.timeout
            while time.monotonic() < deadline:
                output = args.log.read_text(errors="replace")
                if "TESTS_FAIL" in output:
                    raise RuntimeError("authored UI preparation verdict failed")
                if "RUNTIME_UI_PREPARATION" in output and "TESTS_OK 9" in output:
                    break
                for marker in ("first", "invalid", "recovered"):
                    if f"UI_POLICY_TEST_STIMULUS:{marker}" in output and marker not in installed:
                        code = f'import "{source}" as test; test::install_policy("{marker}");'
                        result = session.post({"type": "ExecuteCommand", "command": "RunRhai",
                                               "params": {"code": code}})
                        if result.get("error"):
                            raise RuntimeError(result)
                        installed.add(marker)
                time.sleep(POLL_INTERVAL_S)
            else:
                raise RuntimeError("no authored UI preparation verdict before timeout")
            # The held-clock UI observer releases the fixture's first tick.
            # Require that scene verdict too before Exit can stop its observer.
            wait_for_authored_verdict(session, title="AUTHORED RUNTIME CONTRACTS",
                                     check_count=16, timeout_s=args.timeout)
            response = session.post({"type": "ExecuteCommand", "command": "Exit",
                                     "params": {"force": True}})
            if response.get("error"):
                raise RuntimeError(response)
            session.process.wait(timeout=20)
    except Exception as error:
        print(f"FAIL — {error}; log={args.log}", file=sys.stderr)
        return 1
    print("PASS — nine UI and 16 scene checks; API Exit and port release verified")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
