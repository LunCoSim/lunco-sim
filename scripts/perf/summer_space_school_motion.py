#!/usr/bin/env python3
"""Move and rotate the Summer Space School avatar through native semantic movement and pointer input.

The driver measures displacement and yaw from the live avatar. Pointer samples
exercise ordinary picking during rotation. All held inputs are released even
when a command fails; no camera pose or USD opinion is authored.
"""

from __future__ import annotations

import argparse
import json
import math
import time
import sys
from pathlib import Path
from typing import Any

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "api"))
from runtime import request_json


DEFAULT_AVATAR_PATH = "/Traverse/Avatar"
DEFAULT_PRODUCER_ID = 4110004110001


def post(port: int, payload: dict[str, Any], timeout: float = 10.0) -> dict[str, Any]:
    result = request_json(port, payload, timeout_s=timeout)
    if result.get("error"):
        raise RuntimeError(result["error"])
    return result


def query_entity(port: int, gid: int) -> dict[str, Any]:
    response = post(
        port,
        {"type": "ExecuteCommand", "command": "QueryEntity", "params": {"id": gid}},
    )
    data = response.get("data")
    if not isinstance(data, dict) or not isinstance(data.get("position"), list):
        raise RuntimeError(f"QueryEntity returned no pose for API id {gid}: {response}")
    return data


def find_avatar(port: int, requested_path: str | None) -> tuple[int, dict[str, Any]]:
    response = post(port, {"type": "ListEntities"})
    entities = response.get("data", {}).get("entities", [])
    for entity in entities:
        if entity.get("name") != "Avatar":
            continue
        gid = int(entity["api_id"])
        pose = query_entity(port, gid)
        if requested_path is None or pose.get("usd_prim_path") == requested_path:
            return gid, pose
    wanted = requested_path or "an entity named Avatar"
    raise RuntimeError(f"Could not find {wanted} in the active scene")


def set_intent(port: int, gid: int, producer_id: int, intent: str, held: bool) -> None:
    post(
        port,
        {
            "type": "ExecuteCommand",
            "command": "SimulateIntent",
            "params": {
                "intent": intent,
                "held": held,
                "target": gid,
                "producer_id": producer_id,
            },
        },
    )


def move_avatar(
    port: int,
    gid: int,
    producer_id: int,
    intent: str,
    duration: float,
) -> tuple[dict[str, Any], dict[str, Any]]:
    before = query_entity(port, gid)
    print(f"MOVE_BEGIN intent={intent} seconds={duration:.2f} t={time.monotonic():.3f}", flush=True)
    set_intent(port, gid, producer_id, intent, True)
    try:
        time.sleep(duration)
    finally:
        set_intent(port, gid, producer_id, intent, False)
    after = query_entity(port, gid)
    start = before["position"]
    end = after["position"]
    delta = [float(b) - float(a) for a, b in zip(start, end, strict=True)]
    distance = math.sqrt(sum(component * component for component in delta))
    print(
        "MOVE_END "
        f"t={time.monotonic():.3f} start={start} end={end} "
        f"delta={delta} distance_m={distance:.3f}",
        flush=True,
    )
    if distance < 0.1:
        raise RuntimeError("Avatar moved less than 0.1 m; check scene controls and Twin readiness")
    return before, after


def move_pointer(port: int, x: float, y: float) -> None:
    post(
        port,
        {
            "type": "ExecuteCommand",
            "command": "InjectWindowInput",
            "params": {"event": {"PointerMove": {"x": x, "y": y}}},
        },
    )


