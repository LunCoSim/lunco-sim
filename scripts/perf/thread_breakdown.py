#!/usr/bin/env python3
"""Per-thread self-time breakdown of a samply capture.

The flat table in symbolicate_samply.py mixes ALL threads — render/main,
the Modelica worker OS thread, async-compute pool threads. When the
question is "what stalls the RENDER FRAME", you must look at the main
thread alone: the worker thread's solver cost is irrelevant to frame time.

This lists each thread, its total samples, and its top self-time leaves
(symbolicated), so you can tell whether a spike lives on the main thread
(blocks the frame) or a background thread (does not).

Usage: scripts/perf/thread_breakdown.py <profile.json.gz> [--skip-start S] [--top N]
"""
import sys, json, gzip, collections, subprocess, shutil

def load(path):
    op = gzip.open if path.endswith(".gz") else open
    with op(path, "rt") as f:
        return json.load(f)

def symbolizer():
    for c in ("llvm-symbolizer", "llvm-symbolizer-18"):
        e = shutil.which(c)
        if e:
            return e
    return None

def resolve(exe, binpath, addrs):
    addrs = list(addrs)
    if not exe or not binpath:
        return {a: "??" for a in addrs}
    stdin = "".join(f"{hex(a)}\n" for a in addrs)
    out = subprocess.run([exe, "--obj", binpath, "-f", "-C", "--output-style=LLVM"],
                         input=stdin, capture_output=True, text=True, timeout=600).stdout
    res = {}
    for a, block in zip(addrs, out.split("\n\n")):
        lines = [l for l in block.splitlines() if l.strip()]
        res[a] = lines[0].strip() if lines else "??"
    return res

def main():
    path = sys.argv[1]
    skip = 0.0
    top = 15
    if "--skip-start" in sys.argv:
        skip = float(sys.argv[sys.argv.index("--skip-start") + 1]) * 1000.0
    if "--top" in sys.argv:
        top = int(sys.argv[sys.argv.index("--top") + 1])
    prof = load(path)
    libs = prof["libs"]
    exe = symbolizer()

    # thread -> Counter(leaf (lib,addr)); and per-lib addrs to resolve
    thread_leaves = {}
    thread_total = {}
    need = collections.defaultdict(set)
    for thr in prof["threads"]:
        name = thr.get("name", "?")
        tid = thr.get("tid", "?")
        key = f"{name}/{tid}"
        funcs = thr["funcTable"]["resource"]
        res_lib = thr["resourceTable"]["lib"]
        fr = thr["frameTable"]
        f_addr, f_func = fr["address"], fr["func"]
        st = thr["stackTable"]
        st_frame = st["frame"]
        samples = thr["samples"]
        deltas = samples.get("timeDeltas") or []
        clock = 0.0
        leaves = collections.Counter()
        tot = 0
        for si, sidx in enumerate(samples["stack"]):
            if si < len(deltas):
                clock += deltas[si]
            if sidx is None or clock < skip:
                continue
            frame = st_frame[sidx]  # leaf frame of this sample
            fn = f_func[frame]
            r = funcs[fn]
            lib = res_lib[r] if (r is not None and 0 <= r < len(res_lib)) else -1
            a = f_addr[frame]
            leaves[(lib, a)] += 1
            tot += 1
            if lib >= 0:
                need[lib].add(a)
        if tot:
            thread_leaves[key] = leaves
            thread_total[key] = tot

    resolved = {}
    for lib_idx, addrs in need.items():
        if 0 <= lib_idx < len(libs):
            lib = libs[lib_idx]
            binpath = lib.get("debugPath") or lib.get("path")
            rmap = resolve(exe, binpath, addrs)
            for a in addrs:
                resolved[(lib_idx, a)] = rmap.get(a, "??")

    grand = sum(thread_total.values()) or 1
    print(f"grand total samples (after skip) = {grand}")
    for key in sorted(thread_total, key=lambda k: -thread_total[k]):
        tot = thread_total[key]
        print(f"\n=== thread {key} — {tot} samples ({100.0*tot/grand:.1f}% of all) ===")
        for (lib, a), c in thread_leaves[key].most_common(top):
            name = resolved.get((lib, a), "??") if lib >= 0 else "[native]"
            print(f"  {100.0*c/tot:5.1f}%  {c:7d}  {str(name)[:110]}")

if __name__ == "__main__":
    main()
