#!/usr/bin/env python3
"""Compare authored multi-rover snapshots across pacing, scene sizes, and Compute widths."""

from __future__ import annotations

import hashlib
import json
import math
import os
import argparse
import platform
from pathlib import Path
import re
import shutil
import subprocess
import sys
import time
from datetime import datetime, timezone


ROOT = Path(__file__).resolve().parents[1]
SCENE = "assets/scenes/tests/multi_rover_stress_20.usda"
SCENES_BY_ROVER_COUNT = {
    4: "assets/scenes/tests/multi_rover_stress_4.usda",
    8: "assets/scenes/tests/multi_rover_stress_8.usda",
    20: SCENE,
}
PROFILE_RUNS = 2
EXPECTED_SHARED_ROVERS = 4
EXPECTED_STATE_SNAPSHOT_COUNT = 6
DETERMINISTIC_SEED = 6840157149251759617
JITTER_REPLAY_PROFILES = (
    (0.25, DETERMINISTIC_SEED),
    (0.25, 1234567890123456789),
    (0.5, DETERMINISTIC_SEED),
    (0.5, 1234567890123456789),
)
TRACE_PATTERN = re.compile(r"D4_STATE_TRACE_V1\|([^\r\n]*)")
EARLY_TRACE_PATTERN = re.compile(r"D4_EARLY_STATE_TRACE_V1\|([^\r\n]*)")
FINAL_STAGE_PATTERN = re.compile(r"D4_FINAL_STAGE_V1\|([^\r\n]*)")
MODEL_TRACE_PATTERN = re.compile(r"D4_MODEL_TRACE_V1\|([^\r\n]*)")
ARTICULATED_BODY_TRACE_PATTERN = re.compile(
    r"D4_ARTICULATED_BODY_TRACE_V2\|(\d+)\|([^|\r\n]+)\|([^\r\n]*)"
)
PROFILE_PATTERN = re.compile(r"D4_PROFILE_V1\|(\d+)")
REFERENCE_SCHEMA = "luncosim-deterministic-physics-reference-v1"
ARTICULATED_CHECKPOINT_TICKS = ("1", "11", "80")
PORTABLE_ARTICULATED_CHECKPOINT_TICKS = ("11", "80")


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
    if len(lifecycle_traces) != EXPECTED_STATE_SNAPSHOT_COUNT:
        raise RuntimeError(
            f"{label}: expected {EXPECTED_STATE_SNAPSHOT_COUNT} selected Rhai state "
            f"snapshots, found {len(lifecycle_traces)}"
        )
    startup_tick = raw_sim_tick(lifecycle_traces[0])
    if startup_tick != "0":
        raise RuntimeError(f"{label}: on_start ran at SimTick={startup_tick}, expected 0")
    behavior_traces = EARLY_TRACE_PATTERN.findall(output)
    if len(behavior_traces) != 1:
        raise RuntimeError(
            f"{label}: expected one first-behavior state snapshot, found "
            f"{len(behavior_traces)}"
        )
    first_behavior_tick = raw_sim_tick(behavior_traces[0])
    if first_behavior_tick != "1":
        raise RuntimeError(
            f"{label}: first on_tick ran at SimTick={first_behavior_tick}, expected 1"
        )
    final_stages = FINAL_STAGE_PATTERN.findall(output)
    if len(final_stages) != 1:
        raise RuntimeError(
            f"{label}: expected one explicit final-stage record, found {len(final_stages)}"
        )
    if final_stages[0] != lifecycle_traces[-1]:
        raise RuntimeError(
            f"{label}: explicit final-stage record differs from the final selected state"
        )


def validate_physics_trace_ticks(output: str, label: str) -> None:
    observed = [
        raw_sim_tick(trace) for trace in TRACE_PATTERN.findall(output)
    ]
    repeated = sorted(
        {tick for tick in set(observed) if observed.count(tick) > 1}, key=int
    )
    if (
        len(observed) == EXPECTED_STATE_SNAPSHOT_COUNT
        and observed[0] == "0"
        and len(set(observed)) == len(observed)
        and not repeated
    ):
        return
    raise RuntimeError(
        f"{label}: expected {EXPECTED_STATE_SNAPSHOT_COUNT} unique selected physics "
        f"checkpoints starting at tick 0; observed={observed}, repeated={repeated}"
    )