def rotate_avatar(
    port: int,
    gid: int,
    degrees: float,
    step_degrees: float,
    timeout: float,
    pointer_center: tuple[float, float],
    pointer_radius: float,
) -> None:
    if degrees == 0:
        return
    response = post(port, {"type": "ExecuteCommand", "command": "RunRhai",
                          "params": {"code": 'print(input_binding("look_button"));'}})
    label = response["data"]["stdout"].strip()
    buttons = {"left mouse button": "Primary", "right mouse button": "Secondary",
               "middle mouse button": "Middle"}
    if label not in buttons:
        raise RuntimeError(f"Unsupported native look button binding: {label}")
    button = buttons[label]
    wanted = math.radians(abs(degrees))
    sign = -1.0 if degrees > 0 else 1.0
    previous = float(query_entity(port, gid)["euler"][0])
    turned = 0.0
    gain = None
    samples = 0
    started = time.monotonic()
    interval = min(0.1, timeout / max(1, math.ceil(abs(degrees) / step_degrees)))
    def button_input(state: str) -> None:
        post(port, {"type": "ExecuteCommand", "command": "InjectWindowInput",
                    "params": {"event": {"PointerButton": {
                        "button": button, "state": state,
                        "x": pointer_center[0], "y": pointer_center[1]}}}})
    print(f"ROTATE_BEGIN button={button} degrees={degrees:g} timeout={timeout:g} "
          f"t={started:.3f}", flush=True)
    button_input("Pressed")
    try:
        time.sleep(interval)
        # QueryEntity reports the rendered quaternion as native f32 Euler
        # angles; stop within its explicit angular measurement tolerance.
        while turned < wanted - 1e-4:
            if time.monotonic() - started >= timeout:
                raise RuntimeError(f"Native avatar rotation reached {math.degrees(turned):.2f} "
                                   f"degrees before the {timeout:g}s timeout")
            # Measure the rig's response first, then use that response for each
            # bounded angular step. No camera-specific sensitivity is guessed.
            units = 1.0 if gain is None else min(math.radians(step_degrees), wanted - turned) / gain
            delta_x = units if degrees > 0 else -units
            post(port, {"type": "ExecuteCommand", "command": "InjectWindowInput",
                        "params": {"event": {"MouseMotion": {
                            "delta_x": delta_x, "delta_y": 0.0}}}})
            # Command admission precedes the controller's next input pass. Wait
            # for the measured turn rather than treating a fixed sleep as an
            # input-consumption barrier, particularly in diagnostic builds.
            while True:
                time.sleep(interval)
                current = float(query_entity(port, gid)["euler"][0])
                delta = sign * math.atan2(math.sin(current - previous), math.cos(current - previous))
                if delta < -1e-5:
                    raise RuntimeError("Avatar rotated opposite to the injected motion")
                if delta > 1e-5:
                    break
                if time.monotonic() - started >= timeout:
                    raise RuntimeError("Native mouse look was not consumed before the deadline; check UI capture and camera binding")
            gain = delta / units
            turned += delta
            previous = current
            samples += 1
            angle = 2.0 * math.pi * turned / wanted
            move_pointer(port, pointer_center[0] + pointer_radius * math.cos(angle),
                         pointer_center[1] + pointer_radius * math.sin(angle))
    finally:
        button_input("Released")
    print(f"ROTATE_END actual_degrees={math.degrees(turned):.3f} samples={samples} "
          f"t={time.monotonic():.3f} elapsed={time.monotonic() - started:.3f}", flush=True)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port", type=int, default=4111, help="existing local API port")
    parser.add_argument(
        "--avatar-path",
        default=DEFAULT_AVATAR_PATH,
        help="USD path of the avatar camera; use an empty string to select any Avatar",
    )
    parser.add_argument("--move-intent", default="forward", help="held semantic avatar intent")
    parser.add_argument("--move-seconds", type=float, default=6.0)
    parser.add_argument("--rotate-degrees", type=float, default=360.0)
    parser.add_argument("--rotate-step-degrees", type=float, default=10.0)
    parser.add_argument("--rotate-seconds", type=float, default=12.0)
    parser.add_argument("--pointer-center-x", type=float, default=640.0)
    parser.add_argument("--pointer-center-y", type=float, default=360.0)
    parser.add_argument("--pointer-radius", type=float, default=160.0)
    parser.add_argument("--producer-id", type=int, default=DEFAULT_PRODUCER_ID)
    parser.add_argument(
        "--rotate-only",
        action="store_true",
        help="skip movement and rotate the current avatar",
    )
    args = parser.parse_args()
    if args.producer_id <= 0:
        parser.error("--producer-id must be nonzero")
    if not all(math.isfinite(value) for value in (
        args.move_seconds, args.rotate_seconds, args.rotate_degrees,
        args.rotate_step_degrees, args.pointer_center_x,
        args.pointer_center_y, args.pointer_radius,
    )):
        parser.error("motion parameters must be finite")
    if (
        args.move_seconds < 0
        or args.rotate_seconds <= 0
        or args.rotate_step_degrees <= 0
        or args.pointer_radius < 0
    ):
        parser.error(
            "durations and pointer radius must be nonnegative, and rotation step must be positive"
        )

    requested_path = args.avatar_path or None
    gid, pose = find_avatar(args.port, requested_path)
    print(
        f"AVATAR api_id={gid} path={pose.get('usd_prim_path')} "
        f"position={pose['position']} euler={pose.get('euler')}",
        flush=True,
    )
    if not args.rotate_only and args.move_seconds > 0:
        _, pose = move_avatar(
            args.port,
            gid,
            args.producer_id,
            args.move_intent,
            args.move_seconds,
        )
    rotate_avatar(
        args.port,
        gid,
        args.rotate_degrees,
        args.rotate_step_degrees,
        args.rotate_seconds,
        (args.pointer_center_x, args.pointer_center_y),
        args.pointer_radius,
    )


if __name__ == "__main__":
    main()
