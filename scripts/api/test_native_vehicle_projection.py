#!/usr/bin/env python3
"""Check native support publication on a source-backed vehicle in its real Twin."""
import argparse
import json
import os
import time
from pathlib import Path

from runtime import BINARY, ROOT, ProductionSession, run_authored_scenario


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port", type=int, default=4851)
    parser.add_argument("--twin", required=True)
    parser.add_argument("--subject-path", required=True)
    parser.add_argument("--minimum-support-contacts", type=int, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if args.minimum_support_contacts <= 0:
        parser.error("--minimum-support-contacts must be positive")
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    os.environ.update(LUNCOSIM_EPHEMERAL_SETTINGS="1", LUNCOSIM_ISOLATED_RUN="1",
                      LUNCOSIM_CONFIG=str(output / "config"))
    with ProductionSession(args.port, windowed=True, extra_args=["--no-throttle"],
                           log_path=output / "app.log") as session:
        process = Path("/proc") / str(session.process.pid)
        if process.exists():
            assert (process / "cwd").resolve() == ROOT
            assert (process / "exe").resolve() == BINARY.resolve()

        def execute(command, **params):
            response = session.post({"type": "ExecuteCommand", "command": command, "params": params})
            if response.get("error"):
                raise RuntimeError(response)
            return response.get("data", {})

        execute("OpenTwin", path=str(Path(args.twin).resolve()))
        deadline = time.monotonic() + 120
        while time.monotonic() < deadline:
            ready = execute("GetReadiness")
            if ready.get("faulted"):
                raise RuntimeError(ready)
            models = execute("CosimStatus", include_values=False).get("entities", [])
            if ready.get("ready") is True and ready.get("world_hold") is False and ready.get("pending_count") == 0 and any(
                    row.get("name") == args.subject_path
                    and row.get("status") == "Running"
                    and isinstance(row.get("modelica_current_time"), (int, float))
                    and row["modelica_current_time"] > 0.5 for row in models):
                break
            time.sleep(0.1)
        else:
            raise RuntimeError("source-backed vehicle did not become ready")
        try:
            run_authored_scenario(session, "lunco://scenarios/tests/native_vehicle_projection.rhai",
                title="NATIVE VEHICLE PROJECTION", check_count=6,
                params={"subject_path": args.subject_path,
                        "minimum_support_contacts": args.minimum_support_contacts})
        finally:
            literal = json.dumps(args.subject_path, ensure_ascii=False)
            target = int(execute("RunRhai", code=f"find_path({literal})")["stdout"])
            state = execute("QueryPhysicsState", id=target)
            (output / "physics.json").write_text(json.dumps(state, indent=2) + "\n")
            session.capture_screenshot(output / "result.png")
    (output / "report.json").write_text(json.dumps(
        {"checks": 6, "port": args.port, "port_released": True}, indent=2) + "\n")
    print("6 authored native projection checks passed; owned session and API port released")


if __name__ == "__main__":
    main()