def canonical_scenario_physics_row(
    record: str, normalize_fixture_contact_counts: bool = True
) -> str:
    fields = record.split("|")
    fields = [field for field in fields if not field.startswith("authoredTf=")]
    for index, field in enumerate(fields):
        if not field.startswith("clock="):
            continue
        values = field.partition("=")[2].split(",")
        if len(values) != 7:
            raise RuntimeError("Rhai clock snapshot has an invalid field count")
        # Physics holds and coupling barriers can clear Time<Physics>::delta
        # after a completed step without changing elapsed time. Rhai checks
        # clock conservation per tick; state equality retains both elapsed
        # clocks and excludes this transient delta.
        fields[index] = "clock=" + ",".join((*values[:3], *values[4:]))
        break
    for index, field in enumerate(fields):
        if not field.startswith("avianContact="):
            continue
        if not normalize_fixture_contact_counts:
            break
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
    jitter: float = 0.0,
    seed: int = DETERMINISTIC_SEED,
    tick_hz: float | None = None,
) -> tuple[int, list[str], str, float]:
    print(
        f"Running production scene {Path(scene).stem} with Compute width "
        f"{threads or 'default'}, jitter {jitter}, seed {seed}, "
        f"tick rate {tick_hz or 'default'} Hz",
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
        str(jitter),
        "--seed",
        str(seed),
    ]
    if tick_hz is not None:
        command.extend(("--tick-hz", str(tick_hz)))
    config = ROOT / "target" / "scene-tests" / (
        f"deterministic-{Path(scene).stem}-{threads}-{jitter}-{seed}-"
        f"{tick_hz or 'default'}hz-{os.getpid()}-"
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
                r"(ERROR|NO-VERDICT|TESTS_|MULTI-ROVER|D4_CLOCK_SYNC_FAIL|D4_ON_TICK_GAP|FAIL:|Failed to load asset|"
                r"on_start\(\) failed|on_tick\(\) failed)",
                line,
                re.IGNORECASE,
            )
            and not any(
                marker in line
                for marker in (
                    "D4_MODEL_TRACE_V1|",
                    "D4_STATE_TRACE_V1|",
                    "D4_STATE_TRACE_V1|",
                    "D4_EARLY_STATE_TRACE_V1|",
                )
            )
        ]
        tail = "\n".join((relevant or output.splitlines()[-12:])[-24:])
        raise RuntimeError(
            f"Compute profile --threads {threads} failed with exit "
            f"{result.returncode}:\n{tail}"
        )

    validate_physics_trace_ticks(output, f"{Path(scene).stem} --threads {threads}")
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
    normalize_fixture_contact_counts: bool = True,
) -> tuple[dict[str, str], dict[str, tuple[str, ...]]]:
    checkpoint_traces = TRACE_PATTERN.findall(output)
    behavior_traces = EARLY_TRACE_PATTERN.findall(output)
    traces = checkpoint_traces + behavior_traces
    mapping_trace = next(
        (trace for trace in checkpoint_traces if "authoredTf=" in trace), None
    )
    if mapping_trace is None:
        raise RuntimeError("Rhai produced no snapshot with authored rover transforms")
    path_positions = authored_roster(mapping_trace, expected_rovers)
    if not shared_positions.issubset(set(path_positions.values())):
        raise RuntimeError("scene did not contain every shared authored rover lane")

    canonical_by_tick: dict[str, tuple[str, ...]] = {}
    observed_ticks: list[str] = []
    expected_ticks = set(expected_sample_ticks)
    expected_ticks.update(raw_sim_tick(trace) for trace in behavior_traces)
    for trace in traces:
        records = rover_state_records(trace)
        tick = raw_sim_tick(trace)
        if tick not in expected_ticks:
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
            selected[position] = canonical_scenario_physics_row(
                normalized,
                normalize_fixture_contact_counts=normalize_fixture_contact_counts,
            )
        if set(selected) != shared_positions:
            raise RuntimeError(
                f"physics tick {tick} did not contain all shared rover states"
            )
        canonical_by_tick[tick] = tuple(
            selected[position] for position in sorted(shared_positions, key=float)
        )
    if len(set(observed_ticks)) != len(observed_ticks):
        raise RuntimeError("Rhai physics trace repeated a comparison simulation tick")
    if set(observed_ticks) != expected_ticks:
        missing = sorted(expected_ticks - set(observed_ticks), key=int)
        raise RuntimeError(f"Rhai physics trace omitted comparison ticks {missing}")
    return path_positions, canonical_by_tick


def canonical_full_scene_trace(
    output: str,
    rover_count: int,
    expected_sample_ticks: set[str],
) -> tuple[dict[str, tuple[str, ...]], dict[tuple[str, str, str], str]]:
    traces = TRACE_PATTERN.findall(output)
    mapping_trace = next(
        (trace for trace in traces if "authoredTf=" in trace), None
    )
    if mapping_trace is None:
        raise RuntimeError("Rhai produced no authored rover transforms")
    path_positions = authored_roster(mapping_trace, rover_count)
    positions = set(path_positions.values())
    all_path_positions, physics = canonical_physics_trace(
        output,
        rover_count,
        positions,
        expected_sample_ticks,
        normalize_fixture_contact_counts=False,
    )
    if all_path_positions != path_positions:
        raise RuntimeError("authored rover roster changed between state samples")
    models = canonical_modelica_trace(
        output, path_positions, positions, set(physics)
    )
    return physics, models


def final_stage_tick(output: str) -> str:
    final_records = FINAL_STAGE_PATTERN.findall(output)
    if len(final_records) != 1:
        raise RuntimeError(
            f"expected one explicit final-stage record, found {len(final_records)}"
        )
    return raw_sim_tick(final_records[0])


def final_modelica_state(
    model_trace: dict[tuple[str, str, str], str], tick: str
) -> dict[tuple[str, str, str], str]:
    return {key: value for key, value in model_trace.items() if key[0] == tick}


def compare_final_stage(
    label: str,
    reference_output: str,
    reference_physics: dict[str, tuple[str, ...]],
    reference_models: dict[tuple[str, str, str], str],
    candidate_output: str,
    candidate_physics: dict[str, tuple[str, ...]],
    candidate_models: dict[tuple[str, str, str], str],
) -> None:
    reference_tick = final_stage_tick(reference_output)
    candidate_tick = final_stage_tick(candidate_output)
    if candidate_tick != reference_tick:
        raise RuntimeError(
            f"{label}: final simulation stage is SimTick={candidate_tick}, "
            f"expected SimTick={reference_tick}"
        )
    if candidate_physics.get(candidate_tick) != reference_physics.get(reference_tick):
        raise RuntimeError(
            f"{label}: final physical state differs exactly at SimTick={candidate_tick}; "
            "numeric_tolerance=0"
        )
    if final_modelica_state(candidate_models, candidate_tick) != final_modelica_state(
        reference_models, reference_tick
    ):
        raise RuntimeError(
            f"{label}: final Modelica state differs exactly at SimTick={candidate_tick}; "
            "numeric_tolerance=0"
        )


