#!/usr/bin/env python3
"""Windowed production gate for camera views, effects and world-fixed sky."""
from __future__ import annotations

import argparse
import statistics
import time
from pathlib import Path

from PIL import Image, ImageChops
from runtime import ROOT, ProductionSession, assert_render_log_clean, wait_for_authored_verdict
from run_editor_scene_test import wait_for_scene, wait_for_scene_time_selection


def execute(session, command, **params):
    result = session.post({"type": "ExecuteCommand", "command": command, "params": params})
    if result.get("error"):
        raise RuntimeError(f"{command}: {result}")
    return result["data"]


def await_value(read, predicate, description, timeout=60):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        value = read()
        if predicate(value):
            return value
        time.sleep(.1)
    raise RuntimeError(f"Timed out: {description}; last={value}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port", type=int, required=True)
    parser.add_argument("--output", type=Path, default=ROOT / "target/camera-views")
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    log = output / "runtime.log"
    with ProductionSession(args.port, windowed=True, log_path=log, extra_args=["--no-throttle", "--render-quality", "balanced"]) as session:
        execute(session, "OpenFile", path=str(ROOT / "assets/scenes/tests/sun_disc_occlusion.usda"))
        docs = await_value(lambda: execute(session, "ListOpenDocuments")["open_documents"],
                           lambda ds: any(d["title"] == "sun_disc_occlusion" for d in ds), "fixture document")
        doc = next(d["doc_id"] for d in docs if d["title"] == "sun_disc_occlusion")
        wait_for_scene(session, str(ROOT / "assets/scenes/tests/sun_disc_occlusion.usda"), 60)
        wait_for_scene_time_selection(session, log, 60)
        execute(session, "SetTimeTransport", playing=True)
        def identities():
            value = execute(session, "RunRhai", code='print("ids="+find_path("/World/Camera")+","+find_path("/World/ProbeCamera")+","+find_path("/World/Occluder"));')["stdout"]
            return [int(s) for s in value.strip().split("ids=")[-1].split(",")]
        camera, probe, occluder = await_value(identities, lambda ids: all(i > 0 for i in ids), "projected cameras")
        await_value(lambda: execute(session, "GetReadiness"),
                    lambda s: s["ready"] and not s["world_hold"], "scene admission")
        offset = log.stat().st_size
        execute(session, "RunScenario", target=camera,
                source=(ROOT / "assets/scenarios/tests/camera_feed_live.rhai").read_text(),
                params={"other_camera": probe, "non_camera": occluder})
        wait_for_authored_verdict(session, title="LIVE CAMERA VIEWS", check_count=29, offset=offset)
        feeds = []
        for source in (camera, probe):
            feed = execute(session, "CreateCameraFeed")["feed"]
            execute(session, "SetCameraFeed", feed=feed, source=source, label="Camera", width=640, height=360)
            feeds.append(feed)
        def capture(feed, name):
            await_value(lambda: execute(session, "InspectCameraFeeds")["views"],
                        lambda views: any(v["feed"] == feed and v["ready"] for v in views), "presented image")
            path = output / f"{name}.png"
            started = time.time_ns()
            execute(session, "CaptureCameraFeed", feed=feed, path=str(path), request=name)
            await_value(lambda: path.exists() and path.stat().st_mtime_ns >= started, bool, "saved camera image")
            return Image.open(path).convert("RGB")
        def settings(feed, **values):
            execute(session, "SetCameraImageEffects", feed=feed, settings=values)
        def sun(image):
            points = [(x, y) for y in range(image.height) for x in range(image.width)
                      if (lambda p: p[0] > 130 and p[1] > 150 and p[2] < 130)(image.getpixel((x, y)))]
            assert len(points) > 10, "Sun must be visible"
            return tuple(statistics.mean(p[i] for p in points) for i in (0, 1))
        baseline = capture(feeds[0], "sky-fixed-before")
        before = sun(baseline)
        settings(feeds[0], mode="Depth", near_m=.1, far_m=10.0)
        red = capture(feeds[0], "depth-near-red")
        r, g, b = red.getpixel((160, 180))
        assert r > g * 1.5 and r > b * 2, (r, g, b)
        assert max(red.getpixel((500, 80))) < 3, "Sky has no surface depth"
        settings(feeds[0], mode="Depth", near_m=.1, far_m=2.1)
        green = capture(feeds[0], "depth-far-green")
        r, g, b = green.getpixel((160, 180))
        assert g > r * 1.5 and g > b * 1.3, (r, g, b)
        settings(feeds[0], mode="Color", read_noise=.15, seed=4294967295, animated=False)
        noisy = capture(feeds[0], "noise-fixed")
        again = capture(feeds[0], "noise-fixed-again")
        assert ImageChops.difference(noisy, again).getbbox() is None, "Fixed noise must be repeatable"
        values = [noisy.getpixel((x, y))[2] for x in range(80, 240) for y in range(100, 260)]
        assert statistics.pstdev(values) > 5, "Read noise must affect pixels"
        settings(feeds[0])
        clean = capture(feeds[0], "noise-cleared")
        assert ImageChops.difference(noisy, clean).getbbox(), "Noise can be disabled live"
        execute(session, "SetUserCamera", name="/World/ProbeCamera")
        execute(session, "ApplyUsdTransientOps", doc_id=doc, parent_gen=None, label="Tilt second camera", ops=[
            {"SetAttribute": {"edit_target": "@view@", "path": "/World/ProbeCamera",
                              "name": "xformOp:rotateXYZ", "type_name": "double3", "value": "(10.0, 0.0, 0.0)"}}])
        # Wait for the moved camera's rendered view, not the command acknowledgement.
        await_value(lambda: execute(session, "RunRhai", code=f'print("forward="+vec3_array(world_forward3({probe})));')["stdout"],
                    lambda text: "0.173648" in text, "tilted camera projection")
        fixed = capture(feeds[0], "sky-fixed-after")
        moved = capture(feeds[1], "sky-tilted-camera")
        after = sun(fixed)
        tilted = sun(moved)
        assert max(abs(a-b) for a, b in zip(before, after)) < 1, (before, after)
        assert tilted[1] > after[1] + 40, (after, tilted)
        session.capture_screenshot(output / "camera-windows.png")
        assert_render_log_clean(log)
        print("PASS: 29 authored checks; depth red/green; fixed noise and live disable; independent world-fixed sky")
    print("PASS: owned API Exit and port release")


if __name__ == "__main__":
    main()
