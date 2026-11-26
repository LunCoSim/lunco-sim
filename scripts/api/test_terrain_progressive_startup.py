#!/usr/bin/env python3
"""Measure the native terrain publication stages on an explicitly supplied cache."""
from __future__ import annotations

import argparse
import json
import os
import statistics
import time
from pathlib import Path

from runtime import (
    BINARY, ROOT, ProductionSession, assert_render_log_clean, get_json,
    port_is_open, run_authored_scenario,
)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--scene", required=True)
    parser.add_argument("--port", type=int, required=True)
    parser.add_argument("--cache", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--timeout", type=float, default=300,
                        help="Deadline for final background shading, not startup latency.")
    parser.add_argument("--require-progressive", action="store_true",
                        help="Require root publication before visual preparation finishes (cold cache).")
    parser.add_argument("--verify-recording", action="store_true",
                        help="Verify numbered capture from the fully prepared terrain.")
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    env = {
        **os.environ, "LUNCOSIM_CACHE": str(args.cache.resolve()),
        "LUNCOSIM_CONFIG": str(output / "config"),
        "LUNCOSIM_EPHEMERAL_SETTINGS": "1", "LUNCO_SCREENSHOT_ROOT": str(output),
    }
    evidence = {"milestones": {}, "samples": []}
    started = time.monotonic()
    with ProductionSession(
        args.port, windowed=True, require_readiness=False, env=env,
        extra_args=["--scene", args.scene, "--no-throttle", "--no-vsync"],
        log_path=output / "runtime.log",
    ) as session:
        pid = session.process.pid
        assert Path(f"/proc/{pid}/cwd").resolve() == ROOT
        assert Path(f"/proc/{pid}/exe").resolve() == BINARY.resolve()

        def execute(command, **params):
            response = session.post({"type": "ExecuteCommand", "command": command, "params": params})
            if response.get("error"):
                raise RuntimeError(f"{command}: {response}")
            return response["data"]

        def milestone(name, status):
            if name not in evidence["milestones"]:
                elapsed = time.monotonic() - started
                evidence["milestones"][name] = {"elapsed_s": elapsed, "status": status}
                print(name, round(elapsed, 3), flush=True)
                return True
            return False

        deadline = time.monotonic() + args.timeout
        paused = False
        preview_cover = None

        def drawn_identity():
            status = execute("TerrainLodStatus", include_geometry=True,
                             vertex_limit=1, index_limit=1)
            return {
                "poses": sorted((v["camera"], v["source"], v["position_local_m"], v["forward_local"])
                                for v in status["camera_cover"]),
                "tiles": sorted((t["camera"], t["entity"]) for t in status["tiles"]),
            }

        while time.monotonic() < deadline:
            if session.process.poll() is not None:
                raise RuntimeError("production process exited before terrain settled")
            if not port_is_open(args.port):
                time.sleep(.1)
                continue
            world = get_json(args.port, "/api/ready")["data"]
            if world["ready"] and not paused:
                milestone("world_ready", world)
                execute("SetTimeTransport", playing=False)
                paused = True
            status = execute("TerrainLodStatus")
            views = status["camera_cover"]
            sample = {"elapsed_s": time.monotonic() - started,
                      "views": views, "stream": status["stream"], "derived": status["derived"]}
            evidence["samples"].append(sample)
            if views:
                milestone("visual_demand", sample)
            if views and any(v["drawn"] > 0 for v in views):
                if milestone("first_publication", sample):
                    session.capture_screenshot(output / "first.png")
            if paused and any(v["camera"] == status["viewport_camera"] and v["drawn"] > 0
                              for v in views):
                if milestone("viewport_first_publication", sample):
                    session.capture_screenshot(output / "viewport-first.png")
            if views and all(v["focus_ready"] for v in views):
                if milestone("camera_focus_ready", sample):
                    session.capture_screenshot(output / "near.png")
            if paused and views and all(v["presented_ready"] and v["wanted"] == v["drawn"] == v["resident"] and v["wanted"] > 0 for v in views):
                if milestone("complete_draw_partition", sample):
                    session.capture_screenshot(output / "complete-draw.png")
            if (views and all(v["ready"] and v["wanted"] == v["drawn"] == v["resident"]
                              for v in views) and status["stream"]["pending"] == 0):
                milestone("geometry_ready", sample)
                if paused and status["derived"]["pending"] > 0 and preview_cover is None:
                    preview_cover = drawn_identity()
                    milestone("interactive_preview", sample)
                    session.capture_screenshot(output / "preview.png")
                if status["derived"]["pending"] == 0 and paused:
                    milestone("complete", sample)
                    evidence["preview_cover"] = preview_cover
                    evidence["final_cover"] = drawn_identity()
                    session.capture_screenshot(output / "final.png")
                    break
            time.sleep(.15)
        else:
            (output / "validation.json").write_text(json.dumps(evidence, indent=2))
            raise RuntimeError("progressive terrain did not finish; final selection state retained in validation.json")
        # Retain evidence before any subsequent assertion can fail.
        (output / "validation.json").write_text(json.dumps(evidence, indent=2))
        if args.require_progressive:
            first = evidence["milestones"]["first_publication"]
            complete = evidence["milestones"]["complete"]
            assert first["elapsed_s"] < complete["elapsed_s"], "root publication waited for completion"
            assert first["status"]["derived"]["pending"] > 0, "geometry waited for final shading"
            assert any(v["drawn"] < v["wanted"] or not v["ready"]
                       for v in first["status"]["views"]), "root publication waited for final cover"
            viewport = evidence["milestones"]["viewport_first_publication"]
            assert viewport["status"]["derived"]["pending"] > 0, "main viewport publication waited for final shading"
            assert preview_cover is not None, "full shading finished before a distinct interactive preview"
            assert preview_cover == evidence["final_cover"], "shading upgrade changed a settled geometric cover"
        run_authored_scenario(
            session, "lunco://scenarios/tests/terrain_stationary_cover.rhai",
            title="TERRAIN STATIONARY COVER", check_count=3, params={"duration_s": 10.0},
            timeout_s=30,
        )
        diagnostics = execute("RuntimeDiagnostics")
        assert diagnostics["errors"] == 0 and diagnostics["warnings"] == 0, diagnostics
        evidence["diagnostics"] = diagnostics
        # Clear the scenario-observer workload from the frame-time history
        # before measuring the settled native viewport.
        initial = execute("EnginePerformance")
        deadline = time.monotonic() + 12.0
        while time.monotonic() < deadline:
            time.sleep(.25)
            measured = execute("EnginePerformance")
            if measured["frame_sample_count"] - initial["frame_sample_count"] >= measured["history_capacity"]:
                break
        evidence["performance"] = measured
        frame_ms = sorted(measured["frame_time_samples_ms"])
        assert frame_ms and all(value > 0 for value in frame_ms), "no measured render frames"
        evidence["settled_frame_time"] = {
            "samples": len(frame_ms),
            "median_ms": statistics.median(frame_ms),
            "p95_ms": frame_ms[min(len(frame_ms) - 1, int(.95 * len(frame_ms)))],
            "median_fps": 1000.0 / statistics.median(frame_ms),
        }
        print("settled performance", evidence["settled_frame_time"], flush=True)
        if args.verify_recording:
            execute("StartOfflineRecording", output_dir=str(output / "numbered"), fps=30)
            try:
                recording_deadline = time.monotonic() + 45.0
                while time.monotonic() < recording_deadline:
                    recording = execute("GetOfflineRecordingStatus")
                    assert not recording["failure"], recording
                    if recording["frame_index"] >= 2:
                        assert recording["visual_preparation_ready"], recording
                        evidence["recording"] = recording
                        print("numbered capture", recording["frame_index"], flush=True)
                        break
                    time.sleep(.2)
                else:
                    raise RuntimeError(f"numbered capture failed to advance: {recording}")
            finally:
                execute("StopOfflineRecording")

        (output / "validation.json").write_text(json.dumps(evidence, indent=2))
        assert_render_log_clean(output / "runtime.log")
    assert not port_is_open(args.port), "owned test port was not released"
    print("PASS progressive terrain startup", flush=True)


if __name__ == "__main__":
    main()
