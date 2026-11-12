#!/usr/bin/env python3
"""Repeat Twin scene load, runtime reset and source reload in production."""

import argparse
import json
from pathlib import Path
import re
import threading
import time
import tomllib
from urllib.parse import quote

from runtime import (
    BINARY, ROOT, ProductionSession, get_json, run_authored_scenario, wait_for_ready,
)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port", type=int, default=4197)
    parser.add_argument("--twin", type=Path, default=ROOT / "assets/scenes/tests/twin_session_retirement/replacement")
    parser.add_argument("--scene", help="Twin-relative scene path; defaults to the manifest entry point")
    parser.add_argument("--root-prim", default="/World")
    parser.add_argument("--cycles", type=int, default=3,
                        help="number of reload and Twin remount cycles (default: 3)")
    args = parser.parse_args()
    if args.cycles < 1:
        parser.error("--cycles must be at least 1")
    twin_root = args.twin.resolve()
    manifest = tomllib.loads((twin_root / "twin.toml").read_text())
    name = manifest["name"]
    scene_path = (args.scene or manifest["usd"]["default_scene"]).replace("\\", "/")
    identity = "uuid-" + manifest["uuid"].replace("-", "") if manifest.get("uuid") else quote(name, safe="-._~")
    uri = "twin://" + identity + "/" + scene_path
    with ProductionSession(args.port, windowed=True,
                           log_path=ROOT / "target/twin-source-uri.log") as session:
        process = Path("/proc") / str(session.process.pid)
        if process.exists() and ((process / "cwd").resolve() != ROOT or (process / "exe").resolve() != BINARY.resolve()):
            raise RuntimeError("owned runtime does not match this checkout and binary")

        def execute(command, **params):
            result = session.post({"type": "ExecuteCommand", "command": command, "params": params})
            if result.get("error"):
                raise RuntimeError(f"{command}: {result}")
            return result.get("data", {})

        def pause_scene_clock():
            execute("SetTimeTransport", playing=False)

        application_catalog = execute("ScriptingCatalog")
        expected_catalog_sections = {
            "verbs", "hooks", "policy_hooks", "policy_status", "prelude",
            "tools", "commands", "queries", "reflection",
        }
        if not expected_catalog_sections.issubset(application_catalog):
            raise RuntimeError("application-level scripting catalog omitted its full sections")
        try:
            session.post({
                "type": "ExecuteCommand",
                "command": "ScriptingCatalog",
                "params": {"sections": ["internal_mount_authorities"]},
            })
        except RuntimeError as error:
            if "HTTP 422" not in str(error) or "unknown section" not in str(error):
                raise
        else:
            raise RuntimeError("ScriptingCatalog accepted an unknown section selection")

        def root_entity():
            try:
                prim = execute("QueryUsdPrim", path=args.root_prim, topology=True)
                return prim["topology"]["binding"]["entity"] if prim.get("spawned") else None
            except RuntimeError as error:
                if "no USD stage loaded" in str(error) or "exactly one mounted live stage" in str(error):
                    return None
                raise

        def projected(previous):
            deadline = time.monotonic() + 90
            while time.monotonic() < deadline:
                current = root_entity()
                if current is not None and current != previous:
                    await_ready_with_progress()
                    return current
                if session.process.poll() is not None:
                    raise RuntimeError(f"production process exited; log={session.log_path}")
                time.sleep(.1)
            raise RuntimeError(f"fresh projection timed out; log={session.log_path}")

        def settled_projection():
            deadline = time.monotonic() + 90
            while time.monotonic() < deadline:
                current = root_entity()
                readiness = get_json(args.port, "/api/ready")["data"]
                remember_scene_progress(readiness)
                if (current is not None and readiness.get("ready") is True and
                        readiness.get("world_hold") is False and not readiness.get("faulted")):
                    return current
                if session.process.poll() is not None:
                    raise RuntimeError(f"production process exited; log={session.log_path}")
                time.sleep(.1)
            raise RuntimeError(f"scene projection timed out; log={session.log_path}")

        def require_projection():
            current = root_entity()
            if current is None:
                raise RuntimeError(f"scene is ready without the requested root {args.root_prim}")
            return current

        scene_progress_reasons = []
        scene_load_labels = []
        scene_progress_lock = threading.Lock()
        scene_progress_stop = threading.Event()

        def remember_scene_progress(readiness):
            with scene_progress_lock:
                for reason in readiness.get("preparation", []):
                    if "scene" in reason.lower() and reason not in scene_progress_reasons:
                        scene_progress_reasons.append(reason)
                for pending in readiness.get("pending", []):
                    label = pending.get("label")
                    if pending.get("kind") == "scene_load" and isinstance(label, str) \
                            and label not in scene_load_labels:
                        scene_load_labels.append(label)

        def watch_scene_progress():
            while not scene_progress_stop.is_set():
                try:
                    remember_scene_progress(get_json(args.port, "/api/ready")["data"])
                except RuntimeError:
                    pass
                scene_progress_stop.wait(.01)

        def await_ready_with_progress():
            deadline = time.monotonic() + 90
            last_readiness = None
            while time.monotonic() < deadline:
                readiness = get_json(args.port, "/api/ready")["data"]
                last_readiness = readiness
                remember_scene_progress(readiness)
                if (readiness.get("ready") is True and readiness.get("world_hold") is False
                        and not readiness.get("faulted")):
                    return
                if session.process.poll() is not None:
                    raise RuntimeError(f"production process exited; log={session.log_path}")
                time.sleep(.1)
            raise RuntimeError(
                f"scene readiness timed out; last readiness={last_readiness}; log={session.log_path}"
            )

        def open_twin_after(previous_twin_id):
            deadline = time.monotonic() + 90
            while time.monotonic() < deadline:
                contract = execute("ReadActiveTwinContract")
                if (contract.get("open") and contract.get("name") == name and
                        contract.get("twin_id") != previous_twin_id):
                    try:
                        source_identity = execute("RunRhai", code="print(twin_source_identity());")["stdout"].strip()
                    except RuntimeError:
                        time.sleep(.1)
                        continue
                    if source_identity == identity:
                        return contract["twin_id"]
                if session.process.poll() is not None:
                    raise RuntimeError(f"production process exited; log={session.log_path}")
                time.sleep(.1)
            raise RuntimeError(f"Twin replacement did not become active; log={session.log_path}")

        def authored_verdict(mode, owner, check_count, **params):
            run_authored_scenario(
                session,
                "lunco://scenarios/tests/twin_source_uri.rhai",
                title={"source": "Twin source URI", "progress": "Twin scene progress",
                       "reject": "Twin source URI rejection"}[mode],
                check_count=check_count,
                params={"mode": mode, **params},
                owner_twin_id=owner,
            )

        def verify_active_owner(expected_twin_id):
            contract = execute("ReadActiveTwinContract")
            if not contract.get("open") or contract.get("twin_id") != expected_twin_id:
                raise RuntimeError("scene operation changed the application-owned active Twin")

        def verify_progress_labels():
            with scene_progress_lock:
                reasons = list(scene_progress_reasons)
                labels = list(scene_load_labels)
                scene_progress_reasons.clear()
                scene_load_labels.clear()
            if not reasons:
                raise RuntimeError("no scene-transition progress label was observed")
            if not labels:
                raise RuntimeError("no pending scene-readiness label was observed")
            try:
                authored_verdict(
                    "progress", owner, 3,
                    reasons=reasons, labels=labels,
                    source_prefix=f"twin://{identity}/", uri=uri,
                )
            except RuntimeError as error:
                raise RuntimeError(f"{error}; observed scene-load labels={labels!r}") from error

        def capture_loading_status():
            deadline = time.monotonic() + 15
            while time.monotonic() < deadline:
                readiness = get_json(args.port, "/api/ready")["data"]
                remember_scene_progress(readiness)
                if any("scene" in reason.lower() for reason in readiness.get("preparation", [])):
                    session.capture_screenshot(ROOT / "target/twin-source-uri-loading.png")
                    return
                time.sleep(.01)
            raise RuntimeError("no scene loading status was available to capture")

        previous = root_entity()
        active = execute("ReadActiveTwinContract")
        previous_twin_id = active.get("twin_id") if active.get("open") else None
        progress_watcher = threading.Thread(target=watch_scene_progress, daemon=True)
        progress_watcher.start()
        execute("OpenTwin", path=str(twin_root))
        owner = open_twin_after(previous_twin_id)
        execute("LoadScene", path=uri)
        pause_scene_clock()
        wait_for_ready(args.port)
        previous = settled_projection()
        verify_progress_labels()
        execute("LoadScene", path=uri)
        pause_scene_clock()
        wait_for_ready(args.port)
        previous = require_projection()
        verify_active_owner(owner)
        authored_verdict("source", owner, 7, name=name, uri=uri, identity=identity)
        run_authored_scenario(
            session,
            "lunco://scenarios/tests/twin_identity_boundary.rhai",
            title="Twin execution identity boundary",
            check_count=7,
            owner_twin_id=owner,
        )
        for cycle in range(args.cycles):
            for command in ("LoadScene", "ResetSceneRuntime", "RestartScene"):
                execute(command, **({"path": uri} if command == "LoadScene" else {}))
                pause_scene_clock()
                if command == "LoadScene":
                    wait_for_ready(args.port)
                    previous = require_projection()
                else:
                    if cycle == 0 and command == "RestartScene":
                        capture_loading_status()
                    previous = projected(previous)
                    if cycle == 0 and command == "RestartScene":
                        verify_progress_labels()
                verify_active_owner(owner)
                authored_verdict("source", owner, 7, name=name, uri=uri, identity=identity)
            menu_code = (
                'import "lunco://scripting/tools/scenarios" as scenarios; '
                f"let result = scenarios::load(#{{open_twin: true, twin_id: {owner}, "
                f"relative_path: {json.dumps(scene_path)}}}); "
                'if !result.ok { throw "scenario-menu scene admission failed: " + result.error; }'
            )
            execute("RunRhai", code=menu_code)
            pause_scene_clock()
            previous = settled_projection()
            verify_active_owner(owner)
            authored_verdict("source", owner, 7, name=name, uri=uri, identity=identity)

            previous_twin_id = owner
            execute("OpenTwin", path=str(twin_root))
            owner = open_twin_after(previous_twin_id)
            current_identity = execute("RunRhai", code="print(twin_source_identity());")["stdout"].strip()
            if current_identity != identity:
                raise RuntimeError("reopening the same Twin changed its stable source identity")
            execute("LoadScene", path=uri)
            pause_scene_clock()
            wait_for_ready(args.port)
            previous = settled_projection()
            verify_progress_labels()
            execute("LoadScene", path=uri)
            pause_scene_clock()
            wait_for_ready(args.port)
            previous = require_projection()
            verify_active_owner(owner)
            authored_verdict("source", owner, 7, name=name, uri=uri, identity=identity)
            authored_verdict("reject", owner, 3, uri=uri, relative_path=scene_path)
            print(f"Cycle {cycle + 1}: stable, reusable URI {uri}", flush=True)
        scene_progress_stop.set()
        progress_watcher.join(timeout=2)
        session.capture_screenshot(ROOT / "target/twin-source-uri.png")
    log_text = session.log_path.read_text(errors="replace")
    if re.search(r"twin://[0-9a-fA-F]{32}(?=/|\b)", log_text):
        raise RuntimeError("runtime log exposed an opaque Twin mount authority")
    print(
        f"PASS: {args.cycles} reload and Twin remount cycles; stable URI survived "
        f"{args.cycles} fresh mount leases; owned session exited and port released",
        flush=True,
    )


if __name__ == "__main__":
    main()