def compare_full_roster_replays(
    reference_run: tuple[str, int, list[str], str, float],
    candidate_runs: list[tuple[str, int, list[str], str, float]],
) -> None:
    reference_label, _, reference_traces, reference_output, _ = reference_run
    validate_startup_trace(reference_output, reference_label)
    reference_tick_traces = TRACE_PATTERN.findall(reference_output)
    expected_ticks = {raw_sim_tick(trace) for trace in reference_tick_traces}
    if len(expected_ticks) != len(reference_tick_traces):
        raise RuntimeError(f"{reference_label}: repeated comparison tick")
    reference_physics, reference_models = canonical_full_scene_trace(
        reference_output, 4, expected_ticks
    )
    reference_bodies = articulated_body_trace(reference_output, 4)

    for label, _, traces, output, _ in candidate_runs:
        validate_startup_trace(output, label)
        physics, models = canonical_full_scene_trace(output, 4, expected_ticks)
        compare_final_stage(
            label,
            reference_output,
            reference_physics,
            reference_models,
            output,
            physics,
            models,
        )
        compare_articulated_stages(
            label,
            reference_output,
            reference_bodies,
            output,
            articulated_body_trace(output, 4),
        )
        if physics != reference_physics:
            changed_tick = next((
                tick
                for tick in sorted(set(physics) | set(reference_physics), key=int)
                if physics.get(tick) != reference_physics.get(tick)
            ), "unknown")
            reference_rows = reference_physics.get(changed_tick, ())
            candidate_rows = physics.get(changed_tick, ())
            changed_row = next((
                (left, right)
                for left, right in zip(reference_rows, candidate_rows)
                if left != right
            ), None)
            if changed_row is None:
                detail = "roster or sampled ticks differ"
            else:
                reference_fields = dict(
                    field.split("=", 1)
                    for field in changed_row[0].split("|")
                    if "=" in field
                )
                candidate_fields = dict(
                    field.split("=", 1)
                    for field in changed_row[1].split("|")
                    if "=" in field
                )
                changed_field = next((
                    field
                    for field in dict.fromkeys((
                        *reference_fields.keys(), *candidate_fields.keys()
                    ))
                    if reference_fields.get(field) != candidate_fields.get(field)
                ), "unknown")
                detail = (
                    f"{changed_row[0].split('|', 1)[0]} field={changed_field} "
                    f"reference={reference_fields.get(changed_field)!r} "
                    f"candidate={candidate_fields.get(changed_field)!r}"
                )
            raise RuntimeError(
                f"{label}: full-roster physics differs exactly at SimTick "
                f"{changed_tick} relative to {reference_label}; {detail}; "
                "numeric_tolerance=0"
            )
        if models != reference_models:
            changed = next(
                (
                    key
                    for key in sorted(
                        set(models) | set(reference_models),
                        key=lambda item: (int(item[0]), float(item[1]), item[2]),
                    )
                    if models.get(key) != reference_models.get(key)
                ),
                None,
            )
            raise RuntimeError(
                f"{label}: full-roster Modelica state differs exactly from "
                f"{reference_label} at {changed}; numeric_tolerance=0"
            )
        if traces != reference_traces:
            raise RuntimeError(
                f"{label}: authored lifecycle/milestone states differ from "
                f"{reference_label} after selected physics and Modelica checkpoints matched"
            )


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
    selected_traces = TRACE_PATTERN.findall(output) + EARLY_TRACE_PATTERN.findall(output)
    for trace in selected_traces:
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
            "selected physics checkpoints and first behavior sample"
        )
    return canonical


def first_modelica_trace_difference(
    reference: dict[tuple[str, str, str], str],
    candidate: dict[tuple[str, str, str], str],
    *,
    common_samples_only: bool = False,
) -> tuple[tuple[str, str, str] | None, int]:
    keys = (
        set(reference) & set(candidate)
        if common_samples_only
        else set(reference) | set(candidate)
    )
    ordered_keys = sorted(
        keys,
        key=lambda item: (int(item[0]), float(item[1]), item[2]),
    )
    changed = next(
        (
            key
            for key in ordered_keys
            if reference.get(key) != candidate.get(key)
        ),
        None,
    )
    return changed, len(ordered_keys)


