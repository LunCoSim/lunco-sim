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


def viewport_probe(output, rect, names=("viewport-first.png", "complete-draw.png", "final.png")):
    """Measure an unobscured terrain rectangle, not merely an ECS draw count."""
    from PIL import Image, ImageStat

    probes = {}
    for name in names:
        with Image.open(output / name) as image:
            left, top, right, bottom = rect
            assert 0 <= left < right <= image.width and 0 <= top < bottom <= image.height
            region = image.convert("L").crop(rect)
            histogram = region.histogram()
            probes[name] = {"lit_fraction": sum(histogram[16:]) / sum(histogram),
                            "luminance_stddev": ImageStat.Stat(region).stddev[0]}
    return probes


def assert_viewport_probe(probes):
    final = probes["final.png"]
    assert final["lit_fraction"] >= .2 and final["luminance_stddev"] >= 2.0, "probe must be on visibly lit, unobscured terrain"
    for name, early in probes.items():
        if name == "final.png":
            continue
        assert early["lit_fraction"] >= .5 * final["lit_fraction"], (name, probes)
        assert early["luminance_stddev"] >= .1 * final["luminance_stddev"], (name, probes)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--scene", required=True)
    parser.add_argument("--port", type=int, required=True)
    parser.add_argument("--cache", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--timeout", type=float, default=300,
                        help="Deadline for final background shading, not startup latency.")
    parser.add_argument("--performance-timeout", type=float, default=45,
                        help="Deadline to collect a complete fresh frame history; not an FPS threshold.")
    parser.add_argument("--require-progressive", action="store_true",
                        help="Require root publication before visual preparation finishes (cold cache).")
    parser.add_argument("--verify-recording", action="store_true",
                        help="Verify numbered capture from the fully prepared terrain.")
    parser.add_argument("--verify-view-retirement", action="store_true", help="Close an owned camera feed and retune the remaining cover to exercise native extraction retirement.")
    parser.add_argument("--viewport-probe", nargs=4, type=int, metavar=("LEFT", "TOP", "RIGHT", "BOTTOM"),
                        help="Unobscured terrain pixel rectangle; verify the first and complete main-view publications against the final screenshot.")
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
        recording_started = False
        recording_wait_observed = False

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
            main_view = [v for v in views if v["camera"] == status["viewport_camera"]]
            complete_partition = bool(main_view) and all(
                v["presented_ready"] and v["wanted"] == v["drawn"] == v["resident"] and v["wanted"] > 0
                for v in views)
            sample = {"elapsed_s": time.monotonic() - started,
                      "views": views, "stream": status["stream"], "derived": status["derived"]}
            evidence["samples"].append(sample)
            if status["stream"]["failed"] or status["derived"]["failed"]:
                (output / "validation.json").write_text(json.dumps(evidence, indent=2))
                raise RuntimeError(f"terminal terrain preparation failure: {sample}")
            if views:
                milestone("visual_demand", sample)
            # Sample the exposed viewport before any independent-feed readback
            # can delay it. Do not condition this receipt on CPU tile residency.
            if paused and status["exposed_viewport_camera"] is not None:
                if milestone("viewport_first_publication", sample):
                    session.capture_screenshot(output / "viewport-first.png")
                    evidence["milestones"]["viewport_first_publication"]["captured_elapsed_s"] = time.monotonic() - started
            if views and any(v["drawn"] > 0 for v in views):
                if milestone("first_publication", sample):
                    session.capture_screenshot(output / "first.png")
            if main_view and all(v["focus_ready"] for v in views):
                if milestone("camera_focus_ready", sample):
                    session.capture_screenshot(output / "near.png")
            if paused and complete_partition:
                if milestone("complete_draw_partition", sample):
                    session.capture_screenshot(output / "complete-draw.png")
                    evidence["milestones"]["complete_draw_partition"]["captured_elapsed_s"] = time.monotonic() - started
            if (complete_partition and all(v["ready"] for v in views) and status["stream"]["pending"] == 0):
                milestone("geometry_ready", sample)
                rasters = status["derived"]["rasters"] or []
                preview_published = any(r["published_resolution"] is not None
                    and r["published_resolution"] < r["requested_resolution"] for r in rasters)
                if paused and preview_published and preview_cover is None:
                    preview_cover = drawn_identity()
                    milestone("interactive_preview", sample)
                    session.capture_screenshot(output / "preview.png")
                    if args.verify_recording and args.require_progressive:
                        execute("StartOfflineRecording", output_dir=str(output / "numbered"), fps=30)
                        recording_started = True
                if recording_started and status["derived"]["pending"] > 0:
                    receipt = execute("GetOfflineRecordingStatus")
                    assert not receipt["failure"], receipt
                    if not receipt["visual_preparation_ready"]:
                        assert receipt["frame_index"] == 0, receipt
                        recording_wait_observed = True

                if status["derived"]["pending"] == 0 and paused and rasters and all(r["current"] and r["published_resolution"] == r["requested_resolution"] for r in rasters):
                    milestone("complete", sample)
                    evidence["preview_cover"] = preview_cover
                    evidence["final_cover"] = drawn_identity()
                    if not recording_started:
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
        if recording_started:
            assert recording_wait_observed, "numbered capture did not observe pending terrain preparation"
            deadline = time.monotonic() + 45
            while time.monotonic() < deadline:
                recording = execute("GetOfflineRecordingStatus")
                assert not recording["failure"], recording
                if recording["frame_index"] >= 2:
                    assert recording["visual_preparation_ready"], recording
                    evidence["recording"] = recording
                    evidence["recording_wait_observed"] = True
                    break
                time.sleep(.2)
            else:
                raise RuntimeError("pending capture failed to resume")
            execute("StopOfflineRecording")
            session.capture_screenshot(output / "final.png")
        if args.viewport_probe:
            probes = viewport_probe(output, args.viewport_probe)
            evidence["viewport_probe"] = probes
            (output / "validation.json").write_text(json.dumps(evidence, indent=2))
            assert_viewport_probe(probes)
            print("main viewport startup pixel probe PASS", probes, flush=True)
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
        deadline = time.monotonic() + args.performance_timeout
        while time.monotonic() < deadline:
            time.sleep(.25)
            measured = execute("EnginePerformance")
            if measured["frame_sample_count"] - initial["frame_sample_count"] >= measured["history_capacity"]:
                break
        assert measured["frame_sample_count"] - initial["frame_sample_count"] >= measured["history_capacity"], "performance history still contains scenario observer frames"
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
        if args.verify_recording and not recording_started:
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

        if args.verify_view_retirement:
            before = execute("TerrainLodStatus")
            main_camera = before["viewport_camera"]
            expected = {(v["camera"], v["source"]) for v in before["camera_cover"] if v["camera"] == main_camera}
            assert expected, "expected a nonempty main terrain cover"
            def settled_main_only(status):
                views = status["camera_cover"]
                return (status["stream"]["pending"] == 0
                        and {(v["camera"], v["source"]) for v in views} == expected
                        and all(v["ready"] and v["presented_ready"] and v["wanted"] == v["drawn"] == v["resident"] and v["wanted"] > 0 for v in views))
            feeds = execute("InspectCameraFeeds")["views"]
            assert feeds and len(before["camera_cover"]) > 1, "expected a live independent feed"
            for feed in feeds:
                execute("RemoveCameraFeed", feed=feed["feed"])
            deadline = time.monotonic() + 30
            while time.monotonic() < deadline:
                remaining = execute("TerrainLodStatus")
                if settled_main_only(remaining):
                    break
                time.sleep(.2)
            else:
                raise RuntimeError(f"feed removal left unsettled geometry: {remaining}")
            probe = remaining["config"]["probe_resolution"]
            execute("SetTerrainRenderingQuality", probe_resolution=probe + 2 if probe <= 255 else probe - 2)
            deadline = time.monotonic() + 30
            while time.monotonic() < deadline:
                retuned = execute("TerrainLodStatus")
                if (retuned["config"]["probe_resolution"] != probe
                    and settled_main_only(retuned)):
                    break
                time.sleep(.2)
            else:
                raise RuntimeError(f"retuned cover failed to settle: {retuned}")
            session.capture_screenshot(output / "view-retirement.png")
            evidence["view_retirement"] = {"before": before["camera_cover"],
                                          "remaining": retuned["camera_cover"],
                                          "probe_resolution": retuned["config"]["probe_resolution"]}
            diagnostics = execute("RuntimeDiagnostics")
            assert diagnostics["errors"] == 0 and diagnostics["warnings"] == 0, diagnostics
            print("feed retirement and probe retune PASS", flush=True)
        if args.viewport_probe:
            names = ["final.png"]
            if args.verify_recording:
                names.extend(["numbered/frame_000000.png", "numbered/frame_000001.png"])
            if args.verify_view_retirement:
                names.append("view-retirement.png")
            probes = viewport_probe(output, args.viewport_probe, names)
            evidence["viewport_probe"].update(probes)
            assert_viewport_probe(probes)
        (output / "validation.json").write_text(json.dumps(evidence, indent=2))
        assert_render_log_clean(output / "runtime.log")
    assert not port_is_open(args.port), "owned test port was not released"
    print("PASS progressive terrain startup", flush=True)


if __name__ == "__main__":
    main()
