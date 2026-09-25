#!/usr/bin/env python3
"""Compare authored multi-rover physics snapshots across Compute pool widths."""

from __future__ import annotations

import hashlib
import os
from pathlib import Path
import re
import subprocess
import sys
import time


ROOT = Path(__file__).resolve().parents[1]
SCENE = "assets/scenes/tests/multi_rover_stress_20.usda"
TRACE_PATTERN = re.compile(r"D4_STATE_TRACE_V1\|([^\r\n]*)")
EARLY_TRACE_PATTERN = re.compile(r"D4_EARLY_STATE_TRACE_V1\|([^\r\n]*)")
TICK_TRACE_PATTERN = re.compile(r"D4_TICK_STATE_TRACE_V1\|([^\r\n]*)")
MODEL_TRACE_PATTERN = re.compile(r"D4_MODEL_TRACE_V1\|([^\r\n]*)")
ARTICULATED_BODY_TRACE_PATTERN = re.compile(
    r"D4_ARTICULATED_BODY_TRACE_V2\|(\d+)\|([^|\r\n]+)\|([^\r\n]*)"
)
PROFILE_PATTERN = re.compile(r"D4_PROFILE_V1\|(\d+)")
WARMUP_PATTERN = re.compile(r"\[test\] ([^\r\n]+?) held (\d+) updates")


def run_profile(binary: str, threads: int) -> tuple[int, list[str], str, float]:
    command = [
        binary,
        "test",
        "--scene",
        SCENE,
        "--max-ticks",
        "1200",
        "--threads",
        str(threads),
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
        timeout=300,
    )
    elapsed = time.monotonic() - started
    output = result.stdout + result.stderr
    if result.returncode != 0 or "TESTS_OK" not in output:
        relevant = [
            line
            for line in output.splitlines()
            if re.search(
                r"(ERROR|NO-VERDICT|TESTS_|MULTI-ROVER|D4_|Failed to load asset|"
                r"on_start\(\) failed|on_tick\(\) failed)",
                line,
                re.IGNORECASE,
            )
        ]
        tail = "\n".join((relevant or output.splitlines()[-12:])[-24:])
        raise RuntimeError(
            f"Compute profile --threads {threads} failed with exit "
            f"{result.returncode}:\n{tail}"
        )

    profiles = PROFILE_PATTERN.findall(output)
    traces = TRACE_PATTERN.findall(output)
    if len(profiles) != 1:
        raise RuntimeError(
            f"--threads {threads}: expected one effective profile record, "
            f"found {len(profiles)}"
        )
    if len(traces) != 6:
        raise RuntimeError(
            f"--threads {threads}: expected six authored Rhai snapshots, "
            f"found {len(traces)}"
        )
    return int(profiles[0]), traces, output, elapsed


