#!/usr/bin/env python3
"""Exercise browser patches in one owned, windowed production session.

Pass an existing scene, measured entity API id, and a real numeric source port.
The external runner sequences commands; authored Rhai owns the assertions.
"""
from __future__ import annotations

import argparse
import json
import math
import statistics
import subprocess
import time
from pathlib import Path

from runtime import BINARY, ROOT, ProductionSession


def execute(session: ProductionSession, command: str, params: dict | None = None) -> dict:
    response = session.post({"type": "ExecuteCommand", "command": command, "params": params or {}})
    if response.get("error"):
        raise RuntimeError(f"{command}: {response}")
    return response["data"]


def settled(session: ProductionSession) -> dict:
    deadline = time.monotonic() + 15
    while time.monotonic() < deadline:
        data = execute(session, "InspectTelemetryCatalog")
        if data["initial_scans"] > 0 and data["indexed_channels"] > 0 and not data["pending_channels"] and not data["worker_active"]:
            return data
        time.sleep(0.01)
    raise RuntimeError("incremental catalog did not settle")


def screenshot(session: ProductionSession, name: str) -> None:
    path = ROOT / "target" / name
    path.unlink(missing_ok=True)
    execute(session, "CaptureScreenshot", {"save_to_file": True, "path": str(path.relative_to(ROOT))})
    deadline = time.monotonic() + 10
    while time.monotonic() < deadline:
        if path.is_file():
            return
        time.sleep(0.05)
    raise RuntimeError(f"screenshot was not saved: {path}")


def startup_catalog(session: ProductionSession) -> dict:
    deadline = time.monotonic() + 30
    stable_since = time.monotonic()
    previous = None
    while time.monotonic() < deadline:
        data = execute(session, "InspectTelemetryCatalog")
        signature = (data["catalog_revision"], data["prepared_channels"], data["indexed_channels"])
        if signature != previous or data["pending_channels"] or data["worker_active"] or not data["indexed_channels"]:
            stable_since = time.monotonic()
        elif time.monotonic() - stable_since >= 2:
            return data
        previous = signature
        time.sleep(0.05)
    raise RuntimeError("startup descriptors did not become quiescent")


