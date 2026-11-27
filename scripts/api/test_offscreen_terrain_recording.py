#!/usr/bin/env python3
"""Prove that a captured terrain view and its viewport pose owner both settle."""
from __future__ import annotations

import argparse
import json
import os
import subprocess
import time
from pathlib import Path

from PIL import Image, ImageStat
from runtime import BINARY, ROOT, ProductionSession, assert_render_log_clean, port_is_open


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--scene", required=True)
    parser.add_argument("--port", required=True, type=int)
    parser.add_argument("--cache", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--timeout", type=float, default=240)
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    numbered = output / "numbered"
    env = {**os.environ, "LUNCOSIM_CACHE": str(args.cache.resolve()),
           "LUNCOSIM_CONFIG": str(output / "config"),
           "LUNCOSIM_EPHEMERAL_SETTINGS": "1", "LUNCO_SCREENSHOT_ROOT": str(output)}
    log = output / "runtime.log"
    observations = []
    settled = None
    duplicate = None
    deadline = time.monotonic() + args.timeout
    with ProductionSession(args.port, windowed=True, require_readiness=False, env=env,
                           log_path=log, extra_args=["--scene", args.scene, "--offscreen",
                           "--record-size", "640x360", "--no-throttle", "--no-vsync",
                           "--record-offline", str(numbered), "--record-frames", "6"]) as session:
        pid = session.process.pid
        assert Path(f"/proc/{pid}/cwd").resolve() == ROOT
        assert Path(f"/proc/{pid}/exe").resolve() == BINARY.resolve()

        def request(command, **params):
            return session.post({"type": "ExecuteCommand", "command": command, "params": params})

        try:
            while time.monotonic() < deadline and session.process.poll() is None:
                if not port_is_open(args.port):
                    time.sleep(.1)
                    continue
                try:
                    recording = request("GetOfflineRecordingStatus")["data"]
                    terrain = request("TerrainLodStatus")["data"]
                except (RuntimeError, KeyError):
                    # The CLI can finish its last image and retire the transport
                    # between the two queries. Require real successful process
                    # exit; all exact frame files are verified below.
                    try:
                        if session.process.wait(timeout=2) == 0:
                            break
                    except subprocess.TimeoutExpired:
                        pass
                    raise
                covers = terrain["camera_cover"]
                observations.append({"recording": recording, "viewport_camera": terrain["viewport_camera"],
                                     "camera_cover": covers, "stream": terrain["stream"], "derived": terrain["derived"]})
                assert not recording["failure"], recording
                if recording["pending"] and duplicate is None:
                    try:
                        response = request("StartOfflineRecording", output_dir=str(output / "duplicate"), fps=30)
                    except RuntimeError as error:
                        duplicate = str(error)
                        assert "HTTP 409" in duplicate and "already owns the capture target" in duplicate, duplicate
                    else:
                        raise AssertionError(f"competing start was accepted: {response}")
                if (terrain["viewport_camera"] is not None and covers
                        and all(v["ready"] and v["presented_ready"] and 0 < v["wanted"] == v["drawn"] == v["resident"] for v in covers)
                        and terrain["stream"]["pending"] == 0 and terrain["derived"]["pending"] == 0):
                    settled = observations[-1]
                time.sleep(.1)
            assert session.process.poll() == 0, "numbered recording failed to drain on its own"
        finally:
            (output / "observations.json").write_text(json.dumps(observations, indent=2))
    assert duplicate is not None, "did not exercise competing recording request"
    assert not (output / "duplicate").exists(), "rejected start changed output destination"
    assert settled is not None, "no complete offscreen terrain cover was observed"
    frames = sorted(numbered.glob("frame_*.png"))
    assert [f.name for f in frames] == [f"frame_{i:06d}.png" for i in range(6)], frames
    pixels = []
    for frame in frames:
        with Image.open(frame) as image:
            assert image.size == (640, 360), image.size
            luminance = image.convert("L")
            stddev = ImageStat.Stat(luminance).stddev[0]
            assert stddev >= 2.0, f"{frame}: likely clear image"
            pixels.append({"name": frame.name, "luminance_stddev": stddev})
    assert_render_log_clean(log)
    (output / "validation.json").write_text(json.dumps({"settled": settled, "duplicate": duplicate, "frames": pixels}, indent=2))
    print("PASS offscreen terrain cover, competing start rejection and six numbered frames", flush=True)


if __name__ == "__main__":
    main()
