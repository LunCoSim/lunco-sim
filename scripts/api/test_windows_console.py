#!/usr/bin/env python3
"""Verify Windows console startup with the production binary and owned API sessions.

Set LUNCOSIM_BIN to luncosim.exe. This process-boundary gate probes Win32
console attachment and restarts, which cannot be observed inside Rhai.
"""
from __future__ import annotations

import argparse
import ctypes
import json
import os
import subprocess
import sys
import tempfile
import time
from contextlib import contextmanager
from pathlib import Path

from runtime import BINARY, ROOT, port_is_open, request_json, wait_for_ready


def probe_console(pid: int) -> int:
    kernel = ctypes.WinDLL("kernel32", use_last_error=True)
    kernel.FreeConsole()
    if kernel.AttachConsole(pid):
        kernel.FreeConsole()
        return 0
    if ctypes.get_last_error() == 6:  # ERROR_INVALID_HANDLE: target has no console.
        return 3
    raise ctypes.WinError(ctypes.get_last_error())


def has_console(pid: int) -> bool:
    result = subprocess.run(
        [sys.executable, str(Path(__file__).resolve()), "--probe", str(pid)],
        creationflags=subprocess.DETACHED_PROCESS, capture_output=True, check=False,
    )
    if result.returncode not in (0, 3):
        raise RuntimeError(f"console probe failed: {result.stderr!r}")
    return result.returncode == 0


def command(port: int, name: str, **params: object) -> dict:
    result = request_json(port, {"type": "ExecuteCommand", "command": name, "params": params})
    if result.get("error"):
        raise RuntimeError(f"{name}: {result}")
    return result


def wait_for_preference(config: Path, expected: bool) -> None:
    deadline = time.monotonic() + 10
    while time.monotonic() < deadline:
        path = config / "settings.json"
        if path.exists():
            settings = json.loads(path.read_text(encoding="utf-8"))
            if settings.get("debug_console", {}).get("show_on_startup") is expected:
                return
        time.sleep(0.1)
    raise RuntimeError(f"console preference was not persisted as {expected}")


@contextmanager
def session(port: int, env: dict[str, str], output: Path, *args: str):
    if port_is_open(port):
        raise RuntimeError(f"port {port} is already in use")
    with output.open("wb") as log:
        process = subprocess.Popen(
            [str(BINARY), "--api", str(port), "--no-throttle", *args],
            cwd=ROOT, env=env, stdin=subprocess.DEVNULL, stdout=log,
            stderr=subprocess.STDOUT,
        )
        print(f"OWNED pid={process.pid} port={port} binary={BINARY} cwd={ROOT}", flush=True)
        try:
            wait_for_ready(port)
            yield process
        finally:
            if process.poll() is None:
                command(port, "Exit")
                process.wait(timeout=20)
            if process.returncode != 0:
                raise RuntimeError(f"process exited {process.returncode}; log={output}")
            if port_is_open(port):
                raise RuntimeError(f"owned port {port} remained open after Exit")
    if "[lunco] luncosim" not in output.read_text(encoding="utf-8", errors="replace"):
        raise RuntimeError(f"redirected startup output was lost; log={output}")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port", type=int, default=4127)
    parser.add_argument("--probe", type=int, help=argparse.SUPPRESS)
    parser.add_argument("--detached", action="store_true", help=argparse.SUPPRESS)
    args = parser.parse_args()
    if sys.platform != "win32":
        parser.error("this acceptance check requires Windows")
    if args.probe is not None:
        raise SystemExit(probe_console(args.probe))
    if not args.detached:
        # The controller must have no console: the app explicitly attaches to
        # its parent, even if CreateProcess originally detached the child.
        worker = subprocess.Popen(
            [sys.executable, str(Path(__file__).resolve()), "--port", str(args.port), "--detached"],
            creationflags=subprocess.DETACHED_PROCESS, stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True,
        )
        with worker:
            for line in worker.stdout:
                print(line, end="", flush=True)
            raise SystemExit(worker.wait())
    if not BINARY.is_file():
        parser.error(f"production binary does not exist: {BINARY}; set LUNCOSIM_BIN")
    output = ROOT / "target" / "windows-console-acceptance"
    output.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="lunco-console-config-") as directory:
        config = Path(directory)
        env = dict(os.environ, LUNCOSIM_CONFIG=directory)
        env.pop("LUNCOSIM_EPHEMERAL_SETTINGS", None)
        env.pop("LUNCO_NO_UI", None)
        with session(args.port, env, output / "default.log") as process:
            assert not has_console(process.pid), "default desktop launch opened a console"
            command(args.port, "SetDebugConsoleStartup", enabled=True)
            wait_for_preference(config, True)
            assert not has_console(process.pid), "preference changed the current console"
        with session(args.port, env, output / "enabled.log") as process:
            assert has_console(process.pid), "restart did not open the requested console"
            command(args.port, "SetDebugConsoleStartup", enabled=False)
            wait_for_preference(config, False)
            assert has_console(process.pid), "preference detached the current console"
        with session(args.port, env, output / "disabled.log") as process:
            assert not has_console(process.pid), "disabled preference did not survive restart"
        with session(args.port, env, output / "override.log", "--console") as process:
            assert has_console(process.pid), "--console did not override the preference"
            wait_for_preference(config, False)
        for flags in ([], ["--console"]):
            result = subprocess.run(
                [str(BINARY), *flags, "--help"], cwd=ROOT, env=env,
                creationflags=subprocess.DETACHED_PROCESS, capture_output=True,
                timeout=30, check=False,
            )
            assert result.returncode == 0 and b"USAGE:" in result.stdout, result
        result = subprocess.run(
            [str(BINARY), "--validate"], cwd=ROOT, env=env,
            creationflags=subprocess.DETACHED_PROCESS, capture_output=True,
            timeout=30, check=False,
        )
        assert result.returncode == 2 and b"needs at least one path" in result.stderr, result
    print(f"Windows console startup: PASS; logs={output}; owned port released")


if __name__ == "__main__":
    main()
