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
            print("PREVIEWS", json.dumps(state), flush=True)
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


def measure(session, label, seconds, samples):
    print("WINDOW_BEGIN", label, time.monotonic(), flush=True)
    deadline = time.monotonic() + seconds
    last = None
    values = []
    while time.monotonic() < deadline:
        properties = execute(session, "ReadExposures", {"surface": "engine-health"})[
            "surfaces"]["engine-health"]["properties"]
        if properties["engine_revision"] != last:
            samples.append({"label": label, "t": time.monotonic(), **properties})
            values.append(float(properties["frame_time_raw_ms"]))
            last = properties["engine_revision"]
        time.sleep(0.03)
    values.sort()
    print("WINDOW_END", label, time.monotonic(), "samples", len(values),
          "p50", statistics.median(values), "p99", values[int(.99 * (len(values) - 1))],
          "max", max(values), flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port", type=int, required=True)
    parser.add_argument("--scene", type=Path, required=True)
    parser.add_argument("--file", type=Path, action="append", required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--warmup-seconds", type=float, default=40)
    parser.add_argument("--window-seconds", type=float, default=6)
    args = parser.parse_args()
    if not all(math.isfinite(value) for value in (args.warmup_seconds, args.window_seconds)) \
            or args.warmup_seconds < 0 or args.window_seconds <= 0:
        parser.error("warmup must be nonnegative and measurement duration positive and finite")
    args.output.parent.mkdir(parents=True, exist_ok=True)
    os.environ.update(LUNCOSIM_EPHEMERAL_SETTINGS="1", LUNCOSIM_ISOLATED_RUN="1",
                      LUNCOSIM_CONFIG=str(args.output) + ".config")
    samples = []
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
            wait_for_ready(args.port)
            execute(session, "ActivatePerspective", {"id": "editor"})
            print("PHYSICS_BEFORE", json.dumps(execute(session, "PhysicsPerformance")), flush=True)
            measure(session, "editor_no_preview", args.window_seconds, samples)
            for path in args.file:
                execute(session, "OpenFile", {"path": str(path.resolve())})
                state = wait_preview(session, path)
                time.sleep(2)
                measure(session, str(path), args.window_seconds, samples)
            execute(session, "SetUsdPreviewViewMode", {"view": state["focused_view"], "mode": "text"})
            wait_text(session, state["focused_preview"], "authored")
            measure(session, "all_previews_authored_text", args.window_seconds, samples)
            execute(session, "SetUsdPreviewTextLayer", {"view": state["focused_view"], "layer": "composed"})
            wait_text(session, state["focused_preview"], "composed")
            measure(session, "all_previews_composed_text", args.window_seconds, samples)
            execute(session, "CaptureScreenshot", {"save_to_file": True,
                    "path": str(Path(str(args.output) + ".png").resolve())})
            print("FINAL_VIEWPORT", json.dumps(execute(session, "InspectUsdViewport")), flush=True)
            print("PHYSICS_AFTER", json.dumps(execute(session, "PhysicsPerformance")), flush=True)
    finally:
        Path(str(args.output) + ".samples.json").write_text(json.dumps(samples))


if __name__ == "__main__":
    main()
