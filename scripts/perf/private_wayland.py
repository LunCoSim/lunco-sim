"""Run one Weston GL output for a native, isolated Wayland presentation measurement."""

from contextlib import contextmanager
import json
import os
from pathlib import Path
import re
import statistics
import subprocess
import tempfile
import time


@contextmanager
def private_wayland(size, output, receipt, *, trace_repaints=False):
    log_path = output / "weston.log"
    with tempfile.TemporaryDirectory(prefix="lunco-wayland-") as runtime:
        env = os.environ.copy()
        for key in ("DISPLAY", "WAYLAND_DISPLAY", "WAYLAND_SOCKET", "WINIT_UNIX_BACKEND"):
            env.pop(key, None)
        env["XDG_RUNTIME_DIR"] = runtime
        socket_name = "wayland-profile"
        args = ["weston", "--no-config", "--backend=headless", "--renderer=gl",
                "--shell=kiosk-shell.so", "--idle-time=0", f"--socket={socket_name}",
                f"--width={size[0]}", f"--height={size[1]}"]
        if trace_repaints:
            args.append("--logger-scopes=log,timeline")
        with log_path.open("w") as log:
            server = subprocess.Popen(args, env=env, stdout=log, stderr=log)
        try:
            deadline = time.monotonic() + 10
            while not (Path(runtime) / socket_name).is_socket():
                if server.poll() is not None or time.monotonic() >= deadline:
                    raise RuntimeError(f"Weston failed to open its private output; see {log_path}")
                time.sleep(0.05)
            yield {**env, "WAYLAND_DISPLAY": socket_name}
            if server.poll() is not None:
                raise RuntimeError(f"Weston exited during the measurement; see {log_path}")
            # The GL renderer can also run on the CPU. Keep the compositor's
            # device evidence alongside wgpu's independently selected adapter.
            device = renderer = None
            with log_path.open(errors="replace") as log:
                for line in log:
                    device = device or re.search(r"Using rendering device: (/dev/dri/\S+)", line)
                    renderer = renderer or re.search(r"GL renderer: (.+)", line)
                    if device and renderer:
                        break
            if not (device and renderer):
                raise RuntimeError(f"Weston did not identify a DRM device and GL renderer; see {log_path}")
            if re.search(r"llvmpipe|softpipe|swrast|software", renderer[1], re.I):
                raise RuntimeError(f"Weston selected a software renderer: {renderer[1]}")
            receipt["private_wayland"] = {"renderer": renderer[1], "device": device[1]}
        finally:
            if server.poll() is None:
                server.terminate()
            try:
                server.wait(timeout=5)
            except subprocess.TimeoutExpired:
                server.kill()
                server.wait()


def output_repaints(log_path, start_offset, end_offset):
    """Measure completed output repaints posted inside the sampled log window."""
    pending = set()
    timestamps = []
    outputs = set()
    with log_path.open("rb") as log:
        if start_offset:
            log.seek(start_offset - 1)
            if log.read(1) != b"\n":
                log.readline()
        while log.tell() < end_offset:
            line = log.readline()
            if not line or log.tell() > end_offset:
                break
            if not line.startswith(b"{") or b"core_repaint_" not in line:
                continue
            event = json.loads(line)
            kind = event.get("N")
            if kind not in ("core_repaint_posted", "core_repaint_finished"):
                continue
            output = event["wo"]
            outputs.add(output)
            if kind == "core_repaint_posted":
                if output in pending:
                    raise ValueError("Weston posted another repaint before reporting completion")
                pending.add(output)
            elif output in pending:
                pending.remove(output)
                seconds, nanos = event["vblank_monotonic"]
                timestamps.append(seconds * 1_000_000_000 + nanos)
    if len(outputs) != 1 or len(timestamps) < 2:
        raise ValueError("expected completed repaints on one private Weston output")
    intervals = [(b - a) / 1e6 for a, b in zip(timestamps, timestamps[1:])]
    if min(intervals) <= 0:
        raise ValueError("Weston repaint completion timestamps did not advance")
    return {"count": len(timestamps), "mean_hz": 1000 / statistics.mean(intervals),
            "p50_ms": statistics.median(intervals),
            "p95_ms": sorted(intervals)[int(.95 * len(intervals))],
            "log_byte_range": [start_offset, end_offset]}
