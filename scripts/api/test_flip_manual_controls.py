#!/usr/bin/env python3
"""Exercise FLIP window key delivery in an explicitly supplied ready session.

Assertions belong to the Twin's authored Rhai gate. This driver only sequences
native input and gives the controller time to apply each ordered event.
"""
import argparse
from pathlib import Path
import time

from runtime import request_json, wait_for_ready


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--api", type=int, required=True)
    parser.add_argument("--twin", type=Path, required=True)
    args = parser.parse_args()
    source = (args.twin / "scenarios/tests/flip_manual_controls.rhai").read_text()
    wait_for_ready(args.api)

    def execute(command, params):
        return request_json(args.api, {
            "type": "ExecuteCommand", "command": command, "params": params,
        })

    def key(code, state):
        execute("InjectWindowInput", {"event": {"Key": {
            "key": code, "state": state, "repeat": False, "text": None,
        }}})

    def check(label, port, value):
        result = execute("RunRhai", {"code": source +
            f'\nif flip_manual_assert("{label}", "{port}", {value}).ok != true '
            '{ throw("FLIP native control acceptance failed"); }'})
        print(result)

    execute("RunRhai", {"code":
        'if !is_controlled(find_path("/Griffin1SurfaceOps/FLIP")) '
        '{ throw("Possess FLIP before running native control acceptance"); }'})

    cases = [
        ("KeyW", "throttle", 1.0), ("KeyS", "throttle", -1.0),
        ("KeyA", "steer", -1.0), ("KeyD", "steer", 1.0),
        ("Space", "brake", 1.0),
    ]
    try:
        for code, port, value in cases:
            key(code, "Pressed")
            time.sleep(0.2)
            check(code + " pressed", port, value)
            key(code, "Released")
            time.sleep(0.2)
            check(code + " released", port, 0.0)
    finally:
        for code, _, _ in cases:
            key(code, "Released")


if __name__ == "__main__":
    main()
