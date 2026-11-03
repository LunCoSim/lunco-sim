#!/usr/bin/env python3
"""Verify fixed-step assertion failure and its visible Recent Events diagnostic."""
import argparse
import json
import os
from pathlib import Path

from runtime import BINARY, ROOT, ProductionSession, run_authored_scenario


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port", type=int, default=4839)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    os.environ.update(LUNCOSIM_EPHEMERAL_SETTINGS="1", LUNCOSIM_ISOLATED_RUN="1",
                      LUNCOSIM_CONFIG=str(output / "config"))
    with ProductionSession(args.port, windowed=True,
            extra_args=["--scene", "scenes/tests/twin_session_retirement/replacement/replacement.usda"],
            log_path=output / "app.log") as session:
        pid = session.process.pid
        process = Path("/proc") / str(pid)
        if process.exists():
            assert (process / "cwd").resolve() == ROOT
            assert (process / "exe").resolve() == BINARY.resolve()
        run_authored_scenario(session, "lunco://scenarios/tests/fixed_step_assertion_failure.rhai",
                              title="FIXED STEP ASSERTION FAILURE", check_count=6,
                              allow_runtime_fault=True)
        session.capture_screenshot(output / "failure.png")
    (output / "report.json").write_text(json.dumps(
        {"checks": 6, "port": args.port, "port_released": True}, indent=2) + "\n")
    print("6 authored failure checks passed; owned session and API port released")


if __name__ == "__main__":
    main()
