#!/usr/bin/env python3
"""Qualify atomic co-simulation exchange and native fail-stop behavior."""
from __future__ import annotations

import argparse
import os
from pathlib import Path

from runtime import BINARY, ROOT, ProductionSession, wait_for_authored_verdict


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port", type=int, default=4396)
    parser.add_argument("--windowed", action="store_true")
    args = parser.parse_args()
    os.environ["LUNCOSIM_EPHEMERAL_SETTINGS"] = "1"
    os.environ["LUNCOSIM_ISOLATED_RUN"] = "1"
    log = ROOT / "target" / ("connection-transaction-ui.log" if args.windowed else "connection-transaction.log")
    with ProductionSession(args.port, windowed=args.windowed, log_path=log) as session:
        pid = session.process.pid
        process = Path("/proc") / str(pid)
        if process.exists() and ((process / "cwd").resolve() != ROOT
                                or (process / "exe").resolve() != BINARY.resolve()):
            raise RuntimeError("owned process does not match this checkout/binary")
        print(f"OWNED PID={pid} port={args.port} binary={BINARY}", flush=True)
        result = session.post({"type": "ExecuteCommand", "command": "OpenFile",
                               "params": {"path": str(ROOT / "assets/scenes/tests/connection_transaction.usda")}})
        if result.get("error"):
            raise RuntimeError(f"fixture admission failed: {result}")
        wait_for_authored_verdict(session, title="CONNECTION TRANSACTION", check_count=10,
                                 allow_runtime_fault=True, timeout_s=60)
    print(f"API Exit completed; port {args.port} released", flush=True)


if __name__ == "__main__":
    main()
