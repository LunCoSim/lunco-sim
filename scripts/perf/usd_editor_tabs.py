#!/usr/bin/env python3
"""Measure settled USD editor tabs in an owned production session.

This is a diagnostic driver, not a scene-test verdict. Samples are deduplicated
by engine revision; API polling does not observe every rendered frame.
"""

from __future__ import annotations

import argparse
import json
import math
import os
import statistics
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "api"))
from runtime import BINARY, ROOT, ProductionSession, wait_for_ready
from run_editor_scene_test import wait_for_scene, wait_for_scene_time_selection


def execute(session, name, params=None):
    response = session.post({"type": "ExecuteCommand", "command": name, "params": params or {}})
    if response.get("error"):
        raise RuntimeError(f"{name}: {response}")
    return response["data"]


def wait_preview(session, path):
    deadline = time.monotonic() + 60
    while time.monotonic() < deadline:
        documents = execute(session, "ListOpenDocuments")["open_documents"]
        docs = [document["doc_id"] for document in documents
                if document["origin"]["kind"] == "file"
                and Path(document["origin"]["path"]).resolve() == path.resolve()]
        if len(docs) > 1:
            raise RuntimeError(f"Multiple documents own {path}: {docs}")
        state = execute(session, "InspectUsdViewport")
        matching = [preview for preview in state["previews"]
                    if preview["doc_id"] in docs]
        if matching and matching[0]["projection_ready"]:
            print("PREVIEW_READY", matching[0]["preview"], "count", state["preview_count"], flush=True)
            return state
        time.sleep(0.5)
    raise RuntimeError(f"Preview did not settle for {path}: {state}")


def wait_text(session, preview_id, layer):
    deadline = time.monotonic() + 60
    while time.monotonic() < deadline:
        state = execute(session, "InspectUsdViewport")
        preview = next(preview for preview in state["previews"] if preview["preview"] == preview_id)
        if preview["text_ready"] and any(view["focused"] and view["mode"] == "text"
                                         and view["text_layer"] == layer for view in preview["views"]):
            return
        time.sleep(0.5)
    raise RuntimeError(f"USD text did not settle for preview {preview_id}")


def measure(session, label, seconds, samples, windows):
    viewport = execute(session, "InspectUsdViewport")
    focused = focused_view(viewport)
    initial_physics = execute(session, "PhysicsPerformance")
    before_step = initial_physics["step_number"]
    require_ready(execute(session, "GetReadiness"), label)
    require_diagnostics(execute(session, "RuntimeDiagnostics"), label)
    twin = execute(session, "GetToolLibrary", {"name": "runtime_ui"})["active_twin"]
    checkpoints = []
    camera_audit = execute(session, "SceneCameraAudit")
    print("WINDOW_BEGIN", label, time.monotonic(), flush=True)
    deadline = time.monotonic() + seconds
    last = None
    values = []
    previous_physics = initial_physics
    next_checkpoint = time.monotonic() + 1.0

    def checkpoint():
        nonlocal previous_physics
        readiness = execute(session, "GetReadiness")
        physics = execute(session, "PhysicsPerformance")
        current_viewport = execute(session, "InspectUsdViewport")
        require_ready(readiness, label)
        diagnostics = execute(session, "RuntimeDiagnostics")
        require_diagnostics(diagnostics, label)
        current_twin = execute(session, "GetToolLibrary", {"name": "runtime_ui"})["active_twin"]
        if current_twin != twin:
            raise RuntimeError(f"{label}: active Twin changed during the window")
        require_physics_progress(previous_physics, physics, label)
        require_same_camera(focused, focused_view(current_viewport))
        if current_viewport["preview_count"] != viewport["preview_count"]:
            raise RuntimeError(f"{label}: preview population changed during the window")
        checkpoints.append({"t": time.monotonic(), "physics": physics, "readiness": readiness,
                            "runtime_diagnostics": diagnostics})
        previous_physics = physics

    while time.monotonic() < deadline:
        properties = execute(session, "ReadExposures", {"surface": "engine-health"})[
            "surfaces"]["engine-health"]["properties"]
        if properties["engine_revision"] != last:
            samples.append({"label": label, "t": time.monotonic(), **properties})
            values.append(float(properties["frame_time_raw_ms"]))
            last = properties["engine_revision"]
        if time.monotonic() >= next_checkpoint and deadline - time.monotonic() > .25:
            checkpoint()
            next_checkpoint = time.monotonic() + 1.0
        time.sleep(0.03)
    checkpoint()
    if not values or not all(math.isfinite(value) and value > 0 for value in values):
        raise RuntimeError(f"{label}: no valid rendered frame samples")
    values.sort()
    print("WINDOW_END", label, time.monotonic(), "samples", len(values),
          "p50", statistics.median(values), "p99", values[int(.99 * (len(values) - 1))],
          "max", max(values), flush=True)
    physics = previous_physics
    windows.append({"label": label, "advanced_steps": physics["step_number"] - before_step,
                    "physics": physics, "focused_view": focused,
                    "camera_audit": camera_audit,
                    "checkpoints": checkpoints,
                    "active_twin": twin,
                    "preview_count": viewport["preview_count"]})
    print("PHYSICS_WINDOW", label, "advanced_steps", physics["step_number"] - before_step,
          "bodies", physics["bodies"], "colliders", physics["colliders"], flush=True)


