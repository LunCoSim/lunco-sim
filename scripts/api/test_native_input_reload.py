#!/usr/bin/env python3
"""Drive authored runtime-input verdicts before and after a production restart."""
from __future__ import annotations

import argparse
import sys
import time
from pathlib import Path

from runtime import BINARY, ROOT, POLL_INTERVAL_S, ProductionSession, wait_for_ready
from run_editor_scene_test import wait_for_scene, wait_for_scene_time_selection


def execute(session: ProductionSession, command: str, **params):
    response = session.post({"type": "ExecuteCommand", "command": command, "params": params})
    if response.get("error"):
        raise RuntimeError(f"{command}: {response['error']}")
    return response.get("data", {})


def verdict(session: ProductionSession, log: Path, stage: str, timeout: float):
    offset = log.stat().st_size
    owner = execute(session, "GetToolLibrary", name="runtime_ui").get("active_twin")
    if owner is None:
        raise RuntimeError("scene has no admitted Twin owner")
    execute(session, "RunScenarioAsset", source_asset="lunco://scenarios/tests/native_input_reload.rhai",
            owner_twin_id=owner, params={"stage": stage})
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        with log.open(encoding="utf-8", errors="replace") as stream:
            stream.seek(offset)
            output = stream.read()
        if "[rhai] TESTS_FAIL" in output:
            raise RuntimeError(f"authored {stage} verdict failed; log={log}")
        if "[rhai] TESTS_OK 2" in output:
            return
        if session.process.poll() is not None:
            raise RuntimeError("production session exited before its verdict")
        time.sleep(POLL_INTERVAL_S)
    raise RuntimeError(f"no authored {stage} verdict; log={log}")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port", type=int, default=4220)
    parser.add_argument("--timeout", type=float, default=90)
    parser.add_argument("--log", type=Path, default=Path("/tmp/luncosim-native-input-reload.log"))
    args = parser.parse_args()
    scene = ROOT / "assets/scenes/tests/twin_session_retirement/replacement/replacement.usda"
    try:
        with ProductionSession(args.port, extra_args=("--no-throttle", "--scene", str(scene)),
                               log_path=args.log, windowed=True) as session:
            pid = session.process.pid
            if sys.platform.startswith("linux") and (
                Path(f"/proc/{pid}/cwd").resolve() != ROOT
                or Path(f"/proc/{pid}/exe").resolve() != BINARY.resolve()
            ):
                raise RuntimeError("session does not match this checkout and production binary")
            print(f"OWNED pid={pid} port={args.port} cwd={ROOT}", flush=True)
            wait_for_scene(session, str(scene), args.timeout)
            wait_for_scene_time_selection(session, args.log, args.timeout)
            execute(session, "SetTimeTransport", playing=True)
            verdict(session, args.log, "prepare", args.timeout)
            previous = execute(session, "QueryUsdPrim", path="/World", topology=True)["topology"]["binding"]["entity"]
            execute(session, "RestartScene")
            deadline = time.monotonic() + args.timeout
            while time.monotonic() < deadline:
                try:
                    binding = execute(session, "QueryUsdPrim", path="/World", topology=True)["topology"]["binding"]
                    if binding.get("state") == "visual_synced" and binding.get("entity") != previous:
                        break
                except (KeyError, RuntimeError):
                    # The old scene retires before the replacement is queryable.
                    pass
                time.sleep(POLL_INTERVAL_S)
            else:
                raise RuntimeError("scene restart did not publish a fresh projection")
            wait_for_ready(args.port, args.timeout)
            execute(session, "SetTimeTransport", playing=True)
            verdict(session, args.log, "restart", args.timeout)
    except Exception as error:
        print(f"FAIL — {error}", file=sys.stderr)
        return 1
    print("PASS — four authored checks; API Exit and port release verified")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
