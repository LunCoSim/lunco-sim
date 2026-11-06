#!/usr/bin/env python3
"""Verify the production Windows window installs its native icon."""

import argparse
import ctypes
from ctypes import wintypes
import os
from pathlib import Path
import subprocess
import sys
import time

from runtime import BINARY, ROOT, port_is_open, request_json, wait_for_ready


def window_icon_installed(pid: int) -> bool:
    user = ctypes.WinDLL("user32", use_last_error=True)
    callback_type = ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)
    user.EnumWindows.argtypes = [callback_type, wintypes.LPARAM]
    user.GetWindowThreadProcessId.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.DWORD)]
    user.IsWindowVisible.argtypes = [wintypes.HWND]
    user.GetWindowTextLengthW.argtypes = [wintypes.HWND]
    user.SendMessageTimeoutW.argtypes = [
        wintypes.HWND, wintypes.UINT, wintypes.WPARAM, wintypes.LPARAM,
        wintypes.UINT, wintypes.UINT, ctypes.POINTER(ctypes.c_size_t),
    ]
    user.SendMessageTimeoutW.restype = wintypes.LPARAM
    found = False

    @callback_type
    def inspect(hwnd: int, _data: int) -> bool:
        nonlocal found
        owner = wintypes.DWORD()
        user.GetWindowThreadProcessId(hwnd, ctypes.byref(owner))
        if owner.value == pid and user.IsWindowVisible(hwnd) and user.GetWindowTextLengthW(hwnd):
            result = ctypes.c_size_t()
            if user.SendMessageTimeoutW(hwnd, 0x007F, 0, 0, 0x0002, 1000, ctypes.byref(result)):
                found = result.value != 0
        return not found

    user.EnumWindows(inspect, 0)
    return found


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port", type=int, required=True)
    args = parser.parse_args()
    if sys.platform != "win32" or not BINARY.is_file():
        raise SystemExit("Set LUNCOSIM_BIN to a production Windows binary")
    if port_is_open(args.port):
        raise SystemExit(f"API port {args.port} is already in use")
    log_path = ROOT / "target" / "windows-icon-acceptance.log"
    env = dict(os.environ, LUNCOSIM_EPHEMERAL_SETTINGS="1")
    env.pop("LUNCO_NO_UI", None)
    with log_path.open("wb") as log:
        process = subprocess.Popen([str(BINARY), "--api", str(args.port)], cwd=ROOT,
                                   env=env, stdout=log, stderr=subprocess.STDOUT)
        try:
            wait_for_ready(args.port)
            deadline = time.monotonic() + 30
            while time.monotonic() < deadline and not window_icon_installed(process.pid):
                if process.poll() is not None:
                    raise RuntimeError(f"application exited before installing its icon; log={log_path}")
                time.sleep(0.2)
            if not window_icon_installed(process.pid):
                raise RuntimeError(f"window icon was not installed; log={log_path}")
        finally:
            if process.poll() is None and port_is_open(args.port):
                response = request_json(args.port, {"type": "ExecuteCommand", "command": "Exit", "params": {}})
                if response.get("error"):
                    raise RuntimeError(f"Exit was rejected: {response}")
                process.wait(timeout=30)
            if process.poll() is None or port_is_open(args.port):
                raise RuntimeError(f"owned process {process.pid} or port {args.port} remains active")
    print(f"Windows window icon: PASS; pid={process.pid}; port {args.port} released")


if __name__ == "__main__":
    main()
