#!/usr/bin/env python3
"""Compare authored multi-rover snapshots across repeats, scene sizes, and Compute widths."""

from __future__ import annotations

import hashlib
import math
import os
from pathlib import Path
import re
import subprocess
import sys
import time


ROOT = Path(__file__).resolve().parents[1]
SCENE = "assets/scenes/tests/multi_rover_stress_20.usda"
SCENES_BY_ROVER_COUNT = {
    4: "assets/scenes/tests/multi_rover_stress_4.usda",
    8: "assets/scenes/tests/multi_rover_stress_8.usda",
    20: SCENE,
}
PROFILE_RUNS = 2
EXPECTED_SHARED_ROVERS = 4
AUTHORED_MODEL_TRACE_TICKS = tuple(str(tick) for tick in range(10, 21))
AUTHORED_ARTICULATED_BODY_TRACE_TICKS = ("1", "2", "11", "80")
TRACE_PATTERN = re.compile(r"D4_STATE_TRACE_V1\|([^\r\n]*)")
EARLY_TRACE_PATTERN = re.compile(r"D4_EARLY_STATE_TRACE_V1\|([^\r\n]*)")
TICK_TRACE_PATTERN = re.compile(r"D4_TICK_STATE_TRACE_V1\|([^\r\n]*)")
MODEL_TRACE_PATTERN = re.compile(r"D4_MODEL_TRACE_V1\|([^\r\n]*)")
ARTICULATED_BODY_TRACE_PATTERN = re.compile(
    r"D4_ARTICULATED_BODY_TRACE_V2\|(\d+)\|([^|\r\n]+)\|([^\r\n]*)"
)
PROFILE_PATTERN = re.compile(r"D4_PROFILE_V1\|(\d+)")


def rover_state_records(trace: str) -> list[str]:
    # The Modelica clock summary uses semicolons too, but is not a rover row.
    rover_payload = trace.split("|modelica=", 1)[0]
    return rover_payload.split(";")


def raw_sim_tick(trace: str) -> str:
    try:
        tick = int(rover_state_records(trace)[0])
    except ValueError as error:
        raise RuntimeError("Rhai state trace has an invalid simulation tick") from error
    if tick < 0:
        raise RuntimeError(f"Rhai state trace has negative simulation tick {tick}")
    return str(tick)


def validate_startup_trace(output: str, label: str) -> None:
    lifecycle_traces = TRACE_PATTERN.findall(output)
    if len(lifecycle_traces) != 6:
        raise RuntimeError(
            f"{label}: expected six Rhai lifecycle snapshots, found {len(lifecycle_traces)}"
        )
    startup_tick = raw_sim_tick(lifecycle_traces[0])
    if startup_tick != "0":
        raise RuntimeError(f"{label}: on_start ran at SimTick={startup_tick}, expected 0")
    behavior_traces = TICK_TRACE_PATTERN.findall(output)
    if not behavior_traces:
        raise RuntimeError(f"{label}: Rhai emitted no fixed behavior snapshots")
    first_behavior_tick = raw_sim_tick(behavior_traces[0])
    if first_behavior_tick != "1":
        raise RuntimeError(
            f"{label}: first on_tick ran at SimTick={first_behavior_tick}, expected 1"
        )


def canonical_scenario_physics_row(record: str) -> str:
    fields = record.split("|")
    for index, field in enumerate(fields):
        if not field.startswith("avianContact="):
            continue
        values = field.partition("=")[2].split(",")
        if len(values) != 7:
            raise RuntimeError("Rhai contact snapshot has an invalid field count")
        # Contact-pair, touching-pair, and manifold counts describe the entire
        # fixture contact graph. Same-fixture checks still compare those counts.
        fields[index] = "avianContactSolve=" + ",".join(values[3:])
    return "|".join(fields)


