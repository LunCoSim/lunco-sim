#!/usr/bin/env python3
"""Attribute fixed-step response time in a settled Tracy interval to live Modelica participants."""

import argparse
import bisect
import collections
import csv
import json
import math
from pathlib import Path
import re
import statistics
import subprocess


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("capture", type=Path)
    parser.add_argument("start_seconds", type=float)
    parser.add_argument("end_seconds", type=float)
    parser.add_argument("--exporter", type=Path, required=True)
    args = parser.parse_args()
    lo, hi = (round(t * 1e9) for t in (args.start_seconds, args.end_seconds))
    if lo >= hi:
        parser.error("the interval must have positive duration")

    stages = ("dispatch_step", "live_step_job", "ordered_step_receipt", "admit_step_result")
    events = collections.defaultdict(lambda: collections.defaultdict(list))
    waves = []
    phases = collections.defaultdict(list)
    solver_jobs = []
    with subprocess.Popen(
        [str(args.exporter), "-u", "-s", "|", "-f", "modelica_", str(args.capture)],
        stdout=subprocess.PIPE, text=True,
    ) as export:
        for row in csv.DictReader(export.stdout, delimiter="|"):
            start = int(row["ns_since_start"])
            if not lo <= start < hi:
                continue
            end = start + int(row["exec_time_ns"])
            if end > hi:
                continue
            name = row["name"]
            if "::spawn_modelica_requests" in name:
                waves.append((start, end))
                continue
            phase = next((phase for phase in ("admit_step_inputs", "integrate_macro_step", "collect_step_observables")
                          if name.startswith("modelica_" + phase)), None)
            if phase:
                phases[row["thread"]].append((start, end, phase))
                continue
            stage = next((key for key in stages if name.startswith("modelica_" + key)), None)
            if stage:
                entity = re.search(r'entity\s*=\s*(\d+v\d+)', name + " " + row["value"])
                if entity is None:
                    raise ValueError(f"missing participant identity in {name}")
                events[entity[1]][stage].append((start, end))
                if stage == "live_step_job":
                    solver_jobs.append((entity[1], row["thread"], start, end))
        if export.wait():
            raise RuntimeError("Tracy could not read the capture")

    waves.sort()
    starts = [start for start, _ in waves]
    transactions = collections.defaultdict(list)
    incomplete = set()
    for entity, records in events.items():
        for values in records.values():
            values.sort()
        dispatches = records[stages[0]]
        for i, (dispatch, _) in enumerate(dispatches):
            wave = bisect.bisect_right(starts, dispatch) - 1
            if wave < 0 or dispatch > waves[wave][1]:
                continue
            stop = dispatches[i + 1][0] if i + 1 < len(dispatches) else hi
            matched = []
            for stage in stages[1:]:
                values = records[stage]
                idx = bisect.bisect_left(values, (dispatch, -1))
                if idx == len(values) or values[idx][1] > stop:
                    break
                matched.append(values[idx])
            if len(matched) != 3:
                incomplete.add(wave)
                continue
            (started, finished), (wait_start, received), (admitting, admitted) = matched
            if finished > received or received > admitting:
                incomplete.add(wave)
                continue
            transactions[wave].append((entity, started, finished, wait_start, received, admitting, admitted))

    metrics = collections.defaultdict(list)
    last_solver = collections.Counter()
    last_admitted = collections.Counter()
    wave_latency = []
    for wave, rows in transactions.items():
        if wave in incomplete:
            continue
        finisher = max(rows, key=lambda row: row[2])[0]
        wave_latency.append((max(row[6] for row in rows) - waves[wave][0], wave, finisher))
        last_solver[finisher] += 1
        last_admitted[max(rows, key=lambda row: row[6])[0]] += 1
        for entity, started, finished, wait_start, received, admitting, admitted in rows:
            metrics[entity].append(tuple(t / 1e6 for t in (
                started - waves[wave][0], finished - started, received - wait_start,
                admitting - received, admitted - waves[wave][0],
            )))
    if not metrics:
        raise ValueError("no complete dispatch waves with all four owner markers")
    phase_times = collections.defaultdict(lambda: collections.defaultdict(list))
    for rows in phases.values():
        rows.sort()
    for entity, thread, start, end in solver_jobs:
        rows = phases[thread]
        index = bisect.bisect_left(rows, (start, -1, ""))
        while index < len(rows) and rows[index][0] < end:
            first, last, phase = rows[index]
            if last <= end:
                phase_times[entity][phase].append((last - first) / 1e6)
            index += 1
    names = ("wave_to_job", "solver_job", "ordered_receipt", "receipt_to_admission", "wave_to_admitted")
    tail = sorted(wave_latency, reverse=True)[:math.ceil(len(wave_latency) * .05)]
    print(json.dumps({
        "complete_waves": sum(last_solver.values()),
        "incomplete_waves": len(incomplete),
        "slowest_five_percent_last_solver": collections.Counter(row[2] for row in tail),
        "participants": {entity: {
            "steps": len(rows), "last_solver": last_solver[entity],
            "last_admitted": last_admitted[entity],
            "solver_phase_median_ms": {phase: statistics.median(values) for phase, values in phase_times[entity].items()},
            "median_ms": dict(zip(names, map(statistics.median, zip(*rows)))),
        } for entity, rows in sorted(metrics.items())},
    }, indent=2))


if __name__ == "__main__":
    main()
