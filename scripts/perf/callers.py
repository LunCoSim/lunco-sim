#!/usr/bin/env python3
"""Find OUR functions that sit above the hot openusd/sdf allocation leaves.

The flat self-time table (symbolicate_samply.py) shows the cost lands inside
`openusd`'s HashMap inserts — but that's a library leaf. This walks each
sample's stack downward from the leaf, finds the first frame whose resolved
symbol mentions one of our crate prefixes, and tallies that as the "blamed
caller". Run after a capture to learn which lunco_* system drives the cost.

Usage: scripts/perf/callers.py <profile.json.gz> [--skip-start SECONDS]
"""
import sys
import json
import gzip
import collections
import subprocess
import shutil

OURS = ("lunco_", "sandbox", "process_usd", "sync_modelica", "sync_script",
        "cosim", "apply_usd", "usd_op", "ApplyUsd")
SKIP_LEAF_LIBS = ("libc.so", "libpthread", "ld-linux")


def load(path):
    op = gzip.open if path.endswith(".gz") else open
    with op(path, "rt") as f:
        return json.load(f)


def symbolizer():
    for c in ("llvm-symbolizer", "llvm-symbolizer-18", "addr2line"):
        e = shutil.which(c)
        if e:
            return (e, "llvm" if "llvm" in c else "addr2line")
    sys.exit("no symbolizer")


def resolve_all(sym, binpath, addrs):
    exe, mode = sym
    addrs = list(addrs)
    if mode == "llvm":
        stdin = "".join(f"{hex(a)}\n" for a in addrs)
        out = subprocess.run([exe, "--obj", binpath, "-f", "-C",
                              "--output-style=LLVM"], input=stdin,
                             capture_output=True, text=True, timeout=600).stdout
        res = {}
        for a, block in zip(addrs, out.split("\n\n")):
            lines = [l for l in block.splitlines() if l.strip()]
            res[a] = lines[0].strip() if lines else "??"
        return res
    out = subprocess.run([exe, "-f", "-C", "-e", binpath]
                         + [hex(a) for a in addrs], capture_output=True,
                         text=True, timeout=600).stdout.splitlines()
    return {a: (out[2 * i].strip() if 2 * i < len(out) else "??")
            for i, a in enumerate(addrs)}


def main():
    path = sys.argv[1]
    skip = 0.0
    if "--skip-start" in sys.argv:
        i = sys.argv.index("--skip-start")
        skip = float(sys.argv[i + 1]) * 1000.0
    prof = load(path)
    libs = prof["libs"]
    sym = symbolizer()

    # Collect all (lib,addr) we may need to resolve, lazily.
    need = collections.defaultdict(set)  # lib_idx -> {addr}
    samples_walk = []  # (thread, list-of-(lib_idx,addr) leaf->root)

    for thr in prof["threads"]:
        funcs = thr["funcTable"]["resource"]
        res_lib = thr["resourceTable"]["lib"]
        fr = thr["frameTable"]
        f_addr, f_func = fr["address"], fr["func"]
        st = thr["stackTable"]
        st_frame, st_prefix = st["frame"], st["prefix"]
        samples = thr["samples"]
        deltas = samples.get("timeDeltas") or []
        clock = 0.0
        for si, sidx in enumerate(samples["stack"]):
            if si < len(deltas):
                clock += deltas[si]
            if sidx is None or clock < skip:
                continue
            chain = []
            s = sidx
            while s is not None:
                frame = st_frame[s]
                fn = f_func[frame]
                r = funcs[fn]
                lib = res_lib[r] if (r is not None and 0 <= r < len(res_lib)) else -1
                a = f_addr[frame]
                chain.append((lib, a))
                if lib >= 0:
                    need[lib].add(a)
                s = st_prefix[s]
            samples_walk.append(chain)

    resolved = {}
    for lib_idx, addrs in need.items():
        if lib_idx < 0 or lib_idx >= len(libs):
            continue
        lib = libs[lib_idx]
        name = lib.get("name", "?")
        binpath = lib.get("debugPath") or lib.get("path")
        if any(p in name for p in SKIP_LEAF_LIBS) or not binpath:
            for a in addrs:
                resolved[(lib_idx, a)] = None
            continue
        rmap = resolve_all(sym, binpath, addrs)
        for a in addrs:
            resolved[(lib_idx, a)] = rmap.get(a)

    blame = collections.Counter()
    total = 0
    for chain in samples_walk:
        total += 1
        for (lib, a) in chain:  # leaf -> root
            name = resolved.get((lib, a))
            if name and any(o in name for o in OURS):
                blame[name] += 1
                break
    total = total or 1
    print(f"samples considered = {total}")
    print("=== TOP blamed OUR-code callers above the hot leaves ===")
    for name, c in blame.most_common(25):
        print(f"{100.0 * c / total:5.1f}%  {c:7d}  {name[:300]}")


if __name__ == "__main__":
    main()
