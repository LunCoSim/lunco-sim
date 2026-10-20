#!/usr/bin/env python3
"""Exercise canonical render-frame history and headless unavailability in production."""

import argparse
import hashlib
import json
import os
from pathlib import Path

from runtime import (BINARY, ROOT, ProductionSession, assert_render_log_clean,
                     get_json, run_authored_scenario)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port", type=int, default=4757)
    parser.add_argument("--window-port", type=int, default=4758)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    os.environ.update(LUNCOSIM_EPHEMERAL_SETTINGS="1", LUNCOSIM_ISOLATED_RUN="1",
                      LUNCOSIM_CONFIG=str(output / "config"))
    results = {"binary_sha256": hashlib.sha256(BINARY.read_bytes()).hexdigest(), "hosts": []}
    for windowed, port in [(False, args.port), (True, args.window_port)]:
        host = "windowed" if windowed else "headless"
        with ProductionSession(port, windowed=windowed,
                               extra_args=["--scene", "scenes/base/lunar_surface.usda",
                                           "--no-vsync", "--no-throttle"],
                               log_path=output / f"{host}.log") as session:
            pid = session.process.pid
            assert Path(f"/proc/{pid}/cwd").resolve() == ROOT
            assert Path(f"/proc/{pid}/exe").resolve() == BINARY.resolve()
            (output / f"{host}-schema.json").write_text(
                json.dumps(get_json(port, "/api/commands/schema"), indent=2) + "\n")
            run_authored_scenario(
                session, "lunco://scenarios/tests/engine_performance.rhai",
                title="ENGINE PERFORMANCE", check_count=6, params={"rendered": windowed},
            )
            response = session.post({"type": "ExecuteCommand", "command": "EnginePerformance",
                                     "params": {}})
            if response.get("error"):
                raise RuntimeError(response)
            results["hosts"].append({"host": host, "pid": pid, "port": port,
                                     "performance": response["data"]})
        assert_render_log_clean(output / f"{host}.log")
        print(f"{host}: 6 authored checks passed; owned port {port} released", flush=True)
    (output / "report.json").write_text(json.dumps(results, indent=2) + "\n")


if __name__ == "__main__":
    main()
