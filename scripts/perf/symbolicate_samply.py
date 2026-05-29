#!/usr/bin/env python3
"""Symbolicate a samply ``--save-only`` capture and print hot functions.

samply's `--save-only` profiles are NOT symbolicated — frames carry only
library-relative addresses (Firefox-profiler convention: frame.address is
relative to the lib resolved via funcTable.resource → resourceTable → lib).
The interactive samply server symbolicates on demand in the browser, which is
useless headless. This script resolves the hot addresses with `addr2line`
against each lib's on-disk binary and prints a flat self-time table.

Because Bevy runs its schedule across the whole ComputeTaskPool, "the frame
loop" is spread over many threads — so we aggregate leaf (self-time) samples
across ALL threads by (lib, address), then symbolicate the top N. Frames that
land in libc/pthread/ld are almost always thread parking/futex waits (idle
pool threads), and are labelled as such so they don't masquerade as cost.

Usage:
    scripts/perf/symbolicate_samply.py <profile.json.gz> [top_n] [--skip-start SECONDS]

`--skip-start N` ignores all samples in the first N seconds of the capture
(per-thread, via cumulative timeDeltas). Use it to drop scene-load / asset-parse
noise and see ONLY steady-state per-frame cost — the number that decides FPS.

Requires: addr2line (binutils), on PATH. Resolves symbols only for libs whose
on-disk file still exists (your own binaries with debug=line-tables-only do;
stripped system libs resolve to the lib name only, which is still informative).
"""
import sys
import json
import gzip
import collections
import subprocess
import shutil

PARK_LIBS = ("libc.so", "libpthread", "ld-linux", "libstdc++")


def _find_symbolizer():
    """Prefer llvm-symbolizer — GNU addr2line takes >60s per batch on our
    multi-GB optimized+debuginfo binaries and times out, leaving every
    address unresolved. Returns (cmd_list_builder, parse_mode) or None."""
    for cand in ("llvm-symbolizer", "llvm-symbolizer-18", "llvm-symbolizer-17",
                 "llvm-symbolizer-16", "llvm-symbolizer-15"):
        exe = shutil.which(cand) or shutil.which(f"/usr/bin/{cand}")
        if exe:
            return (exe, "llvm")
    exe = shutil.which("addr2line")
    if exe:
        return (exe, "addr2line")
    return None


def _resolve(symbolizer, binpath, addrs):
    """Return {addr: (func, 'file:line')} for the given lib-relative addrs."""
    exe, mode = symbolizer
    if mode == "llvm":
        # llvm-symbolizer reads addresses on stdin; emits per address:
        #   <func>\n<file>:<line>:<col>\n  (one extra blank line as separator)
        stdin = "".join(f"{hex(a)}\n" for a in addrs)
        out = subprocess.run(
            [exe, "--obj", binpath, "-f", "-C", "--output-style=LLVM"],
            input=stdin, capture_output=True, text=True, timeout=300,
        ).stdout
        blocks = out.split("\n\n")
        result = {}
        for a, block in zip(addrs, blocks):
            lines = [l for l in block.splitlines() if l.strip()]
            fnc = lines[0].strip() if lines else "??"
            loc = lines[1].strip() if len(lines) > 1 else "??"
            result[a] = (fnc, loc)
        return result
    # GNU addr2line fallback: 2 lines per addr (func, then file:line).
    out = subprocess.run(
        [exe, "-f", "-C", "-e", binpath] + [hex(a) for a in addrs],
        capture_output=True, text=True, timeout=300,
    ).stdout.splitlines()
    result = {}
    for i, a in enumerate(addrs):
        fnc = out[2 * i].strip() if 2 * i < len(out) else "??"
        loc = out[2 * i + 1].strip() if 2 * i + 1 < len(out) else "??"
        result[a] = (fnc, loc)
    return result


def load(path):
    op = gzip.open if path.endswith(".gz") else open
    with op(path, "rt") as f:
        return json.load(f)


def main():
    if len(sys.argv) < 2:
        sys.exit(__doc__)
    path = sys.argv[1]
    skip_start = 0.0
    if "--skip-start" in sys.argv:
        i = sys.argv.index("--skip-start")
        skip_start = float(sys.argv[i + 1]) * 1000.0  # ms
        del sys.argv[i:i + 2]
    top_n = int(sys.argv[2]) if len(sys.argv) > 2 else 40
    symbolizer = _find_symbolizer()
    if symbolizer is None:
        sys.exit("no symbolizer found (install llvm or binutils)")

    prof = load(path)
    libs = prof["libs"]  # index-aligned with resourceTable .lib

    # (lib_index, address) -> leaf sample count, aggregated across all threads.
    self_count = collections.Counter()
    total = 0
    for thr in prof["threads"]:
        funcs = thr["funcTable"]
        res = thr["resourceTable"]
        frames = thr["frameTable"]
        frame_addr = frames["address"]
        frame_func = frames["func"]
        func_res = funcs["resource"]
        res_lib = res["lib"]  # resource index -> lib index (or -1)
        stacks = thr["stackTable"]
        stack_frame = stacks["frame"]
        samples = thr["samples"]
        deltas = samples.get("timeDeltas") or []
        clock = 0.0
        for si, sidx in enumerate(samples["stack"]):
            if si < len(deltas):
                clock += deltas[si]
            if sidx is None:
                continue
            if clock < skip_start:
                continue
            total += 1
            fr = stack_frame[sidx]
            fn = frame_func[fr]
            r = func_res[fn]
            lib_idx = res_lib[r] if (r is not None and r >= 0 and r < len(res_lib)) else -1
            addr = frame_addr[fr]
            self_count[(lib_idx, addr)] += 1

    total = total or 1
    # Group the top candidates by lib so addr2line runs once per binary.
    top = self_count.most_common(max(top_n * 3, 120))
    by_lib = collections.defaultdict(list)
    for (lib_idx, addr), c in top:
        by_lib[lib_idx].append(addr)

    resolved = {}  # (lib_idx, addr) -> "func at file:line"
    for lib_idx, addrs in by_lib.items():
        if lib_idx < 0 or lib_idx >= len(libs):
            continue
        lib = libs[lib_idx]
        binpath = lib.get("debugPath") or lib.get("path")
        name = lib.get("name", "?")
        if any(p in name for p in PARK_LIBS):
            for a in addrs:
                resolved[(lib_idx, a)] = f"[park/syscall in {name}]"
            continue
        try:
            res_map = _resolve(symbolizer, binpath, addrs)
            for a in addrs:
                fnc, loc = res_map.get(a, ("??", "??"))
                if fnc == "??" or not fnc:
                    resolved[(lib_idx, a)] = f"[{name} {hex(a)}]"
                else:
                    loc = loc.split("/")[-1]
                    resolved[(lib_idx, a)] = f"{fnc}  ({name} {loc})"
        except Exception as e:
            for a in addrs:
                resolved[(lib_idx, a)] = f"[{name} {hex(a)} — {e}]"

    print(f"total leaf samples (all threads) = {total}")
    print(f"=== TOP {top_n} SELF-TIME functions (addr2line-resolved) ===")
    shown = 0
    for (lib_idx, addr), c in self_count.most_common():
        if shown >= top_n:
            break
        sym = resolved.get((lib_idx, addr))
        if sym is None:
            lib = libs[lib_idx] if 0 <= lib_idx < len(libs) else {"name": "?"}
            sym = f"[{lib.get('name','?')} {hex(addr)}]"
        pct = 100.0 * c / total
        print(f"{pct:5.1f}%  {c:7d}  {sym[:120]}")
        shown += 1


if __name__ == "__main__":
    main()