def report_divergence(
    label: str,
    first_profile: int,
    first_trace: list[str],
    second_profile: int,
    second_trace: list[str],
) -> bool:
    if first_trace == second_trace:
        return False
    print(
        f"{label} divergence at Compute widths {first_profile} and {second_profile}",
        file=sys.stderr,
    )
    for index, (first, second) in enumerate(zip(first_trace, second_trace)):
        if first != second:
            first_tick = first.split(";", 1)[0].split("|", 1)[0]
            second_tick = second.split(";", 1)[0].split("|", 1)[0]
            if "Modelica" in label:
                identity = first.split("|", 2)[1]
                first_fields = first.split("|", 2)[2].split(";")
                second_fields = second.split("|", 2)[2].split(";")
                for field_index, (first_field, second_field) in enumerate(
                    zip(first_fields, second_fields)
                ):
                    if first_field != second_field:
                        field_name = first_field.partition("=")[0]
                        print(
                            f"first differing Modelica field at tick {first_tick}: "
                            f"{identity}.{field_name}",
                            file=sys.stderr,
                        )
                        print(f"first:  {first_field}", file=sys.stderr)
                        print(f"second: {second_field}", file=sys.stderr)
                        return True
                print(
                    f"Modelica field count differs at tick {first_tick}: "
                    f"{identity} ({len(first_fields)} and {len(second_fields)})",
                    file=sys.stderr,
                )
                return True
            print(
                f"state snapshot {index}: ticks "
                f"{first_tick} and {second_tick}",
                file=sys.stderr,
            )
            first_rows = first.split(";")
            second_rows = second.split(";")
            for row_index, (first_row, second_row) in enumerate(
                zip(first_rows, second_rows)
            ):
                if first_row != second_row:
                    first_fields = first_row.split("|")
                    second_fields = second_row.split("|")
                    first_contact = next(
                        (field for field in first_fields if field.startswith("avianContact=")),
                        "avianContact=missing",
                    )
                    second_contact = next(
                        (field for field in second_fields if field.startswith("avianContact=")),
                        "avianContact=missing",
                    )
                    print(
                        f"Avian contact snapshot on first changed rover: "
                        f"{first_contact} / {second_contact}",
                        file=sys.stderr,
                    )
                    reported_field = False
                    reported_wheel = False
                    for field_index, (first_field, second_field) in enumerate(
                        zip(first_fields, second_fields)
                    ):
                        if first_field == second_field:
                            continue
                        if "~" in first_field or "~" in second_field:
                            first_wheels = first_field.split("~")
                            second_wheels = second_field.split("~")
                            if first_wheels[0] == second_wheels[0]:
                                for wheel_index, (first_wheel, second_wheel) in enumerate(
                                    zip(first_wheels[1:], second_wheels[1:])
                                ):
                                    if first_wheel != second_wheel:
                                        print(
                                            f"first differing wheel contact: "
                                            f"{first_wheel.partition(',')[0]} "
                                            f"(wheel index {wheel_index})",
                                            file=sys.stderr,
                                        )
                                        print(f"first:  {first_wheel}", file=sys.stderr)
                                        print(f"second: {second_wheel}", file=sys.stderr)
                                        reported_wheel = True
                                        break
                            continue
                        if reported_field:
                            continue
                        field_name = first_field.partition("=")[0]
                        print(f"first differing physics field: {field_name}", file=sys.stderr)
                        print(f"first:  {first_field}", file=sys.stderr)
                        print(f"second: {second_field}", file=sys.stderr)
                        reported_field = True
                    if reported_field or reported_wheel:
                        return True
                    print(f"first:  {first_row[:500]}", file=sys.stderr)
                    print(f"second: {second_row[:500]}", file=sys.stderr)
                    print(f"rover segment index: {row_index - 1}", file=sys.stderr)
                    break
            return True
    print("snapshot counts differ", file=sys.stderr)
    return True


def model_state_trace(output: str, expected_ticks: list[str]) -> list[str]:
    groups: dict[tuple[str, str], tuple[int, dict[int, str]]] = {}
    for payload in MODEL_TRACE_PATTERN.findall(output):
        parts = payload.split("|", 4)
        if len(parts) != 5:
            raise RuntimeError("malformed authored Modelica state trace")
        tick, identity, field_count_text, block_text, fields = parts
        if tick not in expected_ticks:
            raise RuntimeError(
                f"Modelica trace for unexpected tick {tick}; expected "
                f"{','.join(expected_ticks)}"
            )
        field_count = int(field_count_text)
        block = int(block_text)
        key = (tick, identity)
        previous_count, blocks = groups.setdefault(key, (field_count, {}))
        if previous_count != field_count or block in blocks:
            raise RuntimeError(f"inconsistent Modelica trace chunks for {identity}")
        blocks[block] = fields

    expected_group_count = len(expected_ticks) * 20
    if len(groups) != expected_group_count:
        raise RuntimeError(
            f"expected authored state for {expected_group_count} Modelica "
            f"tick/system pairs, found {len(groups)}"
        )

    identities_by_tick = {
        tick: sorted(identity for row_tick, identity in groups if row_tick == tick)
        for tick in expected_ticks
    }
    if any(len(identities) != 20 for identities in identities_by_tick.values()):
        counts = ", ".join(
            f"{tick}:{len(identities)}"
            for tick, identities in identities_by_tick.items()
        )
        raise RuntimeError(f"expected 20 Modelica systems at every snapshot ({counts})")
    expected_identities = identities_by_tick[expected_ticks[0]]
    if any(identities != expected_identities for identities in identities_by_tick.values()):
        raise RuntimeError("Modelica system identities changed between snapshots")

    traces = []
    for tick in expected_ticks:
        for identity in identities_by_tick[tick]:
            field_count, blocks = groups[(tick, identity)]
            block_count = (field_count + 31) // 32
            if sorted(blocks) != list(range(block_count)):
                raise RuntimeError(
                    f"incomplete Modelica trace chunks for {tick} {identity}"
                )
            fields = []
            for block in range(block_count):
                if blocks[block]:
                    fields.extend(blocks[block].split(";"))
            if len(fields) != field_count:
                raise RuntimeError(
                    f"Modelica trace for {tick} {identity} has {len(fields)} of "
                    f"{field_count} fields"
                )
            traces.append(f"{tick}|{identity}|" + ";".join(fields))
    return traces


