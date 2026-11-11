#!/usr/bin/env python3
"""Check the terrain streaming event preference in the production runtime."""

import argparse
import json
import os
import time
from pathlib import Path

from runtime import BINARY, ProductionSession, ROOT, assert_render_log_clean, wait_for_ready


def run(session, code):
    reply = session.post({"type": "ExecuteCommand", "command": "RunRhai", "params": {"code": code}})
    if reply.get("error"):
        raise RuntimeError(reply)
    output = reply.get("data", {}).get("stdout", "")
    if "PREFERENCE_OK" not in output:
        raise RuntimeError(f"missing Rhai verdict: {reply}")
    print(output.strip(), flush=True)


def await_saved(config, value):
    deadline = time.monotonic() + 30
    while time.monotonic() < deadline:
        try:
            settings = json.loads((config / "settings.json").read_text())
            if settings.get("terrain", {}).get("show_streaming_events") is value:
                return
        except (OSError, ValueError):
            pass
        time.sleep(0.2)
    raise RuntimeError(f"terrain preference was not persisted as {value}")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--port", type=int, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    config = output / "config"
    os.environ["LUNCOSIM_CONFIG"] = str(config)
    os.environ.pop("LUNCOSIM_EPHEMERAL_SETTINGS", None)
    scenario = (ROOT / "assets/scenarios/tests/terrain_streaming_preference.rhai").read_text()

    for index, (initial, desired) in enumerate(((False, True), (True, False))):
        log = output / f"session-{index}.log"
        with ProductionSession(args.port, windowed=True, log_path=log) as session:
            if (Path(f"/proc/{session.process.pid}/cwd").resolve() != ROOT
                    or Path(f"/proc/{session.process.pid}/exe").resolve() != BINARY.resolve()):
                raise RuntimeError("session belongs to a different checkout")
            wait_for_ready(args.port)
            initial_literal = str(initial).lower()
            desired_literal = str(desired).lower()
            run(session, scenario + f"\nverify_and_set({initial_literal}, {desired_literal});")
            await_saved(config, desired)
        assert_render_log_clean(log)
    print("terrain streaming preference persisted across owned sessions", flush=True)


if __name__ == "__main__":
    main()
