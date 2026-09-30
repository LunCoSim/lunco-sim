#!/usr/bin/env python3
"""Send native-style left clicks through the production window input API."""

from __future__ import annotations

import argparse
import json
import time
import sys
from pathlib import Path
from typing import Any

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "api"))
from runtime import request_json


def post(port: int, payload: dict[str, Any]) -> dict[str, Any]:
    result = request_json(port, payload, timeout_s=10.0)
    if result.get("error"):
        raise RuntimeError(result["error"])
    return result


def inject_button(port: int, state: str, x: float, y: float) -> None:
    post(
        port,
        {
            "type": "ExecuteCommand",
            "command": "InjectWindowInput",
            "params": {
                "event": {
                    "PointerButton": {
                        "button": "Primary",
                        "state": state,
                        "x": x,
                        "y": y,
                    }
                }
            },
        },
    )


def projected_position(port: int, path: str) -> tuple[float, float]:
    result = post(port, {
        "type": "ExecuteCommand", "command": "RunRhai",
        "params": {"code": f"print(viewport_position(find_path({json.dumps(path)})));"},
    })
    stdout = result.get("data", {}).get("stdout", "").strip()
    try:
        position = json.loads(stdout)
    except json.JSONDecodeError as error:
        raise RuntimeError(f"No visible viewport position for {path}: {stdout}") from error
    if not isinstance(position, list) or len(position) != 2:
        raise RuntimeError(f"Invalid viewport position for {path}: {position}")
    return float(position[0]), float(position[1])


def click_burst(
    port: int,
    label: str,
    clicks: int,
    interval: float,
    hold: float,
    x: float,
    y: float,
) -> None:
    print(
        f"CLICK_BURST_BEGIN label={label} t={time.monotonic():.6f} "
        f"count={clicks} interval={interval} position=({x},{y})",
        flush=True,
    )
    started = time.monotonic()
    for index in range(clicks):
        inject_button(port, "Pressed", x, y)
        time.sleep(hold)
        inject_button(port, "Released", x, y)
        print(f"CLICK label={label} index={index + 1} t={time.monotonic():.6f}", flush=True)
        deadline = started + (index + 1) * interval
        delay = deadline - time.monotonic()
        if delay > 0:
            time.sleep(delay)
    print(f"CLICK_BURST_END label={label} t={time.monotonic():.6f}", flush=True)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port", type=int, required=True)
    parser.add_argument("--first-path", help="resolve the first target through viewport_position")
    parser.add_argument("--first-x", type=float, default=1024.0)
    parser.add_argument("--first-y", type=float, default=760.0)
    parser.add_argument("--second-x", type=float, default=400.0)
    parser.add_argument("--second-y", type=float, default=400.0)
    parser.add_argument("--clicks", type=int, default=24)
    parser.add_argument("--interval", type=float, default=0.4)
    parser.add_argument("--hold", type=float, default=0.04)
    parser.add_argument("--idle-before", type=float, default=6.0)
    parser.add_argument("--idle-between", type=float, default=4.0)
    parser.add_argument("--idle-after", type=float, default=6.0)
    args = parser.parse_args()
    if args.clicks < 1 or min(args.interval, args.hold, args.idle_before,
                              args.idle_between, args.idle_after) < 0:
        parser.error("clicks must be positive and all durations nonnegative")

    print(f"IDLE_BEFORE t={time.monotonic():.6f} seconds={args.idle_before}", flush=True)
    time.sleep(args.idle_before)
    first_position = (args.first_x, args.first_y)
    if args.first_path:
        first_position = projected_position(args.port, args.first_path)
    click_burst(
        args.port,
        "near-rover",
        args.clicks,
        args.interval,
        args.hold,
        *first_position,
    )
    print(f"IDLE_BETWEEN t={time.monotonic():.6f} seconds={args.idle_between}", flush=True)
    time.sleep(args.idle_between)
    click_burst(
        args.port,
        "open-terrain",
        args.clicks,
        args.interval,
        args.hold,
        args.second_x,
        args.second_y,
    )
    print(f"IDLE_AFTER t={time.monotonic():.6f} seconds={args.idle_after}", flush=True)
    time.sleep(args.idle_after)
    print(f"PROFILE_INPUT_COMPLETE t={time.monotonic():.6f}", flush=True)


if __name__ == "__main__":
    main()