def articulated_body_trace(output: str) -> dict[tuple[str, str], str]:
    records = ARTICULATED_BODY_TRACE_PATTERN.findall(output)
    if len(records) != 40:
        raise RuntimeError(
            f"expected articulated body traces for ticks 11 and 80, found {len(records)}"
        )
    trace = {(tick, rover_path): state for tick, rover_path, state in records}
    if len(trace) != len(records) or any(not state for state in trace.values()):
        raise RuntimeError("articulated body trace has duplicate rovers or no bodies")
    for tick in ("11", "80"):
        if sum(row_tick == tick for row_tick, _ in trace) != 20:
            raise RuntimeError(f"expected articulated body traces for 20 rovers at tick {tick}")
    return dict(sorted(trace.items()))


def report_articulated_body_divergence(
    label: str,
    reference: dict[tuple[str, str], str],
    candidate: dict[tuple[str, str], str],
) -> bool:
    if reference == candidate:
        return False
    for tick, rover_path in sorted(set(reference) | set(candidate)):
        key = (tick, rover_path)
        if reference.get(key) != candidate.get(key):
            print(
                f"{label} articulated-body divergence at tick {tick} for {rover_path}",
                file=sys.stderr,
            )
            first_bodies = (reference.get(key) or "").split("~")
            second_bodies = (candidate.get(key) or "").split("~")
            first_by_path = {body.split("|", 1)[0]: body for body in first_bodies}
            second_by_path = {body.split("|", 1)[0]: body for body in second_bodies}
            changed = False
            for body_path in sorted(set(first_by_path) | set(second_by_path)):
                if first_by_path.get(body_path) != second_by_path.get(body_path):
                    print(f"first differing body: {body_path}", file=sys.stderr)
                    print(f"first:  {first_by_path.get(body_path)}", file=sys.stderr)
                    print(f"second: {second_by_path.get(body_path)}", file=sys.stderr)
                    changed = True
            if changed:
                return True
            print("articulated body membership changed", file=sys.stderr)
            return True
    return True


def report_warmup(label: str, output: str) -> None:
    counts = WARMUP_PATTERN.findall(output)
    summary = ", ".join(f"{name}: {count}" for name, count in counts)
    print(f"{label} startup update counts: {summary or 'unavailable'}")


