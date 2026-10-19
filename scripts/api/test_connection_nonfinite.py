#!/usr/bin/env python3
"""Qualify non-finite fan-in rejection through Rhai and the HTTP diagnostic."""
from __future__ import annotations

import argparse
import json
import os
import time
from pathlib import Path

from run_editor_scene_test import wait_for_scene, wait_for_scene_time_selection
from runtime import BINARY, ROOT, ProductionSession, run_authored_scenario, wait_for_ready


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port", type=int, default=4396)
    parser.add_argument("--windowed", action="store_true")
    args = parser.parse_args()
    os.environ.update(LUNCOSIM_EPHEMERAL_SETTINGS="1", LUNCOSIM_ISOLATED_RUN="1")
    source = (ROOT / "assets/scenarios/tests/connection_nonfinite.rhai").read_text()
    scene = ROOT / "assets/scenes/tests/twin_session_retirement/replacement/replacement.usda"
    host = "windowed" if args.windowed else "headless"
    for direction, kind in [(1.0, "positive_infinity"), (-1.0, "negative_infinity")]:
        log = ROOT / "target" / f"connection-nonfinite-{host}-{kind}.log"
        with ProductionSession(args.port, windowed=args.windowed, log_path=log) as session:
            pid = session.process.pid
            assert Path(f"/proc/{pid}/cwd").resolve() == ROOT
            assert Path(f"/proc/{pid}/exe").resolve() == BINARY.resolve()
            print(f"OWNED pid={pid} port={args.port}", flush=True)

            def execute(name, **params):
                response = session.post({"type": "ExecuteCommand", "command": name, "params": params})
                if response.get("error"):
                    raise RuntimeError(response)
                return response.get("data", {})

            # The pause is ordered behind scene admission in one command
            # execution, so an unfocused graphical frame cannot advance the
            # fixture before its programs and initial connections are ready.
            execute("RunRhai", code=source + '\nnonfinite_require(cmd("OpenFile", #{path: '
                    + json.dumps(str(scene)) + '}));\n'
                    + 'nonfinite_require(cmd("SetTimeTransport", #{playing: false}));')
            wait_for_scene(session, str(scene), 60)
            wait_for_scene_time_selection(session, log, 60)
            wait_for_ready(args.port)
            execute("SetTimeTransport", playing=False)
            doc = next(d["doc_id"] for d in execute("ListOpenDocuments")["open_documents"]
                       if d["kind"] == "usd" and d.get("active"))
            prepared = execute("RunRhai", code=source + f"\nnonfinite_prepare({doc}, {direction});")
            if "NONFINITE_PREPARED" not in prepared.get("stdout", ""):
                raise RuntimeError(prepared)
            deadline = time.monotonic() + 60
            names = {"/World/LargeA", "/World/LargeB", "/World/Sink"}
            while time.monotonic() < deadline:
                rows = [row for row in execute("CosimStatus")["entities"] if row["name"] in names]
                ready = execute("GetReadiness")
                if ready["faulted"]:
                    raise RuntimeError(f"fixture failed before live exchange: {ready}")
                # Idle is the initialized t=0 publication, ready for its first
                # tick; Running would mean this paused fixture already moved.
                if len(rows) == 3 and all(row["status"] == "Idle"
                                         and row["pending_initial_connections"] is None for row in rows):
                    if any(row["modelica_current_time"] != 0.0 for row in rows):
                        raise RuntimeError(f"fixture advanced before explicit play: {rows}")
                    break
                time.sleep(0.1)
            else:
                raise RuntimeError(f"paused program initialization did not finish: {rows}; {ready}")
            execute("SetTimeTransport", playing=True)
            deadline = time.monotonic() + 60
            while time.monotonic() < deadline:
                ready = execute("GetReadiness")
                if ready["faulted"]:
                    break
                time.sleep(0.1)
            else:
                raise RuntimeError("fan-in did not reach terminal rejection")
            run_authored_scenario(session, "lunco://scenarios/tests/connection_nonfinite.rhai",
                                  title="CONNECTION NONFINITE", check_count=8,
                                  params={"direction": direction}, allow_runtime_fault=True,
                                  timeout_s=15)
            report = execute("GetBrokenConnections")
            rejected = next(b["dropped_value"] for b in report["broken"] if b["port"] == "feedback")
            if rejected != {"kind": kind, "value": None}:
                raise RuntimeError(f"HTTP diagnostic disagrees with authored verdict: {rejected}")
            log.with_suffix(".json").write_text(json.dumps(report, indent=2))
        print(f"PASS {kind}; API Exit and port release verified", flush=True)


if __name__ == "__main__":
    main()
