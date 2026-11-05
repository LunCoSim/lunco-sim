#!/usr/bin/env python3
"""Compare compact shadow-input snapshots only across consecutive host frames."""

import argparse
import gzip
import json
from pathlib import Path


def native_maps(audit):
    """Select active 3D scene cameras; native 2D cascade entries are separate work."""
    inputs = audit["shadow_inputs"]
    if not inputs["native_types_registered"] or inputs["host_frame"] is None:
        return None, None
    active = {row["entity"] for row in audit["candidates"]
              if row["camera_active"] and row["camera_output_mode"] == "write"}
    maps = {}
    for light in inputs["lights"]:
        if not light["shadow_maps_enabled"]:
            continue
        for cascade in light["maps"]:
            if cascade["camera"] not in active or not cascade["camera_3d"]:
                continue
            key = (light["entity"], cascade["camera"], cascade["cascade"])
            if key in maps:
                raise ValueError(f"duplicate native shadow-map identity: {key}")
            fingerprints = cascade["caster_fingerprints"]
            if fingerprints is not None and fingerprints["algorithm"] != "fnv1a64-words-v1":
                raise ValueError("unsupported caster fingerprint algorithm")
            maps[key] = dict(
                light=light["usd_path"], camera_name=cascade["camera_name"],
                matrix=[cascade[name] for name in ["world_from_cascade_bits", "clip_from_cascade_bits",
                                                   "clip_from_world_bits", "texel_size_bits"]],
                fingerprints=fingerprints,
                complete=cascade["matrices_finite"] and fingerprints is not None
                    and fingerprints["poses_available"] and fingerprints["poses_finite"]
                    and fingerprints["meshes_available"],
                candidates=cascade["caster_total"],
            )
    return inputs["host_frame"], maps


def summarize(records):
    """Keep gaps explicit; matching diagnostic keys never authorize render reuse."""
    phases = {}
    previous = {}
    for record in records:
        phase = record["phase"]
        result = phases.setdefault(phase, dict(samples=0, unavailable_samples=0,
            duplicates=0, gapped_pairs=0, skipped_host_frames=0, consecutive_pairs=0, maps={}))
        result["samples"] += 1
        frame, maps = native_maps(record["audit"])
        if frame is None:
            result["unavailable_samples"] += 1
            previous.pop(phase, None)
            continue
        if phase in previous:
            old_frame, old_maps = previous[phase]
            delta = (frame - old_frame) % 2**32
            if delta > 2**31:
                raise ValueError("host frame regressed: records must belong to one process")
            if delta == 0:
                result["duplicates"] += 1
            elif delta != 1:
                result["gapped_pairs"] += 1
                result["skipped_host_frames"] += delta - 1
            else:
                result["consecutive_pairs"] += 1
                for key in old_maps.keys() | maps.keys():
                    current = maps.get(key)
                    old = old_maps.get(key)
                    description = current if current is not None else old
                    stats = result["maps"].setdefault("/".join(map(str, key)), dict(
                        light=description["light"], camera_name=description["camera_name"],
                        comparisons=0, appeared=0, disappeared=0, incomplete=0,
                        projection_changes=0, membership_fingerprint_changes=0,
                        pose_fingerprint_changes=0, mesh_fingerprint_changes=0,
                        same_fingerprints_and_matrices=0))
                    if old is None:
                        stats["appeared"] += 1
                        continue
                    if current is None:
                        stats["disappeared"] += 1
                        continue
                    stats["comparisons"] += 1
                    if not current["complete"] or not old["complete"]:
                        stats["incomplete"] += 1
                        continue
                    changes = [current["matrix"] != old["matrix"]]
                    for field in ["membership", "poses", "meshes"]:
                        changes.append(current["fingerprints"][field] != old["fingerprints"][field])
                    for field, changed in zip(["projection_changes", "membership_fingerprint_changes",
                                               "pose_fingerprint_changes", "mesh_fingerprint_changes"], changes):
                        stats[field] += changed
                    stats["same_fingerprints_and_matrices"] += not any(changes)
        previous[phase] = frame, maps
    return dict(scope="main-world diagnostic fingerprints/matrices; not equality, cache admission, or FPS acceptance",
                phases=phases)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("input", type=Path, help="one owned process's JSONL records: {phase, audit}")
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    opener = gzip.open if args.input.suffix == ".gz" else open
    with opener(args.input, "rt") as stream:
        result = summarize(json.loads(line) for line in stream if line.strip())
    encoded = json.dumps(result, indent=2) + "\n"
    if args.output is not None:
        args.output.write_text(encoded)
    print(encoded, end="")


if __name__ == "__main__":
    main()
