#!/usr/bin/env python3
"""Require initialization and receipt-retirement verdicts from the production app."""

import argparse
import hashlib
import json
import os
from pathlib import Path

from runtime import (BINARY, ROOT, ProductionSession, run_authored_scenario,
                     wait_for_authored_verdict)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port", type=int, default=4757)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    os.environ.update(LUNCOSIM_EPHEMERAL_SETTINGS="1", LUNCOSIM_ISOLATED_RUN="1",
                      LUNCOSIM_CONFIG=str(output / "config"))
    receipt = {"binary_sha256": hashlib.sha256(BINARY.read_bytes()).hexdigest(), "port": args.port}
    with ProductionSession(args.port, extra_args=["--scene", "scenes/tests/modelica_initialization_network.usda"],
                           log_path=output / "app.log") as session:
        pid = session.process.pid
        assert Path(f"/proc/{pid}/cwd").resolve() == ROOT
        assert Path(f"/proc/{pid}/exe").resolve() == BINARY.resolve()
        receipt["pid"] = pid
        wait_for_authored_verdict(session, title="MODELICA INITIALIZATION NETWORK", check_count=44)
        run_authored_scenario(
            session, "lunco://scenarios/tests/modelica_initialization_receipt.rhai",
            title="MODELICA INITIALIZATION RECEIPT", check_count=2,
            params={"replacement_scene": "lunco://scenes/tests/twin_session_retirement/replacement/replacement.usda",
                    "replacement_marker": "/World/TestLight"},
        )
    receipt.update(checks=46, port_released=True)
    (output / "receipt.json").write_text(json.dumps(receipt, indent=2) + "\n")
    print(f"46 authored checks passed; owned port {args.port} released")


if __name__ == "__main__":
    main()
