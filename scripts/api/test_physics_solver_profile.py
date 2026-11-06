#!/usr/bin/env python3
"""Drive Rhai solver-profile lint verdicts and inspect the native status consumer."""
from __future__ import annotations

import argparse
import json
import os
import time
from pathlib import Path

from runtime import BINARY, ROOT, ProductionSession, get_json, wait_for_ready
from run_editor_scene_test import wait_for_scene, wait_for_scene_time_selection


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port", type=int, required=True)
    parser.add_argument("--log", type=Path, required=True)
    parser.add_argument("--timeout", type=float, default=90)
    args = parser.parse_args()
    os.environ.update(LUNCOSIM_EPHEMERAL_SETTINGS="1", LUNCOSIM_ISOLATED_RUN="1")
    os.environ.setdefault("LUNCOSIM_CONFIG", str(args.log.resolve().parent / f"solver-profile-{args.port}-config"))
    scene = ROOT / "assets/scenes/tests/authored_runtime_contracts.usda"
    source = "lunco://scenarios/tests/physics_solver_profile.rhai"
    with ProductionSession(args.port, windowed=True, extra_args=["--scene", str(scene), "--no-throttle"], log_path=args.log) as session:
        proc = Path(f"/proc/{session.process.pid}")
        if proc.exists() and (proc.joinpath("cwd").resolve() != ROOT or proc.joinpath("exe").resolve() != BINARY.resolve()):
            raise RuntimeError("owned session does not match this checkout and binary")
        print(f"OWNED pid={session.process.pid} port={session.port}", flush=True)
        wait_for_scene(session, str(scene), args.timeout)
        wait_for_scene_time_selection(session, args.log, args.timeout)
        wait_for_ready(session.port, args.timeout)
        args.log.with_suffix(".schema.json").write_text(json.dumps(
            get_json(session.port, "/api/commands/schema"), indent=2) + "\n")

        def command(name, **params):
            result = session.post({"type": "ExecuteCommand", "command": name, "params": params})
            if result.get("error"):
                raise RuntimeError(result)
            return result.get("data", {})

        command("RunScenarioAsset", source_asset=source)
        installed = set()
        deadline = time.monotonic() + args.timeout
        while time.monotonic() < deadline:
            if session.process.poll() is not None:
                raise RuntimeError("owned solver-profile session exited before its verdict")
            output = args.log.read_text(errors="replace")
            if "TESTS_FAIL" in output or "[rhai] on_start() failed:" in output or "[rhai] on_tick() failed:" in output:
                raise RuntimeError("authored solver-profile verdict failed")
            for marker in ["high", "restore", "maximum", "same", "final"]:
                if f"PHYSICS_PROFILE_STIMULUS:{marker}" in output and marker not in installed:
                    if marker == "restore":
                        args.log.with_suffix(".high.json").write_text(json.dumps({
                            "diagnostics": command("GetDiagnostics", scope="physics"),
                            "readiness": command("GetReadiness"),
                        }, indent=2) + "\n")
                        session.capture_screenshot(args.log.with_suffix(".high.png"))
                    command("RunRhai", code=f'import "{source}" as profile; profile::install_profile("{marker}");')
                    installed.add(marker)
            if "TESTS_OK 10" in output and "PHYSICS_SOLVER_PROFILE_LINT" in output:
                break
            time.sleep(.1)
        else:
            raise RuntimeError(f"solver-profile lint did not complete; log={args.log}")
        result = command("RunRhai", code=f'import "{source}" as profile; profile::verify_invalid_profile();')["stdout"]
        print(result, flush=True)
        if "TESTS_OK 5" not in result or "TESTS_FAIL" in result:
            raise RuntimeError("invalid-profile verdict failed")
        print("PASS — ten live lint/event checks and five input/scope checks; owned API Exit", flush=True)


if __name__ == "__main__":
    main()
