#!/usr/bin/env python3
"""Check a mobility root's solver output ownership and invalid inertia boundary."""
import argparse
import json
import os
from pathlib import Path

from runtime import ProductionSession, run_authored_scenario


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port", type=int, default=4841)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    os.environ.update(LUNCOSIM_EPHEMERAL_SETTINGS="1", LUNCOSIM_ISOLATED_RUN="1",
                      LUNCOSIM_CONFIG=str(output / "config"))
    with ProductionSession(args.port, windowed=True,
            extra_args=["--scene", "scenes/tests/twin_session_retirement/replacement/replacement.usda"],
            log_path=output / "app.log") as session:
        source = "lunco://scenarios/tests/vehicle_program_outputs.rhai"
        try:
            run_authored_scenario(session, source, title="VEHICLE PROGRAM OUTPUTS", check_count=6,
                                  allow_runtime_fault=True)
            run_authored_scenario(session, source, title="VEHICLE INVALID INERTIA", check_count=5,
                                  params={"negative": True}, allow_runtime_fault=True)
        finally:
            for command in ["BindingStatus", "CosimStatus", "GetReadiness"]:
                result = session.post({"type": "ExecuteCommand", "command": command, "params": {}})
                (output / f"{command}.json").write_text(json.dumps(result, indent=2) + "\n")
            session.capture_screenshot(output / "result.png")
    (output / "report.json").write_text(json.dumps(
        {"checks": 11, "port": args.port, "port_released": True}, indent=2) + "\n")
    print("11 authored output/inertia checks passed; owned session and API port released")


if __name__ == "__main__":
    main()
