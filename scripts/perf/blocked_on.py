#!/usr/bin/env python3
"""What is a given thread BLOCKED ON? Resolve callers above the syscall leaf.

When a thread's top self-time is `syscall`/futex, it's waiting, not computing.
The interesting question is WHICH call site parks it: a GPU fence (wgpu
present/submit) vs a futex on a channel/executor lock vs vsync. This walks each
sample whose leaf is a park/syscall and tallies the nearest *named* caller a
few frames up.

Usage: scripts/perf/blocked_on.py <profile.json.gz> --thread <substr> [--skip-start S]
"""
import sys, json, gzip, collections, subprocess, shutil

PARK = ("syscall", "futex", "park", "poll", "ppoll", "epoll", "read", "recv",
        "wait", "ioctl", "lock", "??")

def load(p):
    op = gzip.open if p.endswith(".gz") else open
    with op(p, "rt") as f:
        return json.load(f)

def symbolizer():
    for c in ("llvm-symbolizer", "llvm-symbolizer-18"):
        e = shutil.which(c)
        if e: return e
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
    want = sys.argv[sys.argv.index("--thread") + 1]
    skip = 0.0
    if "--skip-start" in sys.argv:
        skip = float(sys.argv[sys.argv.index("--skip-start") + 1]) * 1000.0
    prof = load(path); libs = prof["libs"]; exe = symbolizer()
    need = collections.defaultdict(set)
    chains = []  # list of leaf->root [(lib,addr)]
    for thr in prof["threads"]:
        key = f"{thr.get('name','?')}/{thr.get('tid','?')}"
        if want not in key: continue
        funcs = thr["funcTable"]["resource"]; res_lib = thr["resourceTable"]["lib"]
        fr = thr["frameTable"]; f_addr, f_func = fr["address"], fr["func"]
        st = thr["stackTable"]; st_frame, st_prefix = st["frame"], st["prefix"]
        samples = thr["samples"]; deltas = samples.get("timeDeltas") or []
        clock = 0.0
        for si, sidx in enumerate(samples["stack"]):
            if si < len(deltas): clock += deltas[si]
            if sidx is None or clock < skip: continue
            chain = []; s = sidx
            while s is not None:
                frame = st_frame[s]; r = funcs[f_func[frame]]
                lib = res_lib[r] if (r is not None and 0 <= r < len(res_lib)) else -1
                a = f_addr[frame]; chain.append((lib, a))
                if lib >= 0: need[lib].add(a)
                s = st_prefix[s]
            chains.append(chain)
    resolved = {}
    for lib_idx, addrs in need.items():
        if 0 <= lib_idx < len(libs):
            lib = libs[lib_idx]; bp = lib.get("debugPath") or lib.get("path")
            for a, n in resolve(exe, bp, addrs).items():
                resolved[(lib_idx, a)] = n
    # For each sample, find the first frame (leaf->root) that is NOT a park/io
    # primitive — that's the call site that decided to block.
    blame = collections.Counter(); total = 0
    for chain in chains:
        total += 1
        picked = None
        for (lib, a) in chain:
            n = resolved.get((lib, a)) if lib >= 0 else "[native]"
            if not n: n = "??"
            if any(p in n.lower() for p in PARK):
                continue
            picked = n; break
        blame[picked or "[all-park]"] += 1
    total = total or 1
    print(f"thread '{want}' samples = {total}")
    print("=== first NON-park caller above the blocking leaf (what it's waiting for) ===")
    for n, c in blame.most_common(25):
        print(f"  {100.0*c/total:5.1f}%  {c:7d}  {str(n)[:120]}")

if __name__ == "__main__":
    main()
