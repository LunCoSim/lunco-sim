#!/usr/bin/env python3
"""Measure achieved simulation speed between two mission events in a runtime log."""

import argparse
from datetime import datetime
import math
from pathlib import Path
import re


EVENT = re.compile(
    r"^(?P<wall>\S+).*?\[BLACKBOX\] EVENT: name=(?P<name>[^,]+),"
    r".*?, sim_secs=(?P<sim>[^,]+), sim_tick=(?P<tick>\d+)\b"
)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("log", type=Path)
    parser.add_argument("start_event")
    parser.add_argument("end_event")
    parser.add_argument("--min-rate", type=float, help="Exit 1 below this real-time factor")
    parser.add_argument("--expected-ticks", type=int, help="Require this many completed ticks")
    args = parser.parse_args()
    if args.start_event == args.end_event:
        parser.error("select two distinct events")
    if args.min_rate is not None and not (math.isfinite(args.min_rate) and args.min_rate > 0):
        parser.error("minimum rate must be finite and positive")
    if args.expected_ticks is not None and args.expected_ticks <= 0:
        parser.error("expected tick count must be positive")

    events = {}
    with args.log.open() as log:
        for line in log:
            match = EVENT.match(line)
            if not match or match["name"] not in (args.start_event, args.end_event):
                continue
            name = match["name"]
            if name in events:
                parser.error(f"duplicate event {name!r} in this log")
            wall = datetime.fromisoformat(match["wall"])
            if wall.utcoffset() is None:
                parser.error(f"event {name!r} has no wall-clock time zone")
            events[name] = (wall, float(match["sim"]), int(match["tick"]))
    for name in (args.start_event, args.end_event):
        if name not in events:
            parser.error(f"missing event {name!r}")
    start, end = events[args.start_event], events[args.end_event]
    ticks = end[2] - start[2]
    sim = end[1] - start[1]
    wall = (end[0] - start[0]).total_seconds()
    if ticks <= 0 or not math.isfinite(sim) or sim <= 0 or wall <= 0:
        parser.error("event tick, simulation time and wall time must all increase")
    if args.expected_ticks is not None and ticks != args.expected_ticks:
        parser.error(f"expected {args.expected_ticks} ticks, measured {ticks}")
    rate = sim / wall
    print(f"{rate:.4f}×  {sim:.6f} simulated s / {wall:.6f} wall s  ({ticks} ticks)")
    return 1 if args.min_rate is not None and rate < args.min_rate else 0


if __name__ == "__main__":
    raise SystemExit(main())
