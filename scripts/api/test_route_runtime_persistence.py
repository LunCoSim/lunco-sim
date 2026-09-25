#!/usr/bin/env python3
"""Verify route runtime-layer persistence across two production Twin opens."""

from __future__ import annotations

import os
import shutil
import sys
import time
import tempfile
from pathlib import Path

from runtime import ProductionSession, ROOT


SCENE_REL = Path("sim/scenes/route_runtime_persistence.usda")
RUNTIME_REL = Path(".lunco/runtime") / SCENE_REL
SCENE_SOURCE = ROOT / "assets/scenes/tests/route_runtime_persistence.usda"
TIMEOUT_S = float(os.environ.get("ROUTE_RUNTIME_TIMEOUT", "120"))
PORT = int(os.environ.get("ROUTE_RUNTIME_API_PORT", "4720"))


def output_since(path: Path, offset: int) -> str:
    try:
        with path.open("rb") as stream:
            stream.seek(offset)
            return stream.read().decode("utf-8", errors="replace")
    except OSError:
        return ""


def wait_for_verdict(path: Path, offset: int, expected_marker: str) -> str:
    deadline = time.monotonic() + TIMEOUT_S
    while time.monotonic() < deadline:
        output = output_since(path, offset)
        if "TESTS_FAIL" in output:
            raise RuntimeError(f"authored scene test failed:\n{output}")
        if "TESTS_OK" in output:
            if expected_marker not in output:
                raise RuntimeError(
                    f"scene passed without {expected_marker!r}; output was:\n{output}"
                )
            return output
        time.sleep(0.25)
    raise RuntimeError(f"no authored scene verdict within {TIMEOUT_S:g}s; log={path}")


def wait_for_runtime_file(path: Path) -> str:
    deadline = time.monotonic() + TIMEOUT_S
    while time.monotonic() < deadline:
        try:
            source = path.read_text(encoding="utf-8")
        except OSError:
            source = ""
        if '"P0"' in source:
            return source
        time.sleep(0.25)
    raise RuntimeError(f"runtime route point was not saved to {path}")


def open_twin_and_wait(twin_root: Path, log_path: Path, expected_marker: str) -> str:
    with ProductionSession(PORT, log_path=log_path) as session:
        offset = log_path.stat().st_size
        response = session.post(
            {
                "type": "ExecuteCommand",
                "command": "OpenTwin",
                "params": {"path": str(twin_root)},
            }
        )
        if response.get("error"):
            raise RuntimeError(f"OpenTwin failed: {response}")
        output = wait_for_verdict(log_path, offset, expected_marker)
    return output


def main() -> int:
    if not SCENE_SOURCE.is_file():
        raise RuntimeError(f"missing authored scene fixture {SCENE_SOURCE}")
    test_root = ROOT / "target/scene-tests"
    test_root.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="route-runtime-twin-", dir=test_root) as temporary:
        twin_root = Path(temporary)
        scene_path = twin_root / SCENE_REL
        scene_path.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(SCENE_SOURCE, scene_path)
        (twin_root / "twin.toml").write_text(
            'name = "RouteRuntimePersistenceGate"\n'
            'version = "0.1.0"\n'
            "\n[usd]\n"
            'default_scene = "sim/scenes/route_runtime_persistence.usda"\n'
            'scenes = ["sim/scenes/*.usda"]\n'
            "\n[settings]\n"
            '"usd.runtime_persistence" = true\n',
            encoding="utf-8",
        )
        original_scene = scene_path.read_bytes()
        runtime_path = twin_root / RUNTIME_REL
        log_root = test_root

        # The normal scene-test runner isolates runtime persistence. This
        # targeted production boundary test enables it only for its temporary
        # manifest-backed Twin.
        os.environ.pop("LUNCOSIM_ISOLATED_RUN", None)
        os.environ.setdefault("LUNCOSIM_EPHEMERAL_SETTINGS", "1")
        cold_log = log_root / "route_runtime_persistence.cold.log"
        cold_output = open_twin_and_wait(
            twin_root, cold_log, "ROUTE_RUNTIME_GATE: ABSENT_AT_START"
        )
        runtime_source = wait_for_runtime_file(runtime_path)
        if scene_path.read_bytes() != original_scene:
            raise RuntimeError("route authoring changed the source scene instead of its sidecar")
        if "primvars:displayColor" in runtime_source or "RouteRibbon" in runtime_source:
            raise RuntimeError("disposable route presentation leaked into the runtime sidecar")

        warm_log = log_root / "route_runtime_persistence.warm.log"
        warm_output = open_twin_and_wait(
            twin_root, warm_log, "ROUTE_RUNTIME_GATE: RESTORED_AT_START"
        )
        if scene_path.read_bytes() != original_scene:
            raise RuntimeError("opening and restoring the Twin changed its source scene")
        if '"P0"' not in runtime_source:
            raise RuntimeError("saved runtime layer does not contain the route point")

    print("PASS — route point saved to the Twin runtime sidecar and restored before remount")
    print(f"    cold: {cold_log}")
    print(f"    warm: {warm_log}")
    if "TESTS_OK" not in cold_output or "TESTS_OK" not in warm_output:
        return 1
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except Exception as error:
        print(f"FAIL — {error}", file=sys.stderr)
        raise SystemExit(1) from error
