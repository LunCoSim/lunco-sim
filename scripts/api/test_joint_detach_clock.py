#!/usr/bin/env python3
"""Qualify joint retirement across batched physics ticks in the production host."""
from __future__ import annotations

import argparse
import os
from pathlib import Path

from runtime import BINARY, ROOT, ProductionSession, wait_for_authored_verdict, wait_for_ready


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port", type=int, default=4396)
    parser.add_argument("--windowed", action="store_true")
    parser.add_argument("--case", choices=("clock", "paused"))
    args = parser.parse_args()
    os.environ.update(LUNCOSIM_EPHEMERAL_SETTINGS="1", LUNCOSIM_ISOLATED_RUN="1")
    host = "ui" if args.windowed else "headless"
    for mode in ((args.case,) if args.case else ("clock", "paused")):
        run(args.port, args.windowed, mode, ROOT / "target" / f"joint-detach-{mode}-{host}.log")


def run(port, windowed, mode, log):
    with ProductionSession(port, windowed=windowed, log_path=log) as session:
        pid = session.process.pid
        process = Path("/proc") / str(pid)
        if process.exists() and ((process / "cwd").resolve() != ROOT
                                 or (process / "exe").resolve() != BINARY.resolve()):
            raise RuntimeError("owned process does not match this checkout/binary")
        print(f"OWNED PID={pid} port={port} binary={BINARY}", flush=True)

        def execute(command, **params):
            result = session.post({"type": "ExecuteCommand", "command": command, "params": params})
            if result.get("error"):
                raise RuntimeError(result)
            return result.get("data", {})

        execute("LoadScene", path="lunco://scenes/tests/joint.usda")
        wait_for_ready(port)
        execute("SetTimeTransport", playing=False)
        execute("StepSimulation", hold=True)
        offset = log.stat().st_size
        execute("RunScenarioAsset", source_asset=f"lunco://scenarios/tests/joint_detach_{mode}.rhai",
                reload_policy="Retain")
        wait_for_ready(port)
        if mode == "clock":
            execute("StepSimulation", steps=3)
        title = "JOINT DETACH CLOCK" if mode == "clock" else "PAUSED JOINT DETACH"
        wait_for_authored_verdict(session, title=title, check_count=4,
                                  offset=offset, timeout_s=30)
    print(f"API Exit completed; port {port} released", flush=True)


if __name__ == "__main__":
    main()
