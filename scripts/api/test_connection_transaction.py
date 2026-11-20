#!/usr/bin/env python3
"""Qualify atomic co-simulation exchange and native fail-stop behavior."""
from __future__ import annotations

import argparse
import os
from pathlib import Path

from runtime import BINARY, ROOT, ProductionSession, wait_for_authored_verdict


def execute(session, command, params):
    result = session.post({"type": "ExecuteCommand", "command": command, "params": params})
    if result.get("error"):
        raise RuntimeError(f"{command} failed: {result}")
    return result.get("data")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port", type=int, default=4396)
    parser.add_argument("--windowed", action="store_true")
    args = parser.parse_args()
    os.environ["LUNCOSIM_EPHEMERAL_SETTINGS"] = "1"
    os.environ["LUNCOSIM_ISOLATED_RUN"] = "1"
    log = ROOT / "target" / ("connection-transaction-ui.log" if args.windowed else "connection-transaction.log")
    scene = "lunco://scenes/tests/connection_transaction.usda"
    with ProductionSession(args.port, windowed=args.windowed, extra_args=("--no-load-default-scene",),
                           log_path=log) as session:
        pid = session.process.pid
        process = Path("/proc") / str(pid)
        if process.exists() and ((process / "cwd").resolve() != ROOT
                                or (process / "exe").resolve() != BINARY.resolve()):
            raise RuntimeError("owned process does not match this checkout/binary")
        print(f"OWNED PID={pid} port={args.port} binary={BINARY}", flush=True)
        offset = log.stat().st_size
        execute(session, "LoadScene", {"path": scene})
        wait_for_authored_verdict(session, title="CONNECTION TRANSACTION", check_count=18,
                                  offset=offset, allow_runtime_fault=True, timeout_s=60)
        # Make the native recorder due on the rejected fourth interval of the
        # replacement scene. Settings are ephemeral in this process.
        execute(session, "ControlTelemetry", {"rate_hz": 60.0})
        offset = log.stat().st_size
        execute(session, "RestartScene", {})
        wait_for_authored_verdict(session, title="CONNECTION TRANSACTION", check_count=18,
                                  offset=offset, allow_runtime_fault=True, timeout_s=60)
        probe, = execute(session, "ListTelemetryChannels", {"name": "exchange_mass_probe"})["channels"]
        native_key = f"api/{probe['source']}:position.x"
        stamps = [s["t"] for s in execute(session, "QueryTelemetryHistory", {"key": native_key})["samples"]]
        if stamps != [i / 60.0 for i in (1, 2, 3)]:
            raise RuntimeError(f"unexpected native position sample times after rejection: {stamps}")
    print(f"API Exit completed; port {args.port} released", flush=True)


if __name__ == "__main__":
    main()