def focused_view(viewport):
    return next((view for preview in viewport["previews"] for view in preview["views"]
                 if view["focused"]), None)


def require_ready(readiness, label):
    if readiness["ready"] is not True or readiness["world_hold"] is not False \
            or readiness["faulted"] is not False or readiness["readiness_tracked"] is not True \
            or readiness["pending_count"] != 0:
        raise RuntimeError(f"{label}: runtime is not fully ready: {readiness}")


def require_diagnostics(diagnostics, label):
    if diagnostics["errors"] != 0:
        raise RuntimeError(f"{label}: retained runtime errors: {diagnostics['findings']}")


def require_physics_progress(before, after, label):
    if after["step_number"] <= before["step_number"]:
        raise RuntimeError(f"{label}: physics stopped between measurement checkpoints")
    keys = ("bodies", "colliders", "dynamic_bodies", "joints")
    if any(before[key] != after[key] for key in keys):
        raise RuntimeError(f"{label}: physics topology changed during the window")


def require_same_camera(reference, measured):
    if reference is None or measured is None:
        if reference != measured:
            raise RuntimeError("Focused preview changed during the window")
        return
    keys = ("view", "mode", "projection", "target", "yaw", "pitch",
            "distance", "orthographic_scale", "image_rect", "scale_factor")
    if any(reference[key] != measured[key] for key in keys):
        raise RuntimeError(f"Equal-camera comparison changed its visible camera: {reference} -> {measured}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port", type=int, required=True)
    parser.add_argument("--scene", type=Path, required=True)
    parser.add_argument("--file", type=Path, action="append", required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--warmup-seconds", type=float, default=40)
    parser.add_argument("--window-seconds", type=float, default=6)
    parser.add_argument("--compare-first", action="store_true",
                        help="Keep the first Visual view/camera visible as more files remain open")
    parser.add_argument("--paired-first", action="store_true",
                        help="Compare warmed many/one/many previews with the same first view")
    args = parser.parse_args()
    if args.paired_first and not args.compare_first:
        parser.error("--paired-first requires --compare-first")
    for path in [args.scene, *args.file]:
        if not path.is_file():
            parser.error(f"source does not exist: {path}")
    if not all(math.isfinite(value) for value in (args.warmup_seconds, args.window_seconds)) \
            or args.warmup_seconds < 0 or args.window_seconds <= 0:
        parser.error("warmup must be nonnegative and measurement duration positive and finite")
    args.output.parent.mkdir(parents=True, exist_ok=True)
    os.environ.update(LUNCOSIM_EPHEMERAL_SETTINGS="1", LUNCOSIM_ISOLATED_RUN="1",
                      LUNCOSIM_CONFIG=str(args.output) + ".config")
    samples = []
    windows = []
    try:
        with ProductionSession(args.port, windowed=True,
                               extra_args=["--scene", str(args.scene.resolve()),
                                           "--render-quality", "high", "--no-vsync", "--no-throttle"],
                               log_path=Path(str(args.output) + ".app.log")) as session:
            pid = session.process.pid
            if Path(f"/proc/{pid}/cwd").resolve() != ROOT or Path(f"/proc/{pid}/exe").resolve() != BINARY.resolve():
                raise RuntimeError("Runtime process does not match this checkout and binary")
            print("OWNED", pid, args.port, flush=True)
            time.sleep(args.warmup_seconds)
            wait_for_scene(session, str(args.scene.resolve()), 60)
            wait_for_scene_time_selection(session, Path(str(args.output) + ".app.log"), 60)
            wait_for_ready(args.port)
            for _ in range(5):
                require_ready(execute(session, "GetReadiness"), "pre-measurement soak")
                require_diagnostics(execute(session, "RuntimeDiagnostics"), "pre-measurement soak")
                time.sleep(1)
            require_ready(execute(session, "GetReadiness"), "pre-measurement soak")
            execute(session, "ActivatePerspective", {"id": "editor"})
            print("PHYSICS_BEFORE", json.dumps(execute(session, "PhysicsPerformance")), flush=True)
            measure(session, "editor_no_preview", args.window_seconds, samples, windows)
            first_view = None
            for count, path in enumerate(args.file, 1):
                execute(session, "OpenUsdSourceDocument", {"source": str(path.resolve())})
                state = wait_preview(session, path)
                if first_view is None:
                    first_view = state["focused_view"]
                    execute(session, "FrameUsdPreviewView", {"view": first_view})
                if args.compare_first:
                    execute(session, "FocusUsdPreviewView", {"view": first_view})
                time.sleep(2)
                if not args.paired_first or count in (1, len(args.file)):
                    measure(session, f"same_visual_{count}_open" if args.compare_first else str(path),
                            args.window_seconds, samples, windows)
                    if args.compare_first:
                        require_same_camera(windows[1]["focused_view"], windows[-1]["focused_view"])
            if args.paired_first:
                state = execute(session, "InspectUsdViewport")
                first = next(preview for preview in state["previews"]
                             if any(view["view"] == first_view for view in preview["views"]))
                for preview in state["previews"]:
                    if preview["preview"] != first["preview"]:
                        execute(session, "CloseUsdPreview", {"preview": preview["preview"]})
                deadline = time.monotonic() + 30
                while execute(session, "InspectUsdViewport")["preview_count"] != 1:
                    if time.monotonic() > deadline:
                        raise RuntimeError("Extra owned previews did not close")
                    time.sleep(.1)
                execute(session, "FocusUsdPreviewView", {"view": first_view})
                time.sleep(3)
                measure(session, "paired_one_warm", args.window_seconds, samples, windows)
                require_same_camera(windows[1]["focused_view"], windows[-1]["focused_view"])
                session.capture_screenshot(Path(str(args.output) + ".one.png"))
                for path in args.file[1:]:
                    documents = execute(session, "ListOpenDocuments")["open_documents"]
                    document = next(doc for doc in documents if doc["origin"]["kind"] == "file"
                                    and Path(doc["origin"]["path"]).resolve() == path.resolve())
                    previous = next(preview for preview in state["previews"]
                                    if preview["doc_id"] == document["doc_id"])
                    execute(session, "OpenUsdPreview", {
                        "preview": previous["preview"], "doc_id": document["doc_id"],
                        "edit_target": previous["edit_target"]})
                    wait_preview(session, path)
                execute(session, "FocusUsdPreviewView", {"view": first_view})
                time.sleep(3)
                measure(session, "paired_many_warm", args.window_seconds, samples, windows)
                require_same_camera(windows[1]["focused_view"], windows[-1]["focused_view"])
                session.capture_screenshot(Path(str(args.output) + ".many.png"))
            state = execute(session, "InspectUsdViewport")
            execute(session, "SetUsdPreviewViewMode", {"view": state["focused_view"], "mode": "text"})
            wait_text(session, state["focused_preview"], "authored")
            measure(session, "all_previews_authored_text", args.window_seconds, samples, windows)
            execute(session, "SetUsdPreviewTextLayer", {"view": state["focused_view"], "layer": "composed"})
            wait_text(session, state["focused_preview"], "composed")
            measure(session, "all_previews_composed_text", args.window_seconds, samples, windows)
            if args.paired_first:
                execute(session, "SetUsdPreviewViewMode", {"view": first_view, "mode": "visual"})
                time.sleep(3)
                measure(session, "all_previews_returned_visual", args.window_seconds, samples, windows)
                require_same_camera(windows[1]["focused_view"], windows[-1]["focused_view"])
            session.capture_screenshot(Path(str(args.output) + ".png"))
            print("FINAL_VIEWPORT", json.dumps(execute(session, "InspectUsdViewport")), flush=True)
            print("PHYSICS_AFTER", json.dumps(execute(session, "PhysicsPerformance")), flush=True)
    finally:
        Path(str(args.output) + ".samples.json").write_text(json.dumps(samples))
        Path(str(args.output) + ".windows.json").write_text(json.dumps(windows))


if __name__ == "__main__":
    main()
