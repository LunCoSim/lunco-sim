#!/usr/bin/env python3
"""Run the authored source-isolation gate against an exact live scene and source."""

from __future__ import annotations

import argparse
import json
import os
import sys
import time
from pathlib import Path

from run_editor_scene_test import tail, wait_for_scene, wait_for_scene_time_selection
from runtime import BINARY, ROOT, ProductionSession, POLL_INTERVAL_S


def execute(session, command, params=None):
    response = session.post({"type": "ExecuteCommand", "command": command, "params": params or {}})
    if response.get("error"):
        raise RuntimeError(f"{command}: {response}")
    return response["data"]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port", type=int, required=True)
    parser.add_argument("--scene", type=Path, required=True)
    parser.add_argument("--source", type=Path, required=True)
    parser.add_argument("--selection-path", required=True)
    parser.add_argument("--log", type=Path, required=True)
    parser.add_argument("--screenshot", type=Path)
    parser.add_argument("--timeout", type=float, default=120)
    args = parser.parse_args()
    for path in (args.scene, args.source):
        if not path.is_file():
            parser.error(f"missing fixture: {path}")
    if not 1 <= args.port <= 65535 or args.timeout <= 0:
        parser.error("a valid port and positive timeout are required")
    if not args.selection_path.startswith("/"):
        parser.error("--selection-path must be an absolute USD prim path")
    os.environ.update(LUNCOSIM_EPHEMERAL_SETTINGS="1", LUNCOSIM_ISOLATED_RUN="1",
                      LUNCOSIM_CONFIG=str(args.log.resolve()) + ".config")
    with ProductionSession(args.port, windowed=True, log_path=args.log,
                           extra_args=("--scene", str(args.scene.resolve()))) as session:
        pid = session.process.pid
        if Path(f"/proc/{pid}/cwd").resolve() != ROOT or Path(f"/proc/{pid}/exe").resolve() != BINARY.resolve():
            raise RuntimeError("Runtime process does not match this checkout and binary")
        print(f"OWNED pid={pid} port={args.port} binary={BINARY} cwd={ROOT}", flush=True)
        wait_for_scene(session, str(args.scene.resolve()), args.timeout)
        wait_for_scene_time_selection(session, args.log, args.timeout)
        deadline = time.monotonic() + args.timeout
        clear_since = None
        while time.monotonic() < deadline:
            state = execute(session, "GetReadiness")
            if state["faulted"]:
                raise RuntimeError(f"Fixture faulted before the gate: {state}")
            if state["ready"] and state["readiness_tracked"] and not state["world_hold"] and state["pending_count"] == 0:
                if clear_since is None:
                    clear_since = time.monotonic()
                if time.monotonic() - clear_since >= 5:
                    break
            else:
                clear_since = None
            time.sleep(POLL_INTERVAL_S)
        else:
            raise RuntimeError("Fixture did not retain clear readiness for five seconds")
        execute(session, "SetTimeTransport", {"playing": True})
        owner = execute(session, "GetToolLibrary", {"name": "runtime_ui"})["active_twin"]
        if owner is None:
            raise RuntimeError("Source-isolation fixture has no active Twin owner")
        offset = args.log.stat().st_size
        execute(session, "RunScenarioAsset", {
            "source_asset": "lunco://scenarios/tests/usd_source_isolation.rhai",
            "owner_twin_id": owner,
            "params": {"source": str(args.source.resolve()),
                       "expected_native_path": str(args.source.resolve()),
                       "selection_path": args.selection_path},
        })
        deadline = time.monotonic() + args.timeout
        while time.monotonic() < deadline:
            with args.log.open("r", encoding="utf-8", errors="replace") as log:
                log.seek(offset)
                output = log.read()
            if "[rhai] TESTS_FAIL" in output:
                raise RuntimeError(f"Authored gate failed:\n{output}")
            if "[rhai] TESTS_OK 8" in output and "USD SOURCE ISOLATION: PASS" in output:
                print(output.strip())
                break
            if session.process.poll() is not None:
                raise RuntimeError("Production app exited before the authored verdict")
            diagnostics = execute(session, "RuntimeDiagnostics")
            if diagnostics["errors"] != 0:
                raise RuntimeError(f"Runtime error before the authored verdict: {diagnostics['findings']}")
            time.sleep(POLL_INTERVAL_S)
        else:
            raise RuntimeError(f"No source-isolation verdict; log tail:\n{tail(args.log)}")
        if args.screenshot:
            state = {"pid": pid, "port": args.port, "scene": str(args.scene.resolve()),
                     "source": str(args.source.resolve()), "selection_path": args.selection_path,
                     "viewport": execute(session, "InspectUsdViewport"),
                     "selection": execute(session, "InspectUsdSelection")}
            session.capture_screenshot(args.screenshot)
            Path(str(args.screenshot) + ".json").write_text(json.dumps(state))
    print(f"PASS — source isolation; owned API Exit and port release verified; log={args.log}")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except RuntimeError as error:
        print(f"FAIL — {error}", file=sys.stderr)
        raise SystemExit(1)