def run_profile(
    binary: str,
    threads: int,
    scene: str = SCENE,
) -> tuple[int, list[str], str, float]:
    print(
        f"Running production scene {Path(scene).stem} with Compute width "
        f"{threads or 'default'}",
        flush=True,
    )
    command = [
        binary,
        "test",
        "--scene",
        scene,
        "--max-ticks",
        "1200",
        "--threads",
        str(threads),
        "--jitter",
        "0",
        "--seed",
        "6840157149251759617",
    ]
    config = ROOT / "target" / "scene-tests" / (
        f"deterministic-{Path(scene).stem}-{threads}-{os.getpid()}-"
        f"{time.monotonic_ns()}"
    )
    config.parent.mkdir(parents=True, exist_ok=True)
    environment = os.environ.copy()
    environment["LUNCOSIM_CONFIG"] = str(config)
    environment["LUNCO_ASSET_ROOT"] = str(ROOT / "assets")
    started = time.monotonic()
    result = subprocess.run(
        command,
        cwd=ROOT,
        env=environment,
        check=False,
        capture_output=True,
        text=True,
        errors="replace",
        timeout=300,
    )
    elapsed = time.monotonic() - started
    output = result.stdout + result.stderr
    log_path = ROOT / "target" / "scene-tests" / (
        f"{Path(scene).stem}-threads-{threads}-"
        f"{os.getpid()}-{time.monotonic_ns()}.log"
    )
    log_path.write_text(output, encoding="utf-8")
    if (
        result.returncode != 0
        or "TESTS_OK" not in output
        or "MULTI-ROVER STRESS: PASS" not in output
    ):
        relevant = [
            line
            for line in output.splitlines()
            if re.search(
                r"(ERROR|NO-VERDICT|TESTS_|MULTI-ROVER|D4_|FAIL:|Failed to load asset|"
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


def authored_roster(trace: str, expected_rovers: int) -> dict[str, str]:
    records = rover_state_records(trace)
    if len(records) != expected_rovers + 1:
        raise RuntimeError(
            f"expected {expected_rovers} rover states in authored trace, "
            f"found {len(records) - 1}"
        )
    path_positions: dict[str, str] = {}
    positions: set[str] = set()
    for record in records[1:]:
        path, separator, state = record.partition("|")
        if not separator or not path.startswith("/MultiRoverStress/Rovers/"):
            raise RuntimeError(f"malformed authored rover state: {record[:160]!r}")
        lane = re.search(r"(?:^|\|)laneX=([^|]+)", state)
        authored = re.search(r"(?:^|\|)authoredTf=([^|]+)", state)
        if lane is None or authored is None:
            raise RuntimeError(
                "Rhai snapshot omitted the authored lane identity needed to "
                "match equivalent rovers across scene sizes"
            )
        coordinates = authored.group(1).split(",")
        if len(coordinates) != 3:
            raise RuntimeError(f"malformed authored transform for {path}")
        try:
            lane_position = float(lane.group(1))
            authored_position = float(coordinates[0])
        except ValueError as error:
            raise RuntimeError(f"invalid authored lane for {path}") from error
        if not math.isclose(
            lane_position, authored_position, rel_tol=0.0, abs_tol=1e-9
        ):
            raise RuntimeError(f"authored lane does not match USD translation for {path}")
        position = format(lane_position, ".17g")
        if path in path_positions or position in positions:
            raise RuntimeError("authored rover paths and start positions must be unique")
        path_positions[path] = position
        positions.add(position)
    return path_positions


def canonical_physics_trace(
    output: str,
    expected_rovers: int,
    shared_positions: set[str],
    expected_sample_ticks: set[str],
) -> tuple[dict[str, str], dict[str, tuple[str, ...]]]:
    traces = TICK_TRACE_PATTERN.findall(output)
    if len(traces) < len(expected_sample_ticks):
        raise RuntimeError(
            f"expected at least {len(expected_sample_ticks)} Rhai physics snapshots for "
            f"the {expected_rovers}-rover scene, found {len(traces)}"
        )
    mapping_trace = next(
        (trace for trace in traces if "authoredTf=" in trace), None
    )
    if mapping_trace is None:
        raise RuntimeError("Rhai produced no snapshot with authored rover transforms")
    path_positions = authored_roster(mapping_trace, expected_rovers)
    if not shared_positions.issubset(set(path_positions.values())):
        raise RuntimeError("scene did not contain every shared authored rover lane")

    canonical_by_tick: dict[str, tuple[str, ...]] = {}
    observed_ticks: list[str] = []
    for trace in traces:
        records = rover_state_records(trace)
        tick = raw_sim_tick(trace)
        if tick not in expected_sample_ticks:
            continue
        observed_ticks.append(tick)
        if len(records) != expected_rovers + 1:
            raise RuntimeError(
                f"physics tick {tick} has {len(records) - 1} of "
                f"{expected_rovers} rover states"
            )
        selected: dict[str, str] = {}
        for record in records[1:]:
            path, separator, state = record.partition("|")
            if not separator or path not in path_positions:
                raise RuntimeError(f"physics tick {tick} contains an unknown rover")
            if state == "invalid-state":
                raise RuntimeError(f"physics tick {tick} contains an invalid rover state")
            position = path_positions[path]
            if position not in shared_positions:
                continue
            normalized = record
            for source_path, source_position in sorted(
                path_positions.items(), key=lambda item: len(item[0]), reverse=True
            ):
                normalized = normalized.replace(
                    source_path, f"RoverAtX[{source_position}]"
                )
            normalized = re.sub(r"\|bevyEntity=[^|]*", "", normalized)
            selected[position] = canonical_scenario_physics_row(normalized)
        if set(selected) != shared_positions:
            raise RuntimeError(
                f"physics tick {tick} did not contain all shared rover states"
            )
        canonical_by_tick[tick] = tuple(
            selected[position] for position in sorted(shared_positions, key=float)
        )
    if len(set(observed_ticks)) != len(observed_ticks):
        raise RuntimeError("Rhai physics trace repeated a comparison simulation tick")
    if set(observed_ticks) != expected_sample_ticks:
        missing = sorted(expected_sample_ticks - set(observed_ticks), key=int)
        raise RuntimeError(f"Rhai physics trace omitted comparison ticks {missing}")
    return path_positions, canonical_by_tick


def canonical_modelica_trace(
    output: str,
    path_positions: dict[str, str],
    shared_positions: set[str],
    physics_ticks: set[str],
) -> dict[tuple[str, str, str], str]:
    groups: dict[tuple[str, str, str], tuple[int, dict[int, str]]] = {}
    for payload in MODEL_TRACE_PATTERN.findall(output):
        parts = payload.split("|", 4)
        if len(parts) != 5:
            raise RuntimeError("malformed authored Modelica state trace")
        absolute_tick, identity, field_count_text, block_text, fields = parts
        try:
            tick = str(int(absolute_tick))
        except ValueError as error:
            raise RuntimeError("Modelica trace has an invalid simulation tick") from error
        if tick not in physics_ticks and tick != "0":
            continue
        owners = [
            path for path in path_positions if identity.startswith(path + "/")
        ]
        if not owners:
            raise RuntimeError(f"Modelica trace has no authored rover owner: {identity}")
        owner = max(owners, key=len)
        position = path_positions[owner]
        if position not in shared_positions:
            continue
        owner_name = owner.rsplit("/", 1)[-1]
        suffix = identity[len(owner):]
        if owner_name not in suffix:
            raise RuntimeError(
                f"Modelica system identity does not retain its USD rover name: {identity}"
            )
        suffix = suffix.replace(owner_name, f"RoverAtX[{position}]")
        try:
            field_count = int(field_count_text)
            block = int(block_text)
        except ValueError as error:
            raise RuntimeError("Modelica trace has a non-numeric block header") from error
        key = (tick, position, suffix)
        previous_count, blocks = groups.setdefault(key, (field_count, {}))
        if previous_count != field_count or block in blocks:
            raise RuntimeError(f"inconsistent Modelica trace chunks for {identity}")
        blocks[block] = fields

    canonical: dict[tuple[str, str, str], str] = {}
    for key, (field_count, blocks) in groups.items():
        block_count = (field_count + 31) // 32
        if sorted(blocks) != list(range(block_count)):
            raise RuntimeError(f"incomplete Modelica state chunks for {key}")
        fields = []
        for block in range(block_count):
            if blocks[block]:
                fields.extend(blocks[block].split(";"))
        if len(fields) != field_count:
            raise RuntimeError(f"incomplete Modelica field list for {key}")
        canonical[key] = ";".join(fields)

    fine_ticks: set[str] = set()
    for trace in TICK_TRACE_PATTERN.findall(output):
        if "authoredTf=" not in trace:
            continue
        tick = raw_sim_tick(trace)
        if tick in physics_ticks:
            fine_ticks.add(tick)
    expected = {
        (tick, position)
        for tick in fine_ticks | {"0"}
        for position in shared_positions
    }
    actual = {(tick, position) for tick, position, _ in canonical}
    if not fine_ticks or not expected.issubset(actual):
        raise RuntimeError(
            "Rhai Modelica trace did not capture every shared rover at the "
            "fine-grained physics sample ticks"
        )
    return canonical


def compare_scenario_matrix(
    runs: dict[int, list[tuple[str, int, list[str], str, float]]],
) -> tuple[int, int]:
    reference_rover_count = max(runs)
    reference_traces = TICK_TRACE_PATTERN.findall(runs[reference_rover_count][0][3])
    for rover_count, scene_runs in runs.items():
        for label, _, _, output, _ in scene_runs:
            validate_startup_trace(output, label)
    reference_ticks = [raw_sim_tick(trace) for trace in reference_traces]
    if len(reference_ticks) < 32 or len(set(reference_ticks)) != len(reference_ticks):
        raise RuntimeError(
            f"the {reference_rover_count}-rover Rhai scenario did not emit at least 32 unique "
            f"comparison ticks (found {reference_ticks})"
        )
    expected_sample_ticks = set(reference_ticks)
    roster_by_run: dict[tuple[int, str], dict[str, str]] = {}
    first_trace_by_scene: dict[int, dict[str, str]] = {}
    for rover_count, scene_runs in runs.items():
        for label, _, _, output, _ in scene_runs:
            traces = TICK_TRACE_PATTERN.findall(output)
            mapping_trace = next(
                (trace for trace in traces if "authoredTf=" in trace), None
            )
            if mapping_trace is None:
                raise RuntimeError(f"{label}: missing authored rover transforms")
            roster = authored_roster(mapping_trace, rover_count)
            roster_by_run[(rover_count, label)] = roster
        first_trace_by_scene[rover_count] = roster_by_run[
            (rover_count, scene_runs[0][0])
        ]

    shared_positions = set.intersection(
        *(set(roster.values()) for roster in first_trace_by_scene.values())
    )
    if len(shared_positions) != EXPECTED_SHARED_ROVERS:
        raise RuntimeError(
            f"expected {EXPECTED_SHARED_ROVERS} authored rover lanes shared by "
            f"the 4/8/20 scenes, found {sorted(shared_positions, key=float)}"
        )

    reference_physics = None
    reference_models = None
    reference_label = None
    compared_runs = 0
    for rover_count, scene_runs in runs.items():
        for label, _, _, output, _ in scene_runs:
            path_positions, physics = canonical_physics_trace(
                output, rover_count, shared_positions, expected_sample_ticks,
            )
            if path_positions != roster_by_run[(rover_count, label)]:
                raise RuntimeError(f"{label}: authored rover roster changed between samples")
            models = canonical_modelica_trace(
                output,
                path_positions,
                shared_positions,
                set(physics),
            )
            if reference_physics is None:
                reference_physics = physics
                reference_models = models
                reference_label = label
            else:
                if physics != reference_physics:
                    changed_ticks = [
                        tick
                        for tick in sorted(set(physics) | set(reference_physics), key=int)
                        if physics.get(tick) != reference_physics.get(tick)
                    ]
                    raise RuntimeError(
                        f"shared rover physics differs between {reference_label} and "
                        f"{label}; first changed tick: "
                        f"{changed_ticks[0] if changed_ticks else 'unknown'}"
                    )
                if models != reference_models:
                    changed = next(
                        (
                            key
                            for key in sorted(
                                set(models) | set(reference_models),
                                key=lambda item: (
                                    int(item[0]), float(item[1]), item[2]
                                ),
                            )
                            if models.get(key) != reference_models.get(key)
                        ),
                        None,
                    )
                    raise RuntimeError(
                        f"shared rover Modelica state differs between {reference_label} "
                        f"and {label} at {changed}"
                    )
            compared_runs += 1
    return len(shared_positions), compared_runs


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
            first_rows = rover_state_records(first)
            second_rows = rover_state_records(second)
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
    expected_record_count = 20 * len(AUTHORED_ARTICULATED_BODY_TRACE_TICKS)
    if len(records) != expected_record_count:
        raise RuntimeError(
            f"expected {expected_record_count} articulated body trace records, "
            f"found {len(records)}"
        )
    trace = {(tick, rover_path): state for tick, rover_path, state in records}
    if len(trace) != len(records) or any(not state for state in trace.values()):
        raise RuntimeError("articulated body trace has duplicate rovers or no bodies")
    observed_ticks = {tick for tick, _ in trace}
    if observed_ticks != set(AUTHORED_ARTICULATED_BODY_TRACE_TICKS):
        raise RuntimeError(
            "articulated body trace ticks differ from the authored startup and "
            f"milestone samples: {sorted(observed_ticks, key=int)}"
        )
    for tick in AUTHORED_ARTICULATED_BODY_TRACE_TICKS:
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

    scene_runs = {
        20: [
            ("20 serial 1", serial_width, serial_trace, serial_output, serial_elapsed),
            ("20 serial 2", repeat_width, repeat_trace, repeat_output, repeat_elapsed),
            ("20 default 1", default_width, default_trace, default_output, default_elapsed),
            ("20 default 2", default_repeat_width, default_repeat_trace,
             default_repeat_output, default_repeat_elapsed),
        ]
    }
    for rover_count in (4, 8):
        scene_runs[rover_count] = []
        scene = SCENES_BY_ROVER_COUNT[rover_count]
        for label, threads in (("serial", 1), ("default", 0)):
            for repeat in range(1, PROFILE_RUNS + 1):
                width, traces, output, elapsed = run_profile(binary, threads, scene)
                scene_runs[rover_count].append(
                    (f"{rover_count} {label} {repeat}", width, traces, output, elapsed)
                )
        widths = [run[1] for run in scene_runs[rover_count]]
        serial_widths = widths[:PROFILE_RUNS]
        default_widths = widths[PROFILE_RUNS:]
        if (
            serial_widths != [1] * PROFILE_RUNS
            or not default_widths
            or default_widths[0] <= 1
            or any(width != default_widths[0] for width in default_widths)
        ):
            raise RuntimeError(
                f"{rover_count}-rover scene did not establish repeated serial and "
                f"default Compute profiles: {widths}"
            )
    shared_rovers, compared_runs = compare_scenario_matrix(scene_runs)
    print(
        "DETERMINISTIC_SCENARIO_MATRIX_OK "
        f"scene_sizes=4,8,20 shared_rovers={shared_rovers} "
        f"runs={compared_runs} compared=physics,Modelica",
        flush=True,
    )

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
    early_tick = raw_sim_tick(early_traces[0][0])
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
    if not tick_traces[0] or len(tick_traces[0]) < 32:
        counts = ", ".join(str(len(traces)) for traces in tick_traces)
        raise RuntimeError(
            f"expected at least 32 authored physics snapshots in the 20-rover runs "
            f"({counts})"
        )
    tick_numbers = [[raw_sim_tick(trace) for trace in traces] for traces in tick_traces]
    if any(ticks != tick_numbers[0] for ticks in tick_numbers[1:]):
        raise RuntimeError("early physics snapshots were captured at different ticks")
    serial_ticks = [raw_sim_tick(trace) for trace in serial_trace]
    repeat_ticks = [raw_sim_tick(trace) for trace in repeat_trace]
    default_ticks = [raw_sim_tick(trace) for trace in default_trace]
    default_repeat_ticks = [raw_sim_tick(trace) for trace in default_repeat_trace]
    if any(
        ticks != serial_ticks
        for ticks in (repeat_ticks, default_ticks, default_repeat_ticks)
    ):
        raise RuntimeError("physics snapshots were captured at different simulation ticks")
    model_ticks = []
    for tick in (
        serial_ticks[0],
        early_tick,
        *AUTHORED_MODEL_TRACE_TICKS,
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
        f"scene_sizes=4,8,20 shared_rovers={shared_rovers} "
        f"matrix_runs={compared_runs} "
        f"compute_widths={serial_width},{default_width} "
        f"snapshots={len(serial_trace)} "
        f"rover_states_per_snapshot={len(rover_state_records(serial_trace[0])) - 1} "
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
    except (OSError, subprocess.TimeoutExpired, RuntimeError, ValueError) as error:
        print(error, file=sys.stderr)
        raise SystemExit(1)