def main() -> int:
    binary = os.environ.get("LUNCOSIM_BIN")
    if not binary:
        print("LUNCOSIM_BIN must name the production luncosim binary", file=sys.stderr)
        return 2

    serial_width, serial_trace, serial_output, serial_elapsed = run_profile(binary, 1)
    repeat_width, repeat_trace, repeat_output, repeat_elapsed = run_profile(binary, 1)
    default_width, default_trace, default_output, default_elapsed = run_profile(binary, 0)
    default_repeat_width, default_repeat_trace, default_repeat_output, default_repeat_elapsed = run_profile(binary, 0)
    if serial_width != 1:
        raise RuntimeError(f"serial run reported Compute width {serial_width}, expected 1")
    if repeat_width != serial_width:
        raise RuntimeError(
            f"repeated serial run reported Compute width {repeat_width}, "
            f"expected {serial_width}"
        )
    if default_width <= 1:
        raise RuntimeError(
            f"default run reported Compute width {default_width}; this host did not "
            "provide a distinct profile to compare"
        )
    if default_repeat_width != default_width:
        raise RuntimeError(
            f"repeated default run reported Compute width {default_repeat_width}, "
            f"expected {default_width}"
        )

    report_warmup("serial run 1", serial_output)
    report_warmup("serial run 2", repeat_output)
    report_warmup("default run", default_output)
    report_warmup("default run 2", default_repeat_output)
    early_traces = [
        EARLY_TRACE_PATTERN.findall(output)
        for output in (
            serial_output,
            repeat_output,
            default_output,
            default_repeat_output,
        )
    ]
    if any(len(traces) != 1 for traces in early_traces):
        counts = ", ".join(str(len(traces)) for traces in early_traces)
        raise RuntimeError(
            f"expected one authored early physics snapshot per run ({counts})"
        )
    early_tick = early_traces[0][0].split(";", 1)[0]
    if any(traces[0].split(";", 1)[0] != early_tick for traces in early_traces):
        raise RuntimeError("early physics snapshots were captured at different ticks")
    tick_traces = [
        TICK_TRACE_PATTERN.findall(output)
        for output in (
            serial_output,
            repeat_output,
            default_output,
            default_repeat_output,
        )
    ]
    if any(len(traces) != 32 for traces in tick_traces):
        counts = ", ".join(str(len(traces)) for traces in tick_traces)
        raise RuntimeError(
            f"expected 32 authored physics snapshots per run for ticks 11-30 and "
            f"40-150 every 10 ticks "
            f"({counts})"
        )
    tick_numbers = [
        [trace.split(";", 1)[0] for trace in traces] for traces in tick_traces
    ]
    if any(ticks != tick_numbers[0] for ticks in tick_numbers[1:]):
        raise RuntimeError("early physics snapshots were captured at different ticks")
    serial_ticks = [trace.split(";", 1)[0] for trace in serial_trace]
    repeat_ticks = [trace.split(";", 1)[0] for trace in repeat_trace]
    default_ticks = [trace.split(";", 1)[0] for trace in default_trace]
    default_repeat_ticks = [
        trace.split(";", 1)[0] for trace in default_repeat_trace
    ]
    if any(
        ticks != serial_ticks
        for ticks in (repeat_ticks, default_ticks, default_repeat_ticks)
    ):
        raise RuntimeError("physics snapshots were captured at different simulation ticks")
    model_ticks = []
    fine_modelica_ticks = [
        tick for tick in tick_numbers[0] if 10 <= int(tick) <= 20
    ]
    for tick in (
        serial_ticks[0],
        early_tick,
        *fine_modelica_ticks,
        serial_ticks[1],
    ):
        if tick not in model_ticks:
            model_ticks.append(tick)
    serial_models = model_state_trace(serial_output, model_ticks)
    repeat_models = model_state_trace(repeat_output, model_ticks)
    default_models = model_state_trace(default_output, model_ticks)
    default_repeat_models = model_state_trace(default_repeat_output, model_ticks)
    body_traces = [
        articulated_body_trace(output)
        for output in (
            serial_output,
            repeat_output,
            default_output,
            default_repeat_output,
        )
    ]
    divergences = [
        report_divergence(
            "repeated single-thread Modelica", serial_width, serial_models,
            repeat_width, repeat_models
        ),
        report_divergence(
            "repeated default-profile Modelica", default_width, default_models,
            default_repeat_width, default_repeat_models
        ),
        report_divergence(
            "cross-profile Modelica", serial_width, serial_models,
            default_width, default_models
        ),
        report_divergence(
            "repeated single-thread physics", serial_width, serial_trace,
            repeat_width, repeat_trace
        ),
        report_divergence(
            "repeated single-thread first-tick physics", serial_width,
            early_traces[0], repeat_width, early_traces[1]
        ),
        report_divergence(
            "repeated single-thread physics through tick 150", serial_width,
            tick_traces[0], repeat_width, tick_traces[1]
        ),
        report_divergence(
            "repeated default-profile physics", default_width, default_trace,
            default_repeat_width, default_repeat_trace
        ),
        report_divergence(
            "repeated default-profile first-tick physics", default_width,
            early_traces[2], default_repeat_width, early_traces[3]
        ),
        report_divergence(
            "repeated default-profile physics through tick 150", default_width,
            tick_traces[2], default_repeat_width, tick_traces[3]
        ),
        report_divergence(
            "cross-profile physics", serial_width, serial_trace,
            default_width, default_trace
        ),
    ]
    body_divergences = [
        report_articulated_body_divergence(
            "repeated single-thread", body_traces[0], body_traces[1]
        ),
        report_articulated_body_divergence(
            "repeated default-profile", body_traces[2], body_traces[3]
        ),
        report_articulated_body_divergence(
            "cross-profile", body_traces[0], body_traces[2]
        ),
    ]
    if any(divergences) or any(body_divergences):
        return 1

    digest = hashlib.sha256("\n".join(serial_trace + serial_models).encode()).hexdigest()
    print(
        "DETERMINISTIC_PHYSICS_PROFILES_OK "
        f"compute_widths={serial_width},{default_width} "
        f"snapshots={len(serial_trace)} "
        f"rover_states_per_snapshot={len(serial_trace[0].split(';')) - 1} "
        f"modelica_systems={len(serial_models)} "
        f"sha256={digest} "
        "wall_seconds="
        f"{serial_elapsed:.1f},{repeat_elapsed:.1f},"
        f"{default_elapsed:.1f},{default_repeat_elapsed:.1f}"
    )
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, subprocess.TimeoutExpired, RuntimeError) as error:
        print(error, file=sys.stderr)
        raise SystemExit(1)
