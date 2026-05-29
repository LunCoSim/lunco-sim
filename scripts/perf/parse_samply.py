#!/usr/bin/env python3
"""Flatten a samply / Firefox-Profiler ``profile.json(.gz)`` into per-function
self-time and inclusive-time tables.

samply records native stack samples; the Firefox Profiler UI is great for
interactive exploration but useless in a headless agent loop. This script
reduces a capture to two ranked tables we can read in the terminal:

* **self-time** — where the CPU actually sat (leaf of each stack). This is the
  number that tells you which function to optimise.
* **inclusive-time** — every function that appeared anywhere in a stack. Useful
  for attributing cost to a whole subsystem (e.g. ``bevy_render`` extract).

Usage::

    scripts/perf/parse_samply.py <profile.json|.gz> [top_n]

Kept dependency-free (stdlib only) so it runs anywhere the capture lands.
"""
import sys
import json
import gzip
import collections


def load(path):
    """Load a samply capture, transparently handling gzip."""
    opener = gzip.open if path.endswith(".gz") else open
    with opener(path, "rt") as f:
        return json.load(f)


def main():
    if len(sys.argv) < 2:
        sys.exit(__doc__)
    path = sys.argv[1]
    top_n = int(sys.argv[2]) if len(sys.argv) > 2 else 40
    prof = load(path)

    self_time = collections.Counter()   # leaf-of-stack samples, keyed (thread, fn)
    total_time = collections.Counter()  # appears-anywhere-in-stack samples
    interval = prof.get("meta", {}).get("interval", 1.0)  # ms per sample

    for thr in prof.get("threads", []):
        tname = thr.get("name", "?")
        funcs = thr["funcTable"]
        # Firefox Profiler renamed stringTable -> stringArray across versions.
        strings = thr.get("stringArray") or thr.get("stringTable") or []
        func_name = [
            strings[n] if (n is not None and n < len(strings)) else "?"
            for n in funcs["name"]
        ]
        frame_func = thr["frameTable"]["func"]
        stacks = thr["stackTable"]
        stack_frame = stacks["frame"]
        stack_prefix = stacks["prefix"]

        for sidx in thr["samples"]["stack"]:
            if sidx is None:
                continue
            leaf = func_name[frame_func[stack_frame[sidx]]]
            self_time[(tname, leaf)] += 1
            seen = set()
            cur = sidx
            while cur is not None:
                fn = func_name[frame_func[stack_frame[cur]]]
                if fn not in seen:
                    total_time[(tname, fn)] += 1
                    seen.add(fn)
                cur = stack_prefix[cur]

    total_samples = sum(self_time.values()) or 1
    print(
        f"interval={interval}ms  total_samples={total_samples}  "
        f"approx_wall={total_samples * interval:.0f}ms\n"
    )
    print("=== TOP SELF-TIME (leaf) — what the CPU actually executed ===")
    for (tname, fn), c in self_time.most_common(top_n):
        pct = 100.0 * c / total_samples
        print(f"{pct:5.1f}%  {c * interval:7.0f}ms  [{tname}] {fn[:110]}")
    print("\n=== TOP INCLUSIVE (anywhere in stack) — cost per subsystem ===")
    for (tname, fn), c in total_time.most_common(top_n):
        pct = 100.0 * c / total_samples
        print(f"{pct:5.1f}%  {c * interval:7.0f}ms  [{tname}] {fn[:110]}")


if __name__ == "__main__":
    main()