def verdict(session: ProductionSession, baseline: dict, signal: str, prepared: int, added: int, unit: str, preparing_states: list[bool], histories: tuple[dict, dict] | None = None) -> None:
    constants = {
        "CATALOG_SIGNAL": json.dumps(signal),
        "CATALOG_MISSING_SIGNAL": json.dumps("__catalog_missing_" + str(session.process.pid)),
        "CATALOG_EXPECT_PREPARED": str(prepared),
        "CATALOG_EXPECT_ADDED": str(added),
        "CATALOG_EXPECT_UNIT": json.dumps(unit),
        "CATALOG_PREPARING_OBSERVATIONS": json.dumps(preparing_states),
        "CATALOG_CHECK_HISTORY": "true" if histories is not None else "false",
        "CATALOG_HISTORY_BEFORE": json.dumps([sample["t"] for sample in histories[0]["samples"]]) if histories is not None else "[]",
        "CATALOG_HISTORY_AFTER": json.dumps([sample["t"] for sample in histories[1]["samples"]]) if histories is not None else "[]",
        "CATALOG_BASELINE": "#{" + ", ".join(f"{key}: {baseline[key]}" for key in ["initial_scans", "prepared_channels", "indexed_channels"]) + "}",
    }
    code = "\n".join(f"const {key} = {value};" for key, value in constants.items())
    code += "\n" + (ROOT / "assets/scripting/tests/lib/test_assert.rhai").read_text()
    code += "\n" + (ROOT / "assets/scripting/tests/test_telemetry_catalog.rhai").read_text()
    result = subprocess.run([str(BINARY), "rhai", "--api", str(session.port), "--stdout", "-e", code], cwd=ROOT, capture_output=True, text=True, timeout=30)
    print(result.stdout.strip(), flush=True)
    lines = result.stdout.strip().splitlines()
    if result.returncode or not lines or not lines[-1].startswith("TESTS_OK "):
        print(json.dumps({"baseline": baseline, "current": execute(session, "InspectTelemetryCatalog", {"signal": signal})}), flush=True)
        raise RuntimeError(f"authored catalog verdict failed: {result.stdout}\n{result.stderr}")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--scene", type=Path, required=True)
    parser.add_argument("--entity", type=int, required=True)
    parser.add_argument("--source-port", required=True)
    parser.add_argument("--api-port", type=int, required=True)
    parser.add_argument("--cycles", type=int, default=20)
    parser.add_argument("--output", type=Path, default=ROOT / "target/telemetry-catalog-evidence.json")
    args = parser.parse_args()
    if args.cycles < 1:
        parser.error("--cycles must be positive")
    scene = args.scene.resolve()
    if not scene.is_file():
        raise RuntimeError(f"scene does not exist: {scene}")
    evidence = {"scene": str(scene), "entity": args.entity, "source_port": args.source_port, "cycles": args.cycles, "measurements": []}
    with ProductionSession(args.api_port, windowed=True, extra_args=["--scene", str(scene), "--render-quality", "high", "--no-vsync", "--no-throttle"], log_path=ROOT / "target/telemetry-catalog-runtime.log") as session:
        process_root = Path(f"/proc/{session.process.pid}")
        if process_root.exists():
            if (process_root / "cwd").resolve() != ROOT or (process_root / "exe").resolve() != BINARY.resolve():
                raise RuntimeError("session ownership does not match checkout/binary")
        evidence["pid"] = session.process.pid
        evidence["api_port"] = session.port
        deadline = time.monotonic() + 30
        while time.monotonic() < deadline:
            entities = session.post({"type": "ListEntities"})["data"]["entities"]
            if any(entity["api_id"] == args.entity for entity in entities):
                ports = execute(session, "ListPorts", {"api_id": args.entity})["ports"]
                if any(port["name"] == args.source_port for port in ports):
                    break
            time.sleep(0.05)
        else:
            raise RuntimeError("measured entity/source port was not admitted by the scene")
        execute(session, "ReadPorts", {"api_id": args.entity, "port_names": [args.source_port]})
        execute(session, "ActivatePerspective", {"id": "rover_build"})
        execute(session, "FocusPanel", {"id": "telemetry_browser"})
        execute(session, "SetTelemetryBrowserView", {"filter": "", "signal": ""})
        execute(session, "SetTimeTransport", {"playing": True, "rate": 0.1})
        # Scene admission and first fixed ticks can publish additional descriptors.
        initial = startup_catalog(session)
        evidence["initial"] = initial
        verdict(session, initial, "", 0, 0, "", [initial["preparing"]])
        signal = f"__catalog_probe_{session.process.pid}"
        before = settled(session)
        execute(session, "ControlTelemetry", {"channel": signal, "entity": args.entity, "port": args.source_port, "unit": "W", "rate_hz": 60.0})
        deadline = time.monotonic() + 15
        preparing_states = []
        while time.monotonic() < deadline:
            data = execute(session, "InspectTelemetryCatalog", {"signal": signal})
            preparing_states.append(data["preparing"])
            if data["descriptors"] and not data["pending_channels"] and not data["worker_active"]:
                break
            time.sleep(0.01)
        else:
            raise RuntimeError("probe channel did not enter catalog")
        verdict(session, before, signal, 1, 1, "W", preparing_states)
        screenshot(session, "telemetry-catalog-admission.png")
        # Selection changes must not prepare any descriptor or replace the tree.
        before = settled(session)
        preparing_states = []
        for target in [args.entity, 0, args.entity, 0]:
            execute(session, "SelectEntity", {"entity_id": target})
            selected = settled(session)
            preparing_states.append(selected["preparing"])
        verdict(session, before, signal, 0, 0, "W", preparing_states)
        for index in range(args.cycles):
            before = settled(session)
            unit = "J/s" if index % 2 == 0 else "W"
            started = time.monotonic()
            execute(session, "ControlTelemetry", {"channel": signal, "entity": args.entity, "port": args.source_port, "unit": unit, "rate_hz": 60.0})
            deadline = time.monotonic() + 15
            preparing_states = []
            while time.monotonic() < deadline:
                data = execute(session, "InspectTelemetryCatalog", {"signal": signal})
                preparing_states.append(data["preparing"])
                if data["descriptors"] and data["descriptors"][0]["unit"] == unit and not data["pending_channels"] and not data["worker_active"]:
                    break
                time.sleep(0.01)
            else:
                raise RuntimeError("descriptor metadata did not update")
            observed_latency_ms = (time.monotonic() - started) * 1000
            verdict(session, before, signal, 1, 0, unit, preparing_states)
            evidence["measurements"].append({"capture_ms": data["last_capture_ms"], "worker_ms": data["last_worker_ms"], "commit_ms": data["last_commit_ms"], "observed_latency_ms": observed_latency_ms})
        # Samples continue streaming without descriptor work after the last patch.
        before = settled(session)
        key = f"api/{args.entity}:{signal}"
        history_before = execute(session, "QueryTelemetryHistory", {"key": key, "limit": 10})
        time.sleep(0.5)
        history_after = execute(session, "QueryTelemetryHistory", {"key": key, "limit": 10})
        verdict(session, before, signal, 0, 0, "J/s" if args.cycles % 2 else "W", [settled(session)["preparing"]], (history_before, history_after))
        screenshot(session, "telemetry-catalog-settled.png")
        evidence["final"] = settled(session)
        execute(session, "SetTimeTransport", {"playing": False})
    evidence["session_closed"] = True
    for field in ["capture_ms", "worker_ms", "commit_ms", "observed_latency_ms"]:
        values = sorted(row[field] for row in evidence["measurements"])
        evidence[field] = {"median": statistics.median(values), "p95": values[math.ceil(len(values)*.95)-1], "max": max(values)}
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(evidence, indent=2) + "\n")
    print(json.dumps({field: evidence[field] for field in ["capture_ms", "worker_ms", "commit_ms", "observed_latency_ms", "session_closed"]}, indent=2), flush=True)


if __name__ == "__main__":
    main()
