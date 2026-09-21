#!/usr/bin/env python3
"""Run one authored editor scene through a ready, windowed production app."""

from __future__ import annotations

import argparse
import re
import sys
import time
from pathlib import Path

from runtime import POLL_INTERVAL_S, ProductionSession


def tail(path: Path, lines: int = 20) -> str:
    try:
        return "\n".join(path.read_text(encoding="utf-8", errors="replace").splitlines()[-lines:])
    except OSError:
        return ""


def scene_root(scene: str) -> str:
    scene_path = Path(scene)
    if not scene_path.is_absolute():
        scene_path = ROOT / "assets" / scene_path
    source = scene_path.read_text(encoding="utf-8")
    match = re.search(r'\bdefaultPrim\s*=\s*"([^"]+)"', source)
    if match is None:
        raise RuntimeError(f"editor test scene has no defaultPrim: {scene_path}")
    return match.group(1)


def wait_for_scene(session: ProductionSession, root: str, timeout: float) -> None:
    """Require the requested fixture's USD root in the live production world."""
    deadline = time.monotonic() + timeout
    path = f"/{root}"
    last_error = "no query result"
    while time.monotonic() < deadline:
        response = session.post({
            "type": "ExecuteCommand",
            "command": "QueryUsdPrim",
            "params": {"path": path},
        })
        if response.get("error"):
            last_error = str(response["error"])
        else:
            data = response.get("data")
            if isinstance(data, dict) and data.get("path") == path:
                return
            last_error = f"unexpected QueryUsdPrim result: {data!r}"
        if session.process is None or session.process.poll() is not None:
            raise RuntimeError(f"production editor exited before mounting {path}: {last_error}")
        time.sleep(POLL_INTERVAL_S)
    raise RuntimeError(
        f"production host reported ready without mounting {path}; last query: {last_error}"
    )


def run(port: int, timeout: float, scene: str, log_path: Path) -> int:
    verdict = ""
    error = ""
    try:
        with ProductionSession(
            port,
            extra_args=("--scene", scene),
            log_path=log_path,
            windowed=True,
        ) as session:
            wait_for_scene(session, scene_root(scene), min(timeout, 45.0))
            deadline = time.monotonic() + timeout
            while time.monotonic() < deadline:
                if session.process is None or session.process.poll() is not None:
                    break
                output = tail(log_path, lines=10000)
                if "[rhai] TESTS_FAIL" in output:
                    verdict = "FAIL"
                    break
                if "[rhai] TESTS_OK" in output:
                    verdict = "PASS"
                    break
                time.sleep(POLL_INTERVAL_S)
            if not verdict:
                if time.monotonic() >= deadline:
                    error = f"no authored verdict within {timeout:g}s"
                else:
                    error = "production editor exited before an authored verdict"
    except Exception as exc:
        error = str(exc)

    if error or verdict != "PASS":
        detail = error or "authored editor scene reported TESTS_FAIL"
        print(f"FAIL — {detail}; log={log_path}", file=sys.stderr)
        for line in tail(log_path).splitlines():
            print(f"    | {line}", file=sys.stderr)
        return 1
    print(f"PASS — ready windowed production app; API Exit and port release verified; log={log_path}")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port", type=int, required=True)
    parser.add_argument("--timeout", type=float, required=True)
    parser.add_argument("--scene", required=True)
    parser.add_argument("--log", type=Path, required=True)
    args = parser.parse_args()
    if args.port < 1 or args.port > 65535:
        parser.error("--port must be a valid TCP port")
    if args.timeout <= 0:
        parser.error("--timeout must be positive")
    return run(args.port, args.timeout, args.scene, args.log)


if __name__ == "__main__":
    raise SystemExit(main())
