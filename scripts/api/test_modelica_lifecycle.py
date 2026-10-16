#!/usr/bin/env python3
"""Run parameterized Modelica lifecycle gates in sequential owned UI sessions."""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import time
import uuid

from runtime import BINARY, ROOT, ProductionSession, wait_for_ready

SCENARIOS = ("modelica_twin_runtime_retirement", "modelica_duplicate_lifecycle")


def execute(session, command, **params):
    response = session.post({"type": "ExecuteCommand", "command": command, "params": params})
    if response.get("error"):
        raise RuntimeError(f"{command}: {response}")
    return response.get("data", {})


def run_gate(scenario, port, timeout):
    fixture = ROOT / "target" / ("modelica-lifecycle-" + uuid.uuid4().hex)
    fixture.mkdir()
    config = fixture / "config"
    config.mkdir()
    (config / "settings.json").write_text(json.dumps({"experiments": {"max_parallel": 1}}))
    os.environ["LUNCOSIM_CONFIG"] = str(config)
    os.environ["LUNCOSIM_ISOLATED_RUN"] = "1"
    os.environ.pop("LUNCOSIM_EPHEMERAL_SETTINGS", None)
    log = ROOT / "target" / (scenario + ".api.log")
    session = None
    try:
        session = ProductionSession(port, windowed=True, log_path=log)
        with session:
            process = Path("/proc") / str(session.process.pid)
            if process.exists() and ((process / "cwd").resolve() != ROOT
                                     or (process / "exe").resolve() != BINARY.resolve()):
                raise RuntimeError("owned process does not match this checkout/binary")
            print(f"OWNED {scenario}: PID={session.process.pid}, port={port}", flush=True)
            roots = [fixture / "owner_a", fixture / "owner_b"]
            for root in roots:
                root.mkdir()
                execute(session, "NewDocument", kind="usd")
                documents = execute(session, "ListOpenDocuments")["open_documents"]
                doc = next(doc for doc in documents if doc["kind"] == "usd")["doc_id"]
                inspected = execute(session, "InspectUsdDocument", doc_id=doc, path="/World")
                if not inspected["prim"]["exists"] or inspected["layers"]["root"]["id"] != "@root@":
                    raise RuntimeError("scratch USD entry has no valid root/edit target")
                for name, kind in ((root.name, "Scope"), ("TestLight", "DistantLight")):
                    execute(session, "ApplyUsdOp", doc_id=doc, op={"AddPrim": {
                        "edit_target": "@root@", "parent_path": "/World", "name": name,
                        "type_name": kind, "reference": None, "reference_prim_path": None,
                    }})
                execute(session, "SaveAsDocument", doc_id=doc, path=str(root / "scene.usda"))
                execute(session, "InspectUsdDocument", doc_id=doc, path="/World")
                execute(session, "CloseDocument", doc_id=doc)
            if execute(session, "ListOpenDocuments")["open_documents"]:
                raise RuntimeError("fixture preparation left open documents")
            # Application simulation programs need an admitted scene clock;
            # LoadScene supplies it without opening a Workspace Twin.
            execute(session, "LoadScene", path="lunco://scenes/tests/twin_session_retirement/replacement/replacement.usda")
            wait_for_ready(session.port)
            execute(session, "SetTimeTransport", playing=True)
            execute(session, "RunScenarioAsset", source_asset=f"lunco://scenarios/tests/{scenario}.rhai",
                    reload_policy="Retain", params={"root_a": str(roots[0]), "root_b": str(roots[1]),
                                                     "entry_scene": "scene.usda", "marker_a": "/World/owner_a", "marker_b": "/World/owner_b"})
            deadline = time.monotonic() + timeout
            admitted = set()
            while time.monotonic() < deadline:
                output = log.read_text(errors="replace")
                if "[rhai] TESTS_FAIL" in output:
                    raise RuntimeError(f"authored {scenario} verdict failed; log={log}")
                if "[rhai] TESTS_OK" in output:
                    for line in output.splitlines():
                        if "[rhai] TESTS_OK" in line:
                            print(line, flush=True)
                    print(f"PASS {scenario}", flush=True)
                    break
                if session.process.poll() is not None:
                    raise RuntimeError(f"runtime exited before {scenario} verdict; log={log}")
                if scenario == "modelica_twin_runtime_retirement":
                    twin = execute(session, "ReadActiveTwinContract")
                    for owner in ("a", "b"):
                        if owner in admitted or twin.get("name") != "Runtime owner " + owner.upper():
                            continue
                        readiness = execute(session, "GetReadiness")
                        if readiness.get("faulted"):
                            raise RuntimeError(f"fixture scene fault: {readiness}")
                        if not readiness.get("ready") or readiness.get("world_hold"):
                            continue
                        marker = "/World/owner_" + owner
                        try:
                            prim = execute(session, "QueryUsdPrim", path=marker)
                        except RuntimeError as error:
                            if ("not found in the requested document or live stage" in str(error)
                                    or "no USD stage loaded" in str(error)
                                    or "exactly one mounted live stage is required" in str(error)):
                                continue
                            raise
                        if not prim.get("spawned"):
                            continue
                        # The retained Application orchestrator cannot read a
                        # Twin Simulation route. The Command host admits this
                        # exact mounted fixture before native work is submitted.
                        emitted = subprocess.run([str(BINARY), "rhai", "--api", str(port), "--stdout", "-e",
                            'emit("modelica-retirement-scene-ready", ' + json.dumps(marker) + ');'],
                            cwd=ROOT, capture_output=True, text=True, timeout=10)
                        if emitted.returncode:
                            raise RuntimeError(f"scene admission event rejected: {emitted.stdout}\n{emitted.stderr}")
                        admitted.add(owner)
                time.sleep(.25)
            else:
                raise RuntimeError(f"no authored {scenario} verdict within {timeout}s; log={log}")
        print(f"API Exit completed; port {port} released", flush=True)
    finally:
        # Keep source fixtures if shutdown failed and a worker can still read them.
        if session is not None and session.process is not None and session.process.poll() is None:
            print(f"Retained live session fixtures at {fixture}", flush=True)
        else:
            shutil.rmtree(fixture)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port", type=int, default=4310)
    parser.add_argument("--timeout", type=float, default=180)
    parser.add_argument("--scenario", choices=SCENARIOS)
    args = parser.parse_args()
    failures = []
    for index, scenario in enumerate((args.scenario,) if args.scenario else SCENARIOS):
        try:
            run_gate(scenario, args.port + index, args.timeout)
        except Exception as error:
            failures.append(scenario)
            print(f"FAIL {scenario}: {error}", flush=True)
    return int(bool(failures))


if __name__ == "__main__":
    raise SystemExit(main())