def compare_scenario_matrix(
    runs: dict[int, list[tuple[str, int, list[str], str, float]]],
) -> tuple[int, int]:
    reference_rover_count = max(runs)
    reference_traces = TRACE_PATTERN.findall(runs[reference_rover_count][0][3])
    for rover_count in sorted(runs, reverse=True):
        scene_runs = runs[rover_count]
        for label, _, _, output, _ in scene_runs:
            validate_startup_trace(output, label)
    reference_ticks = [raw_sim_tick(trace) for trace in reference_traces]
    if (
        len(reference_ticks) != EXPECTED_STATE_SNAPSHOT_COUNT
        or len(set(reference_ticks)) != len(reference_ticks)
    ):
        raise RuntimeError(
            f"the {reference_rover_count}-rover Rhai scenario did not emit exactly "
            f"{EXPECTED_STATE_SNAPSHOT_COUNT} unique checkpoints "
            f"(found {reference_ticks})"
        )
    expected_sample_ticks = set(reference_ticks)
    roster_by_run: dict[tuple[int, str], dict[str, str]] = {}
    first_trace_by_scene: dict[int, dict[str, str]] = {}
    for rover_count, scene_runs in runs.items():
        for label, _, _, output, _ in scene_runs:
            traces = TRACE_PATTERN.findall(output)
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
    reference_label = None
    modelica_reference_by_scene: dict[
        int, tuple[str, dict[tuple[str, str, str], str]]
    ] = {}
    articulated_reference_by_scene: dict[
        int, tuple[str, str, dict[tuple[str, str], str]]
    ] = {}
    full_roster_reference_by_scene: dict[
        int, tuple[str, dict[str, tuple[str, ...]], dict[tuple[str, str, str], str]]
    ] = {}
    compared_runs = 0
    for rover_count in sorted(runs, reverse=True):
        scene_runs = runs[rover_count]
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
            bodies = articulated_body_trace(output, rover_count)
            articulated_reference = articulated_reference_by_scene.get(rover_count)
            if articulated_reference is None:
                articulated_reference_by_scene[rover_count] = (label, output, bodies)
            else:
                body_reference_label, body_reference_output, body_reference = (
                    articulated_reference
                )
                compare_articulated_stages(
                    f"{rover_count}-rover articulated state against "
                    f"{body_reference_label}",
                    body_reference_output,
                    body_reference,
                    output,
                    bodies,
                )
            if rover_count in (8, 20):
                # Larger scenes add lanes absent from the smaller fixtures.
                # Cross-fixture comparison intentionally uses the shared four,
                # so compare every rover within each larger scene here.
                all_positions = set(path_positions.values())
                all_path_positions, all_physics = canonical_physics_trace(
                    output,
                    rover_count,
                    all_positions,
                    expected_sample_ticks,
                    normalize_fixture_contact_counts=False,
                )
                if all_path_positions != path_positions:
                    raise RuntimeError(
                        f"{label}: authored {rover_count}-rover roster changed while comparing "
                        "its complete state"
                    )
                all_models = canonical_modelica_trace(
                    output,
                    all_path_positions,
                    all_positions,
                    set(all_physics),
                )
                full_roster_reference = full_roster_reference_by_scene.get(rover_count)
                if full_roster_reference is None:
                    full_roster_reference_by_scene[rover_count] = (
                        label, all_physics, all_models
                    )
                else:
                    full_roster_label, full_roster_physics, full_roster_models = full_roster_reference
                    reference_final_tick = max(full_roster_physics, key=int)
                    candidate_final_tick = final_stage_tick(output)
                    if candidate_final_tick != reference_final_tick or all_physics.get(
                        candidate_final_tick
                    ) != full_roster_physics.get(reference_final_tick):
                        raise RuntimeError(
                            f"full {rover_count}-rover final physical state differs between "
                            f"{full_roster_label} and {label}; exact final-stage "
                            "comparison failed; numeric_tolerance=0"
                        )
                    if final_modelica_state(
                        all_models, candidate_final_tick
                    ) != final_modelica_state(
                        full_roster_models, reference_final_tick
                    ):
                        raise RuntimeError(
                            f"full {rover_count}-rover final Modelica state differs between "
                            f"{full_roster_label} and {label}; numeric_tolerance=0"
                        )
                    if all_physics != full_roster_physics:
                        changed_ticks = [
                            tick
                            for tick in sorted(
                                set(all_physics) | set(full_roster_physics), key=int
                            )
                            if all_physics.get(tick) != full_roster_physics.get(tick)
                        ]
                        raise RuntimeError(
                            f"full {rover_count}-rover physics differs between {full_roster_label} "
                            f"and {label}; first changed tick: "
                            f"{changed_ticks[0] if changed_ticks else 'unknown'}; "
                            "numeric_tolerance=0"
                        )
                    if all_models != full_roster_models:
                        changed = next(
                            (
                                key
                                for key in sorted(
                                    set(all_models) | set(full_roster_models),
                                    key=lambda item: (
                                        int(item[0]), float(item[1]), item[2]
                                    ),
                                )
                                if all_models.get(key) != full_roster_models.get(key)
                            ),
                            None,
                        )
                        raise RuntimeError(
                            f"full {rover_count}-rover Modelica state differs between "
                            f"{full_roster_label} and {label} at {changed}; "
                            "numeric_tolerance=0"
                        )
            if reference_physics is None:
                reference_physics = physics
                reference_label = label
            else:
                reference_final_tick = max(reference_physics, key=int)
                candidate_final_tick = final_stage_tick(output)
                if candidate_final_tick != reference_final_tick or physics.get(
                    candidate_final_tick
                ) != reference_physics.get(reference_final_tick):
                    raise RuntimeError(
                        f"shared rover final physical state differs between "
                        f"{reference_label} and {label}; exact final-stage "
                        "comparison failed; numeric_tolerance=0"
                    )
                if physics != reference_physics:
                    changed_ticks = [
                        tick
                        for tick in sorted(set(physics) | set(reference_physics), key=int)
                        if physics.get(tick) != reference_physics.get(tick)
                    ]
                    raise RuntimeError(
                        f"shared rover physics differs between {reference_label} and "
                        f"{label}; first changed tick: "
                        f"{changed_ticks[0] if changed_ticks else 'unknown'}; "
                        "numeric_tolerance=0"
                    )

            scene_reference = modelica_reference_by_scene.get(rover_count)
            if scene_reference is None:
                modelica_reference_by_scene[rover_count] = (label, models)
            else:
                scene_reference_label, scene_reference_models = scene_reference
                reference_final_tick = max(
                    (key[0] for key in scene_reference_models), key=int
                )
                candidate_final_tick = final_stage_tick(output)
                if final_modelica_state(
                    models, candidate_final_tick
                ) != final_modelica_state(
                    scene_reference_models, reference_final_tick
                ):
                    raise RuntimeError(
                        f"{rover_count}-rover final Modelica state differs between "
                        f"{scene_reference_label} and {label}; numeric_tolerance=0"
                    )
                changed, _ = first_modelica_trace_difference(
                    scene_reference_models, models
                )
                if changed is not None:
                    raise RuntimeError(
                        f"{rover_count}-rover Modelica state differs between "
                        f"{scene_reference_label} and {label} at {changed}; "
                        "numeric_tolerance=0"
                    )

            if rover_count != reference_rover_count:
                reference_modelica = modelica_reference_by_scene[
                    reference_rover_count
                ][1]
                reference_final_tick = max(
                    (key[0] for key in reference_modelica), key=int
                )
                candidate_final_tick = final_stage_tick(output)
                if final_modelica_state(
                    models, candidate_final_tick
                ) != final_modelica_state(
                    reference_modelica, reference_final_tick
                ):
                    raise RuntimeError(
                        f"shared rover final Modelica state differs between "
                        f"{reference_label} and {label}; numeric_tolerance=0"
                    )
                changed, common_sample_count = first_modelica_trace_difference(
                    reference_modelica, models, common_samples_only=True
                )
                if common_sample_count == 0:
                    raise RuntimeError(
                        f"{label}: no common authored Modelica samples with the "
                        f"{reference_rover_count}-rover reference"
                    )
                if changed is not None:
                    raise RuntimeError(
                        f"shared rover Modelica state differs between "
                        f"{reference_label} and {label} at {changed}; "
                        "numeric_tolerance=0"
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


def articulated_body_trace(
    output: str, expected_rovers: int
) -> dict[tuple[str, str], str]:
    records = ARTICULATED_BODY_TRACE_PATTERN.findall(output)
    expected_ticks = set(ARTICULATED_CHECKPOINT_TICKS) | {final_stage_tick(output)}
    expected_record_count = expected_rovers * len(expected_ticks)
    if len(records) != expected_record_count:
        raise RuntimeError(
            f"expected {expected_record_count} articulated body trace records, "
            f"found {len(records)}"
        )
    trace = {(tick, rover_path): state for tick, rover_path, state in records}
    if len(trace) != len(records) or any(not state for state in trace.values()):
        raise RuntimeError("articulated body trace has duplicate rovers or no bodies")
    observed_ticks = {tick for tick, _ in trace}
    if observed_ticks != expected_ticks:
        raise RuntimeError(
            "articulated body trace ticks differ from the selected checkpoints and "
            f"milestone samples: {sorted(observed_ticks, key=int)}"
    )
    for tick in expected_ticks:
        if sum(row_tick == tick for row_tick, _ in trace) != expected_rovers:
            raise RuntimeError(
                f"expected articulated body traces for {expected_rovers} rovers "
                f"at tick {tick}"
            )
    return dict(sorted(trace.items()))


def report_articulated_body_divergence(
    label: str,
    reference: dict[tuple[str, str], str],
    candidate: dict[tuple[str, str], str],
) -> bool:
    if reference == candidate:
        return False
    final_tick = max((tick for tick, _ in reference), key=int)
    for tick, rover_path in sorted(
        set(reference) | set(candidate),
        key=lambda key: (key[0] != final_tick, int(key[0]), key[1]),
    ):
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


def compare_articulated_stages(
    label: str,
    reference_output: str,
    reference: dict[tuple[str, str], str],
    candidate_output: str,
    candidate: dict[tuple[str, str], str],
) -> None:
    reference_tick = final_stage_tick(reference_output)
    candidate_tick = final_stage_tick(candidate_output)
    reference_final = {
        path: state for (tick, path), state in reference.items()
        if tick == reference_tick
    }
    candidate_final = {
        path: state for (tick, path), state in candidate.items()
        if tick == candidate_tick
    }
    if candidate_tick != reference_tick or candidate_final != reference_final:
        raise RuntimeError(
            f"{label}: final articulated stage differs exactly; numeric_tolerance=0"
        )
    if candidate != reference:
        report_articulated_body_divergence(label, reference, candidate)
        raise RuntimeError(
            f"{label}: a selected articulated-body checkpoint differs; "
            "numeric_tolerance=0"
        )


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for block in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def command_version(command: str) -> str | None:
    executable = shutil.which(command)
    if executable is None:
        return None
    result = subprocess.run(
        [executable, "--version"],
        check=False,
        capture_output=True,
        text=True,
        timeout=10,
    )
    return (result.stdout or result.stderr).strip() if result.returncode == 0 else None


def source_metadata(binary: str) -> dict[str, object]:
    def git_value(*args: str) -> str:
        result = subprocess.run(
            ["git", *args], cwd=ROOT, check=True, capture_output=True, text=True
        )
        return result.stdout.strip()

    binary_path = Path(shutil.which(binary) or binary).expanduser().resolve()
    input_paths = sorted(
        {
            Path("Cargo.lock"),
            Path("scripts/compare_deterministic_physics_profiles.py"),
            Path("assets/scenarios/tests/multi_rover_stress.rhai"),
            *(Path(scene) for scene in SCENES_BY_ROVER_COUNT.values()),
        },
        key=str,
    )
    inputs = {
        path.as_posix(): sha256_file(ROOT / path)
        for path in input_paths
    }
    tracked_worktree_clean = subprocess.run(
        ["git", "diff", "--quiet"], cwd=ROOT, check=False
    ).returncode == 0 and subprocess.run(
        ["git", "diff", "--cached", "--quiet"], cwd=ROOT, check=False
    ).returncode == 0
    return {
        "git_commit": git_value("rev-parse", "HEAD"),
        "git_tree": git_value("rev-parse", "HEAD^{tree}"),
        "tracked_worktree_clean": tracked_worktree_clean,
        "input_sha256": inputs,
        "host": {
            "platform": platform.platform(),
            "machine": platform.machine(),
            "processor": platform.processor(),
            "python": platform.python_version(),
            "cargo": command_version("cargo"),
            "rustc": command_version("rustc"),
        },
        "binary": {
            "path": str(binary_path),
            "sha256": sha256_file(binary_path),
        },
    }


def canonical_modelica_point(
    model_trace: dict[tuple[str, str, str], str], tick: str
) -> list[dict[str, str]]:
    return [
        {"lane_x": lane, "system": system, "variables": variables}
        for (point_tick, lane, system), variables in sorted(
            model_trace.items(),
            key=lambda item: (float(item[0][1]), item[0][2]),
        )
        if point_tick == tick
    ]


def canonical_articulated_point(
    body_trace: dict[tuple[str, str], str], tick: str
) -> list[dict[str, str]]:
    return [
        {"rover_path": path, "bodies": state}
        for (point_tick, path), state in sorted(body_trace.items())
        if point_tick == tick
    ]


def portable_state_point(
    tick: str,
    physics: dict[str, tuple[str, ...]],
    models: dict[tuple[str, str, str], str],
    bodies: dict[tuple[str, str], str],
) -> dict[str, object]:
    point = {
        "tick": tick,
        "physics": list(physics[tick]),
        "modelica": canonical_modelica_point(models, tick),
    }
    articulated = canonical_articulated_point(bodies, tick)
    if articulated:
        point["articulated"] = articulated
    if not point["physics"] or not point["modelica"]:
        raise RuntimeError(f"final/checkpoint state at SimTick={tick} is incomplete")
    return point


def make_reference_case(
    name: str,
    run: tuple[str, int, list[str], str, float],
    *,
    scene: str,
    rover_count: int,
    thread_setting: str,
    jitter: float,
    seed: int,
    tick_hz: float | None,
) -> dict[str, object]:
    label, width, _, output, _ = run
    validate_startup_trace(output, label)
    checkpoint_ticks = {raw_sim_tick(trace) for trace in TRACE_PATTERN.findall(output)}
    physics, models = canonical_full_scene_trace(output, rover_count, checkpoint_ticks)
    final_tick = final_stage_tick(output)
    if final_tick not in physics:
        raise RuntimeError(f"{label}: final stage is not a selected physics checkpoint")
    bodies = articulated_body_trace(output, rover_count)
    final = portable_state_point(final_tick, physics, models, bodies)
    selected_ticks = sorted(physics, key=int)
    checkpoints = [
        portable_state_point(tick, physics, models, bodies)
        for tick in selected_ticks
        if tick != final_tick
    ]
    articulated_checkpoints = [
        {"tick": tick, "rovers": canonical_articulated_point(bodies, tick)}
        for tick in PORTABLE_ARTICULATED_CHECKPOINT_TICKS
    ]
    return {
        "parameters": {
            "scene": scene,
            "rover_count": rover_count,
            "thread_setting": thread_setting,
            "jitter": jitter,
            "seed": seed,
            "tick_hz": tick_hz,
        },
        "effective_compute_width": width,
        "first_behavior_tick": raw_sim_tick(EARLY_TRACE_PATTERN.findall(output)[0]),
        "checkpoints": checkpoints,
        "articulated_checkpoints": articulated_checkpoints,
        "final": final,
    }


def collect_reference_cases(
    scene_runs: dict[int, list[tuple[str, int, list[str], str, float]]],
    jitter_runs: list[tuple[str, int, list[str], str, float]],
    tick_rate_runs: list[tuple[str, int, list[str], str, float]],
) -> dict[str, dict[str, object]]:
    cases: dict[str, dict[str, object]] = {}
    for rover_count in (4, 8, 20):
        scene = SCENES_BY_ROVER_COUNT[rover_count]
        cases[f"scene-{rover_count}-serial"] = make_reference_case(
            f"{rover_count} serial reference",
            scene_runs[rover_count][0],
            scene=scene,
            rover_count=rover_count,
            thread_setting="single",
            jitter=0.0,
            seed=DETERMINISTIC_SEED,
            tick_hz=None,
        )
        cases[f"scene-{rover_count}-default"] = make_reference_case(
            f"{rover_count} default reference",
            scene_runs[rover_count][PROFILE_RUNS],
            scene=scene,
            rover_count=rover_count,
            thread_setting="default",
            jitter=0.0,
            seed=DETERMINISTIC_SEED,
            tick_hz=None,
        )

    for profile_index, (jitter, seed) in enumerate(JITTER_REPLAY_PROFILES):
        run = jitter_runs[profile_index * PROFILE_RUNS]
        cases[f"jitter-{jitter:g}-seed-{seed}"] = make_reference_case(
            run[0],
            run,
            scene=SCENES_BY_ROVER_COUNT[4],
            rover_count=4,
            thread_setting="single",
            jitter=jitter,
            seed=seed,
            tick_hz=None,
        )

    cases["tick-rate-30hz"] = make_reference_case(
        tick_rate_runs[0][0],
        tick_rate_runs[0],
        scene=SCENES_BY_ROVER_COUNT[4],
        rover_count=4,
        thread_setting="single",
        jitter=0.0,
        seed=DETERMINISTIC_SEED,
        tick_hz=30.0,
    )
    return cases


def reference_document(binary: str, profiles: dict[str, dict[str, object]]) -> dict[str, object]:
    return {
        "schema": REFERENCE_SCHEMA,
        "recorded_at_utc": datetime.now(timezone.utc).isoformat(),
        "comparison": {
            "numeric_tolerance": 0.0,
            "final_stage_required": True,
            "same_machine_repeatability": "exact",
            "portable_reference": "exact",
            "selected_lifecycle_checkpoints": EXPECTED_STATE_SNAPSHOT_COUNT,
            "selected_articulated_checkpoint_ticks": list(
                PORTABLE_ARTICULATED_CHECKPOINT_TICKS
            ),
        },
        "source": source_metadata(binary),
        "profiles": profiles,
    }


def write_reference(
    path: Path, binary: str, profiles: dict[str, dict[str, object]]
) -> Path:
    document = reference_document(binary, profiles)
    source = document["source"]
    if source["tracked_worktree_clean"] is not True:
        raise RuntimeError(
            "recording a portable reference requires committed, clean tracked source"
        )
    output = path.expanduser().resolve()
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(
        json.dumps(document, indent=2, sort_keys=True, ensure_ascii=False) + "\n",
        encoding="utf-8",
    )
    return output


def compare_reference(
    path: Path, binary: str, profiles: dict[str, dict[str, object]]
) -> None:
    document = json.loads(path.expanduser().read_text(encoding="utf-8"))
    if document.get("schema") != REFERENCE_SCHEMA:
        raise RuntimeError("portable reference uses an unsupported schema")
    recorded_source = document.get("source")
    current_source = source_metadata(binary)
    if recorded_source.get("git_tree") != current_source.get("git_tree"):
        raise RuntimeError("portable reference was recorded from a different source tree")
    if recorded_source.get("input_sha256") != current_source.get("input_sha256"):
        raise RuntimeError("portable reference inputs differ from the current checkout")
    if document.get("comparison", {}).get("numeric_tolerance") != 0.0:
        raise RuntimeError("portable reference must require exact numeric equality")
    recorded_profiles = document.get("profiles")
    if not isinstance(recorded_profiles, dict) or set(recorded_profiles) != set(profiles):
        raise RuntimeError("portable reference profile set differs from the production matrix")

    compare_profile_cases(recorded_profiles, profiles)
    print(
        "DETERMINISTIC_PORTABLE_REFERENCE_OK "
        f"profiles={len(profiles)} final_stage=exact checkpoints="
        f"{EXPECTED_STATE_SNAPSHOT_COUNT} numeric_tolerance=0",
        flush=True,
    )


def compare_profile_cases(
    recorded_profiles: dict[str, dict[str, object]],
    profiles: dict[str, dict[str, object]],
) -> None:
    if set(recorded_profiles) != set(profiles):
        raise RuntimeError("portable reference profile set differs from the production matrix")
    for name in sorted(profiles):
        reference = recorded_profiles[name]
        candidate = profiles[name]
        if reference.get("parameters") != candidate.get("parameters"):
            raise RuntimeError(f"{name}: portable reference parameters do not match")
        if reference.get("final") != candidate.get("final"):
            raise RuntimeError(
                f"{name}: final physics, Modelica, or articulated stage differs "
                "exactly from the recorded machine; numeric_tolerance=0"
            )
        if reference.get("checkpoints") != candidate.get("checkpoints"):
            raise RuntimeError(
                f"{name}: a selected pre-final checkpoint differs from the "
                "recorded machine; numeric_tolerance=0"
            )
        if reference.get("articulated_checkpoints") != candidate.get(
            "articulated_checkpoints"
        ):
            raise RuntimeError(
                f"{name}: an articulated-body checkpoint differs from the "
                "recorded machine; numeric_tolerance=0"
            )


def parse_arguments(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description=(
            "Run the production deterministic-physics matrix and optionally "
            "record or compare a compact cross-machine state reference."
        )
    )
    group = parser.add_mutually_exclusive_group()
    group.add_argument(
        "--record-reference",
        type=Path,
        metavar="PATH",
        help="write selected checkpoint and exact final-stage values after a green matrix",
    )
    group.add_argument(
        "--compare-reference",
        type=Path,
        metavar="PATH",
        help="require exact final-stage and selected-checkpoint equality to a saved reference",
    )
    return parser.parse_args(argv)


def main() -> int:
    args = parse_arguments()
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

    jitter_runs = []
    for jitter, seed in JITTER_REPLAY_PROFILES:
        for repeat in range(1, PROFILE_RUNS + 1):
            width, traces, output, elapsed = run_profile(
                binary,
                1,
                SCENES_BY_ROVER_COUNT[4],
                jitter=jitter,
                seed=seed,
            )
            if width != 1:
                raise RuntimeError(
                    f"4-rover jitter profile reported Compute width {width}, expected 1"
                )
            jitter_runs.append(
                (
                    f"4 jitter {jitter} seed {seed} repeat {repeat}",
                    width,
                    traces,
                    output,
                    elapsed,
                )
            )
    replay_errors = []
    for start in range(0, len(jitter_runs), PROFILE_RUNS):
        profile_repeats = jitter_runs[start : start + PROFILE_RUNS]
        try:
            compare_full_roster_replays(profile_repeats[0], profile_repeats[1:])
        except RuntimeError as error:
            replay_errors.append(str(error))
    for start in range(0, len(jitter_runs), PROFILE_RUNS):
        profile_run = jitter_runs[start]
        try:
            compare_full_roster_replays(scene_runs[4][0], [profile_run])
        except RuntimeError as error:
            replay_errors.append(str(error))

    tick_rate_runs = []
    for repeat in range(1, PROFILE_RUNS + 1):
        width, traces, output, elapsed = run_profile(
            binary,
            1,
            SCENES_BY_ROVER_COUNT[4],
            tick_hz=30.0,
        )
        if width != 1:
            raise RuntimeError(
                f"4-rover 30 Hz profile reported Compute width {width}, expected 1"
            )
        tick_rate_runs.append(
            (f"4 30 Hz repeat {repeat}", width, traces, output, elapsed)
        )
    try:
        compare_full_roster_replays(tick_rate_runs[0], tick_rate_runs[1:])
    except RuntimeError as error:
        replay_errors.append(str(error))

    try:
        shared_rovers, compared_runs = compare_scenario_matrix(scene_runs)
    except RuntimeError as error:
        replay_errors.append(str(error))
    else:
        print(
            "DETERMINISTIC_SCENARIO_MATRIX_OK "
            f"scene_sizes=4,8,20 shared_rovers={shared_rovers} "
            "full_roster_scene_sizes=8,20 "
            f"runs={compared_runs} compared=physics,Modelica",
            flush=True,
        )

    if replay_errors:
        print("DETERMINISM_COMPARISON_FAILURES", file=sys.stderr)
        for error in replay_errors:
            print(f"- {error}", file=sys.stderr)
        return 1

    print(
        "DETERMINISTIC_JITTER_PROFILES_OK "
        "scene_size=4 compute_width=1 jitter=0.25,0.5 "
        f"seeds={JITTER_REPLAY_PROFILES[0][1]},{JITTER_REPLAY_PROFILES[1][1]} "
        f"profiles={len(JITTER_REPLAY_PROFILES)} repeats_per_profile={PROFILE_RUNS} "
        f"runs={len(JITTER_REPLAY_PROFILES) * PROFILE_RUNS} "
        "compared=full_roster_physics,Modelica "
        "against=fixed_step comparison=exact numeric_tolerance=0",
        flush=True,
    )
    print(
        "DETERMINISTIC_TICK_RATE_PROFILE_OK "
        "scene_size=4 compute_width=1 tick_hz=30 repeats=2 "
        "compared=full_roster_physics,Modelica comparison=exact",
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
    if early_tick != "1" or any(
        raw_sim_tick(traces[0]) != early_tick for traces in early_traces
    ):
        raise RuntimeError("the first behavior snapshot was not captured at SimTick=1")
    tick_traces = [
        TRACE_PATTERN.findall(output)
        for output in (
            serial_output,
            repeat_output,
            default_output,
            default_repeat_output,
        )
    ]
    if len(tick_traces[0]) != EXPECTED_STATE_SNAPSHOT_COUNT:
        counts = ", ".join(str(len(traces)) for traces in tick_traces)
        raise RuntimeError(
            f"expected {EXPECTED_STATE_SNAPSHOT_COUNT} selected physics checkpoints "
            f"in the 20-rover runs "
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
    model_ticks = sorted({*serial_ticks, early_tick}, key=int)
    serial_models = model_state_trace(serial_output, model_ticks)
    repeat_models = model_state_trace(repeat_output, model_ticks)
    default_models = model_state_trace(default_output, model_ticks)
    default_repeat_models = model_state_trace(default_repeat_output, model_ticks)
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
            "repeated single-thread selected physics checkpoints", serial_width,
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
            "repeated default-profile selected physics checkpoints", default_width,
            tick_traces[2], default_repeat_width, tick_traces[3]
        ),
        report_divergence(
            "cross-profile physics", serial_width, serial_trace,
            default_width, default_trace
        ),
    ]
    if any(divergences):
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
    if args.record_reference is not None or args.compare_reference is not None:
        profiles = collect_reference_cases(scene_runs, jitter_runs, tick_rate_runs)
        if args.record_reference is not None:
            output = write_reference(args.record_reference, binary, profiles)
            print(f"DETERMINISTIC_REFERENCE_RECORDED path={output}", flush=True)
        else:
            compare_reference(args.compare_reference, binary, profiles)
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, subprocess.TimeoutExpired, RuntimeError, ValueError) as error:
        print(error, file=sys.stderr)
        raise SystemExit(1)
