#!/usr/bin/env python3
"""Run one authored editor scene through a ready, windowed production app."""

from __future__ import annotations

import argparse
import re
import sys
import time
from pathlib import Path

from runtime import BINARY, POLL_INTERVAL_S, ProductionSession, ROOT


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


def wait_for_scene(session: ProductionSession, scene: str, timeout: float) -> None:
    """Require the requested fixture root in the mounted live USD stage."""
    deadline = time.monotonic() + timeout
    scene_path = Path(scene)
    if not scene_path.is_absolute():
        scene_path = ROOT / "assets" / scene_path
    expected_path = scene_path.resolve()
    root = scene_root(scene)
    path = f"/{root}"
    last_error = "no query result"
    while time.monotonic() < deadline:
        try:
            documents = session.post({
                "type": "ExecuteCommand",
                "command": "ListOpenDocuments",
                "params": {},
            })
            if documents.get("error"):
                last_error = str(documents["error"])
            else:
                open_documents = documents.get("data", {}).get("open_documents", [])
                fixture_doc_id = None
                for document in open_documents:
                    origin = document.get("origin", {})
                    if (
                        document.get("kind") != "usd"
                        or origin.get("kind") != "file"
                        or Path(origin.get("path", "")).resolve() != expected_path
                    ):
                        continue
                    fixture_doc_id = int(document["doc_id"])
                    break
                if fixture_doc_id is None:
                    last_error = f"fixture document is not open: {expected_path}"
                else:
                    response = session.post({
                        "type": "ExecuteCommand",
                        "command": "QueryUsdPrim",
                        "params": {"doc_id": fixture_doc_id, "path": path},
                    })
                    if response.get("error"):
                        last_error = str(response["error"])
                    else:
                        data = response.get("data")
                        if (
                            isinstance(data, dict)
                            and data.get("path") == path
                            and int(data.get("doc_id", -1)) == fixture_doc_id
                        ):
                            return
                        last_error = f"unexpected live QueryUsdPrim result: {data!r}"
        except (KeyError, TypeError, ValueError, RuntimeError) as error:
            last_error = str(error)
        if session.process is None or session.process.poll() is not None:
            raise RuntimeError(f"production editor exited before mounting {path}: {last_error}")
        time.sleep(POLL_INTERVAL_S)
    raise RuntimeError(
        f"production host reported ready without mounting {expected_path} at {path}; "
        f"last query: {last_error}"
    )


def wait_for_scene_time_selection(
    session: ProductionSession, log_path: Path, timeout: float
) -> None:
    """Wait until the scene owner has committed its epoch."""
    deadline = time.monotonic() + timeout
    marker = "[time] scene epoch "
    while time.monotonic() < deadline:
        if marker in tail(log_path, lines=10000):
            return
        if session.process is None or session.process.poll() is not None:
            raise RuntimeError("production editor exited before scene time selection committed")
        time.sleep(POLL_INTERVAL_S)
    raise RuntimeError(
        f"scene time selection did not commit within {timeout:g}s; log={log_path}"
    )


def run(port: int, timeout: float, scene: str, log_path: Path, scenario: str | None = None) -> int:
    verdict = ""
    error = ""
    try:
        with ProductionSession(
            port,
            extra_args=("--scene", scene),
            log_path=log_path,
            windowed=True,
        ) as session:
            pid = session.process.pid
            if sys.platform.startswith("linux") and (
                Path(f"/proc/{pid}/cwd").resolve() != ROOT
                or Path(f"/proc/{pid}/exe").resolve() != BINARY.resolve()
            ):
                raise RuntimeError("runtime process does not match this checkout and binary")
            print(f"OWNED pid={pid} port={port} cwd={ROOT}", flush=True)
            wait_for_scene(session, scene, min(timeout, 45.0))
            # Wait for the scene-time selection boundary before issuing this
            # test's transport command while asynchronous scene preparation is
            # still settling.
            wait_for_scene_time_selection(session, log_path, timeout)
            # Editor acceptance observers advance through on_tick, but a
            # freshly opened editor scene can retain a paused live transport.
            # Resume it in this isolated test process after scene time settles.
            transport = session.post({
                "type": "ExecuteCommand",
                "command": "SetTimeTransport",
                "params": {"playing": True},
            })
            if transport.get("error") or transport.get("data", {}).get("accepted") is not True:
                raise RuntimeError(f"could not start the editor test simulation: {transport!r}")
            offset = 0
            if scenario:
                offset = log_path.stat().st_size
                library = session.post({"type": "ExecuteCommand", "command": "GetToolLibrary",
                                        "params": {"name": "runtime_ui"}})
                owner = library.get("data", {}).get("active_twin")
                if owner is None:
                    raise RuntimeError("scenario fixture requires an admitted Twin owner")
                attached = session.post({"type": "ExecuteCommand", "command": "RunScenarioAsset",
                                         "params": {"source_asset": scenario, "owner_twin_id": owner}})
                if attached.get("error"):
                    raise RuntimeError(f"could not attach authored scenario: {attached!r}")
            deadline = time.monotonic() + timeout
            while time.monotonic() < deadline:
                if session.process is None or session.process.poll() is not None:
                    break
                with log_path.open(encoding="utf-8", errors="replace") as log:
                    log.seek(offset)
                    output = log.read()
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
    parser.add_argument("--scenario", help="root-qualified Rhai asset attached after the fixture settles")
    args = parser.parse_args()
    if args.port < 1 or args.port > 65535:
        parser.error("--port must be a valid TCP port")
    if args.timeout <= 0:
        parser.error("--timeout must be positive")
    return run(args.port, args.timeout, args.scene, args.log, args.scenario)


if __name__ == "__main__":
    raise SystemExit(main())
