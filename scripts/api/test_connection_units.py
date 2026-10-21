#!/usr/bin/env python3
"""Run the authored connection-contract gate across live USD reprojection."""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path

from run_editor_scene_test import wait_for_scene, wait_for_scene_time_selection
from runtime import BINARY, ROOT, ProductionSession, run_authored_scenario, wait_for_ready


def execute(session, command, **params):
    response = session.post({"type": "ExecuteCommand", "command": command, "params": params})
    if response.get("error"):
        raise RuntimeError(f"{command}: {response}")
    return response.get("data", {})


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port", type=int, default=4396)
    parser.add_argument("--windowed", action="store_true")
    args = parser.parse_args()
    os.environ["LUNCOSIM_EPHEMERAL_SETTINGS"] = "1"
    os.environ["LUNCOSIM_ISOLATED_RUN"] = "1"
    log = ROOT / "target" / ("connection-units-ui.log" if args.windowed else "connection-units.log")
    with ProductionSession(args.port, windowed=args.windowed, log_path=log) as session:
        pid = session.process.pid
        process = Path("/proc") / str(pid)
        if process.exists() and ((process / "cwd").resolve() != ROOT
                                or (process / "exe").resolve() != BINARY.resolve()):
            raise RuntimeError("owned process does not match this checkout/binary")
        print(f"OWNED PID={pid} port={args.port} binary={BINARY}", flush=True)
        execute(session, "OpenFile", path=str(ROOT / "assets/scenes/tests/connection_units.usda"))
        wait_for_scene(session, "scenes/tests/connection_units.usda", 60)
        wait_for_scene_time_selection(session, log, 60)
        wait_for_ready(args.port)
        documents = execute(session, "ListOpenDocuments")["open_documents"]
        doc = next(doc for doc in documents if doc["kind"] == "usd"
                   and doc["title"] == "connection_units")["doc_id"]
        inspected = execute(session, "InspectUsdDocument", doc_id=doc, path="/World/Sun")
        if not inspected["prim"]["exists"] or inspected["layers"]["root"]["id"] != "@root@":
            raise RuntimeError("saved fixture has no standard light/root layer")
        log.with_suffix(".document.json").write_text(json.dumps(inspected, indent=2))
        execute(session, "SetTimeTransport", playing=True)
        run_authored_scenario(
            session, "lunco://scenarios/tests/connection_units.rhai",
            title="CONNECTION UNITS", check_count=6, allow_runtime_fault=True, timeout_s=60,
        )
    print(f"API Exit completed; port {args.port} released", flush=True)


if __name__ == "__main__":
    main()
