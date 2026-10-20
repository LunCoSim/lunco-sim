#!/usr/bin/env python3
"""Require transport/lifecycle verdicts from a retained production observer."""
import argparse
import hashlib
import json
import os
from pathlib import Path
from runtime import BINARY, ROOT, ProductionSession, run_authored_scenario


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port", type=int, default=4757)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--windowed", action="store_true")
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    os.environ.update(LUNCOSIM_EPHEMERAL_SETTINGS="1", LUNCOSIM_ISOLATED_RUN="1",
                      LUNCOSIM_CONFIG=str(output / "config"))
    receipt = {"binary_sha256": hashlib.sha256(BINARY.read_bytes()).hexdigest(),
               "port": args.port, "windowed": args.windowed}
    with ProductionSession(args.port, windowed=args.windowed,
                           extra_args=["--no-vsync", "--no-throttle", "--scene",
                                       "scenes/tests/twin_session_retirement/replacement/replacement.usda"],
                           log_path=output / "app.log") as session:
        pid = session.process.pid
        assert Path(f"/proc/{pid}/cwd").resolve() == ROOT
        assert Path(f"/proc/{pid}/exe").resolve() == BINARY.resolve()
        receipt["pid"] = pid
        run_authored_scenario(session, "lunco://scenarios/tests/simulation_lifecycle.rhai",
                              title="SIMULATION LIFECYCLE", check_count=11,
                              params={"scene": "lunco://scenes/tests/twin_session_retirement/replacement/replacement.usda"})
        if args.windowed:
            session.capture_screenshot(output / "stopped.png")
    receipt.update(checks=11, port_released=True)
    (output / "report.json").write_text(json.dumps(receipt, indent=2) + "\n")
    print(f"11 authored checks passed; owned port {args.port} released")


if __name__ == "__main__":
    main()
