#!/usr/bin/env python3
"""Exercise finite input bindings through the production headless solve owner."""
import argparse
import json
import os
from pathlib import Path

from runtime import ROOT, ProductionSession, port_is_open, run_authored_scenario


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port", type=int, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    os.environ.update(LUNCOSIM_EPHEMERAL_SETTINGS="1", LUNCOSIM_CONFIG=str(output / "config"))
    with ProductionSession(args.port,
            extra_args=("--scene", str(ROOT / "assets/scenes/tests/modelica_runtime_contracts.usda")),
            log_path=output / "runtime.log") as session:
        session.post({"type": "ExecuteCommand", "command": "SetTimeTransport", "params": {"playing": True}})
        run_authored_scenario(session, "lunco://scenarios/tests/modelica_solve_inputs.rhai",
                              title="MODELICA SOLVE INPUTS", check_count=6)
    assert not port_is_open(args.port)
    (output / "qualification.json").write_text(json.dumps(dict(checks=6, port_released=True), indent=2))
    print("PASS: 6 solve input checks; owned headless session closed")


if __name__ == "__main__":
    main()
