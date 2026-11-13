#!/usr/bin/env python3
"""Verify Twin-local View layout saving, restoration, disabling, and reset."""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import subprocess
import time
import uuid

from runtime import BINARY, ROOT, ProductionSession


SCENE = ROOT / "assets/scenes/tests/sun_disc_occlusion.usda"
AUTHORED_VIEW_TEST = ROOT / "assets/scripting/tests/test_workspace_view_layout.rhai"
RHAI_TEST_RUNNER = ROOT / "scripts/api/run_rhai_test.sh"


def execute(session, command, **params):
    response = session.post({"type": "ExecuteCommand", "command": command, "params": params})
    if response.get("error"):
        raise RuntimeError(f"{command}: {response}")
    return response.get("data", {})


def authored_verdict(session):
    environment = os.environ.copy()
    environment["LUNCOSIM_BIN"] = str(BINARY)
    result = subprocess.run(
        [str(RHAI_TEST_RUNNER), str(session.port), str(AUTHORED_VIEW_TEST)],
        cwd=ROOT,
        env=environment,
        capture_output=True,
        text=True,
        timeout=30,
    )
    if result.stdout.strip():
        print(result.stdout.strip(), flush=True)
    if result.returncode:
        raise RuntimeError(
            f"authored View layout verdict failed: {result.stdout}\n{result.stderr}"
        )


def assert_owned_session(session, port):
    if session.port != port or session.process is None:
        raise RuntimeError("session is not the process launched for this API port")
    process = Path(f"/proc/{session.process.pid}")
    if not process.is_dir() or (process / "cwd").resolve() != ROOT.resolve():
        raise RuntimeError("owned production process is not running from this checkout")
    if (process / "exe").resolve() != BINARY.resolve():
        raise RuntimeError("owned production process is not the checkout's production binary")
    print(f"Owned production PID {session.process.pid}, API port {port}, binary {BINARY}", flush=True)


def wait_for(session, read, predicate, description, timeout=90):
    deadline = time.monotonic() + timeout
    last = None
    while time.monotonic() < deadline:
        last = read()
        if predicate(last):
            return last
        if session.process is None or session.process.poll() is not None:
            raise RuntimeError(f"runtime exited waiting for {description}; log={session.log_path}")
        time.sleep(.15)
    raise RuntimeError(f"timed out waiting for {description}; last={last}; log={session.log_path}")


def workspace_twin_id(session):
    workspace = execute(session, "ReadActiveWorkspace")
    return workspace.get("scope_id") if workspace.get("open") else None


def wait_scene(session):
    def admitted():
        readiness = execute(session, "GetReadiness")
        if not readiness.get("ready") or readiness.get("world_hold") or readiness.get("faulted"):
            return False
        try:
            prim = execute(session, "QueryUsdPrim", path="/World/Camera")
        except RuntimeError as error:
            if any(
                message in str(error)
                for message in (
                    "exactly one mounted live stage is required",
                    "no USD stage loaded",
                )
            ):
                return False
            raise
        return bool(prim.get("spawned"))

    return wait_for(session, admitted, bool, "admitted scene camera")


def camera_source(session):
    output = execute(
        session,
        "RunRhai",
        code='print("camera_source=" + find_path("/World/Camera"));',
    ).get("stdout", "")
    for line in output.splitlines():
        if "camera_source=" in line:
            return int(line.rsplit("camera_source=", 1)[1].strip())
    return 0


def open_fixture_twin(session, twin_root):
    execute(session, "OpenTwin", path=str(twin_root))
    wait_for(session, lambda: workspace_twin_id(session), lambda twin: twin is not None, "active Twin")
    wait_scene(session)
    return workspace_twin_id(session)


def create_camera_view(session, source):
    feed = execute(session, "CreateCameraFeed")["feed"]
    execute(
        session,
        "SetCameraFeed",
        feed=feed,
        source=source,
        label="Persistence probe",
        width=480,
        height=270,
    )
    execute(
        session,
        "SetCameraFeedResolution",
        feed=feed,
        match_window=False,
        width=480,
        height=270,
    )
    wait_for(
        session,
        lambda: execute(session, "InspectCameraFeeds")["views"],
        lambda views: any(view["feed"] == feed and view["source"] == source and view["ready"] for view in views),
        "rendered camera feed",
    )
    return feed


def create_graph_view(session):
    execute(
        session,
        "SetGraphHud",
        visible=True,
        width=420,
        height=280,
        x=840,
        y=400,
    )
    return wait_for(
        session,
        lambda: execute(session, "InspectGraphHud")["huds"],
        lambda huds: any(hud["visible"] and hud["width"] >= 320 for hud in huds),
        "visible graph HUD",
    )


def view_sources(state):
    return [
        view["source"]
        for scene in state.get("runtime_views", [])
        for view in scene.get("camera_views", [])
    ]


def camera_snapshots(state):
    return [
        view
        for scene in state.get("runtime_views", [])
        for view in scene.get("camera_views", [])
    ]


def graph_snapshots(state):
    return [
        view
        for scene in state.get("runtime_views", [])
        for view in scene.get("graph_views", [])
    ]


