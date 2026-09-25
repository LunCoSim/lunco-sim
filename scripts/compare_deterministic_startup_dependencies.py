#!/usr/bin/env python3
"""Compare first Modelica/Avian reads and Rhai actor order across startups."""

from __future__ import annotations

import hashlib
import os
from pathlib import Path
import re
import subprocess
import sys
import time


ROOT = Path(__file__).resolve().parents[1]
SCENE = "assets/scenes/tests/sensor.usda"
TRACE_PATTERN = re.compile(r"SENSOR_FIRST_TICK_TRACE_V2\|([^\r\n]*)")
ACTOR_ORDER_PATTERN = re.compile(r"SCENARIO_ACTOR_ORDER_TRACE\|([^\r\n]+)")
WARMUP_PATTERN = re.compile(r"\[test\] ([^\r\n]+?) held (\d+) updates")


def run_profile(binary: str, requested_threads: int) -> tuple[int, str, str, float]:
    command = [
        binary,
        "test",
        "--scene",
        SCENE,
        "--max-ticks",
        "600",
        "--threads",
        str(requested_threads),
        "--jitter",
        "0",
        "--seed",
        "6840157149251759617",
    ]
    started = time.monotonic()
    result = subprocess.run(
        command,
        cwd=ROOT,
        check=False,
        capture_output=True,
        text=True,
        errors="replace",
        timeout=180,
    )
    elapsed = time.monotonic() - started
    output = result.stdout + result.stderr
    traces = TRACE_PATTERN.findall(output)
    actor_order = ACTOR_ORDER_PATTERN.findall(output)
    if (
        result.returncode != 0
        or "TESTS_OK" not in output
        or "SENSORS: PASS" not in output
        or actor_order != ["PASS"]
        or len(traces) != 1
    ):
        relevant = [
            line
            for line in output.splitlines()
            if re.search(
                r"(ERROR|NO-VERDICT|TESTS_|SENSORS|SCENARIO_ACTOR_ORDER|"
                r"SENSOR_FIRST_TICK|"
                r"Failed to load asset|on_start\(\) failed|on_tick\(\) failed)",
                line,
                re.IGNORECASE,
            )
        ]
        tail = "\n".join((relevant or output.splitlines()[-12:])[-24:])
        raise RuntimeError(
            f"sensor startup with --threads {requested_threads} failed "
            f"(exit {result.returncode}, traces {len(traces)}):\n{tail}"
        )

    fields = traces[0].split("|")
    if len(fields) != 11:
        raise RuntimeError(f"malformed first-tick trace: {traces[0]!r}")
    effective_threads = int(fields[2])
    if effective_threads < 1:
        raise RuntimeError(
            f"--threads {requested_threads} did not publish an effective "
            f"physics profile: {fields[2]!r}"
        )
    if requested_threads == 1 and effective_threads != 1:
        raise RuntimeError(
            f"--threads 1 reported effective physics width {effective_threads}"
        )
    actor_identity = int(fields[3])
    if actor_identity <= 0:
        raise RuntimeError(
            f"scenario actor did not expose a stable GlobalEntityId: {fields[3]!r}"
        )
    warmups = WARMUP_PATTERN.findall(output)
    held_updates = ";".join(f"{name}:{count}" for name, count in warmups)
    return effective_threads, traces[0], held_updates or "none", elapsed


def authoritative_snapshot(trace: str) -> str:
    fields = trace.split("|")
    # Keep the first tick and scene generation, omitting only the intentionally
    # different Compute pool width.
    return "|".join(fields[:2] + fields[3:])


def main() -> int:
    binary = os.environ.get("LUNCOSIM_BIN")
    if not binary:
        print("LUNCOSIM_BIN must name the production luncosim binary", file=sys.stderr)
        return 2

    serial = [run_profile(binary, 1) for _ in range(2)]
    default = [run_profile(binary, 0) for _ in range(2)]
    serial_widths = {width for width, _, _, _ in serial}
    default_widths = {width for width, _, _, _ in default}
    if serial_widths != {1}:
        raise RuntimeError(f"serial runs reported physics widths {sorted(serial_widths)}")
    if len(default_widths) != 1 or next(iter(default_widths)) <= 1:
        raise RuntimeError(
            f"default runs did not establish one distinct physics width: "
            f"{sorted(default_widths)}"
        )

    runs = serial + default
    snapshots = [authoritative_snapshot(trace) for _, trace, _, _ in runs]
    if any(snapshot != snapshots[0] for snapshot in snapshots[1:]):
        for label, run in zip(
            ("serial 1", "serial 2", "default 1", "default 2"), runs
        ):
            print(f"{label}: {run[1]}", file=sys.stderr)
        raise RuntimeError("first behavior-tick Modelica/Avian snapshots diverged")

    runtimes = ",".join(f"{elapsed:.1f}" for _, _, _, elapsed in runs)
    update_counts = ",".join(counts for _, _, counts, _ in runs)
    digest = hashlib.sha256(snapshots[0].encode()).hexdigest()
    print(
        "DETERMINISTIC_STARTUP_DEPENDENCIES_OK "
        "actor_order=PASS "
        f"compute_widths=1,{next(iter(default_widths))} "
        f"runs=4 first_tick={snapshots[0].split('|', 1)[0]} "
        f"sha256={digest} admission_holds={update_counts} wall_seconds={runtimes}"
    )
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, subprocess.TimeoutExpired, RuntimeError, ValueError) as error:
        print(error, file=sys.stderr)
        raise SystemExit(1)