def wait_saved_camera(session, state_path, source):
    return wait_for(
        session,
        lambda: json.loads(state_path.read_text()) if state_path.is_file() else None,
        lambda state: state is not None and any(
            view.get("source") == source
            and view.get("position") is not None
            and view.get("size") is not None
            for view in camera_snapshots(state)
        ) and any(
            view.get("visible")
            and view.get("position") is not None
            and view.get("width", 0) >= 320
            for view in graph_snapshots(state)
        ),
        "Twin-local camera and graph selection and window geometry",
    )


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port", type=int, required=True)
    args = parser.parse_args()

    evidence = ROOT / "target" / ("workspace-view-layout-" + uuid.uuid4().hex)
    twin_root = evidence / "twin"
    twin_root.mkdir(parents=True)
    config_root = evidence / "config"
    config_root.mkdir()
    state_path = twin_root / ".lunco/workspace.json"
    os.environ["LUNCOSIM_CONFIG"] = str(config_root)
    os.environ["LUNCOSIM_ISOLATED_RUN"] = "1"
    os.environ.pop("LUNCOSIM_EPHEMERAL_SETTINGS", None)
    print(f"Evidence: {evidence}", flush=True)

    with ProductionSession(args.port, windowed=True, log_path=evidence / "save.log") as session:
        assert_owned_session(session, args.port)
        execute(session, "OpenUsdSourceDocument", source=str(SCENE))
        document = wait_for(
            session,
            lambda: next(
                (
                    row["doc_id"]
                    for row in execute(session, "ListOpenDocuments")["open_documents"]
                    if row["title"] == "sun_disc_occlusion"
                ),
                None,
            ),
            lambda doc: doc is not None,
            "camera scene source document",
        )
        scene_path = twin_root / "camera_views.usda"
        execute(session, "SaveAsDocument", doc_id=document, path=str(scene_path))
        wait_for(session, scene_path.is_file, bool, "saved camera scene")
        execute(session, "CloseDocument", doc_id=document)
        wait_for(
            session,
            lambda: execute(session, "ListOpenDocuments")["open_documents"],
            lambda documents: not documents,
            "closed camera scene source document",
        )
        execute(
            session,
            "CreateTwin",
            path=str(twin_root),
            name="View layout persistence",
            default_scene=scene_path.name,
        )
        wait_for(
            session,
            lambda: workspace_twin_id(session),
            lambda twin: twin is not None,
            "created Twin",
        )
        wait_scene(session)
        authored_verdict(session)
        twin_id = workspace_twin_id(session)
        source = wait_for(session, lambda: camera_source(session), bool, "stable camera identity")
        execute(session, "ActivatePerspective", id="sandbox_view")
        create_camera_view(session, source)
        create_graph_view(session)
        saved = wait_saved_camera(session, state_path, source)
        if not state_path.is_relative_to(twin_root / ".lunco"):
            raise RuntimeError(f"workspace state escaped the Twin: {state_path}")
        if "sandbox_view" not in saved.get("docks", {}):
            raise RuntimeError("workspace snapshot omitted the active View dock layout")
        session.capture_screenshot(evidence / "camera-view-saved.png")
    if not state_path.is_file():
        raise RuntimeError(f"workspace state did not survive API Exit: {state_path}")

    with ProductionSession(args.port, windowed=True, log_path=evidence / "restore-reset.log") as session:
        assert_owned_session(session, args.port)
        twin_id = open_fixture_twin(session, twin_root)
        restored = wait_for(
            session,
            lambda: execute(session, "InspectCameraFeeds")["views"],
            lambda views: any(view["source"] == source for view in views),
            "restored camera feed",
        )
        if len(restored) != 1:
            raise RuntimeError(f"expected one restored camera feed, got {restored}")
        authored_verdict(session)
        session.capture_screenshot(evidence / "camera-view-restored.png")
        execute(session, "ResetTwinViewLayout", twin=twin_id)
        wait_for(
            session,
            lambda: execute(session, "InspectCameraFeeds")["views"],
            lambda views: not views,
            "reset camera views",
        )
        wait_for(
            session,
            lambda: execute(session, "InspectGraphHud")["huds"],
            lambda huds: not huds,
            "reset graph views",
        )
        authored_verdict(session)
        reset_state = wait_for(
            session,
            lambda: json.loads(state_path.read_text()) if state_path.is_file() else None,
            lambda value: value is not None
            and all(
                not scene.get("camera_views") and not scene.get("graph_views")
                for scene in value.get("runtime_views", [])
            )
            and not value.get("runtime_surface_layouts"),
            "persisted View layout reset",
        )
        if "sandbox_view" not in reset_state.get("docks", {}):
            raise RuntimeError("reset did not persist the authored View dock layout")
        session.capture_screenshot(evidence / "camera-view-reset.png")
        execute(session, "SetProjectViewStateRestoration", twin=twin_id, value=False)
        settings_path = twin_root / ".lunco/settings.json"
        wait_for(
            session,
            lambda: json.loads(settings_path.read_text()) if settings_path.is_file() else None,
            lambda value: value is not None and value.get("ui", {}).get("restore_view_state") is False,
            "View layout setting write",
        )
        create_camera_view(session, source)
        create_graph_view(session)

    state = json.loads(state_path.read_text())
    if view_sources(state) or graph_snapshots(state):
        raise RuntimeError(
            f"reset or disabled saving retained View state: cameras={view_sources(state)}, "
            f"graphs={graph_snapshots(state)}"
        )

    with ProductionSession(args.port, windowed=True, log_path=evidence / "disabled-restore.log") as session:
        assert_owned_session(session, args.port)
        open_fixture_twin(session, twin_root)
        authored_verdict(session)
        session.capture_screenshot(evidence / "camera-view-disabled.png")

    print("PASS: Twin-local camera/graph save, restart restore, reset, and disabled View layout", flush=True)
    print(f"State: {state_path}", flush=True)


if __name__ == "__main__":
    main()
