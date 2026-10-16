#!/usr/bin/env python3
"""Capture resource-layout regressions in an owned headful production session.

The four colored image corners, title bar and text-size notice require directed
visual review. A successful capture is not itself a layout verdict.
"""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import struct
import time
import zlib

from runtime import BINARY, ROOT, ProductionSession, assert_render_log_clean, port_is_open
from test_asset_resources import execute


def image_fixture(path, width, height):
    """Write a deterministic RGB PNG with a distinct marker at each corner."""
    colors = [(255, 32, 64), (32, 255, 64), (32, 64, 255), (255, 255, 32)]
    rows = bytearray()
    for y in range(height):
        rows.append(0)  # PNG scanline without filtering.
        for x in range(width):
            corner = (int(x >= width - 90) + 2 * int(y >= height - 90))
            marked = (x < 90 or x >= width - 90) and (y < 90 or y >= height - 90)
            rows.extend(colors[corner] if marked else (204, 204, 204))

    def chunk(kind, data):
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))

    path.write_bytes(b"\x89PNG\r\n\x1a\n"
                     + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0))
                     + chunk(b"IDAT", zlib.compress(rows)) + chunk(b"IEND", b""))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--compact", action="store_true", help="exercise the 960x540 production window profile")
    args = parser.parse_args()
    port = int(os.environ.get("LUNCOSIM_API_PORT", "4286"))
    output = Path(os.environ.get("LUNCOSIM_PREVIEW_TEST_OUTPUT", ROOT / ".lunco/resource-preview-layout/current")).resolve()
    output.mkdir(parents=True, exist_ok=True)
    fixtures = output / "fixtures"
    fixtures.mkdir(exist_ok=True)
    for name, size in [("tall", (360, 1440)), ("wide", (1440, 360)), ("square", (360, 360))]:
        image_fixture(fixtures / f"{name}.png", *size)
    long_path = fixtures / ("long path " + "x" * 90) / ("nested " + "y" * 90) / ("resource " + "z" * 90 + ".png")
    long_path.parent.mkdir(parents=True, exist_ok=True)
    long_path.write_bytes((fixtures / "tall.png").read_bytes())
    (fixtures / "text.txt").write_text("Resource preview text\n" + "".join(
        f"Line {index}: " + "long line " * 150 + "\n" for index in range(150)) + "END OF PREVIEW\n")
    os.environ.update(LUNCOSIM_CONFIG=str(output / "config"),
                      LUNCOSIM_EPHEMERAL_SETTINGS="1", LUNCOSIM_ISOLATED_RUN="1")
    log = output / "production.log"
    captures = []
    extra = ["--render-profile=fast"] if args.compact else []
    with ProductionSession(port, windowed=True, extra_args=extra, log_path=log) as session:
        pid = session.process.pid
        if Path("/proc").is_dir():
            assert Path(f"/proc/{pid}/cwd").resolve() == ROOT
            assert Path(f"/proc/{pid}/exe").resolve() == BINARY.resolve()
        (output / "session.json").write_text(json.dumps({"pid": pid, "port": port,
            "cwd": str(ROOT), "binary": str(BINARY.resolve()), "args": extra}, indent=2))

        def capture(name):
            session.capture_screenshot(output / f"{name}.png")
            captures.append(name)
            print(f"captured {name}", flush=True)

        for name, source in [("missing", fixtures / "missing.png"), ("tall", fixtures / "tall.png"),
                             ("wide", fixtures / "wide.png"), ("long-uri", long_path),
                             ("square", fixtures / "square.png"), ("text", fixtures / "text.txt"),
                             ("image-after-text", fixtures / "tall.png")]:
            execute(session, "OpenAssetResource", uri=source.as_uri())
            time.sleep(3)
            capture(name)
            if name == "long-uri":
                width, height = struct.unpack(">II", (output / f"{name}.png").read_bytes()[16:24])
                execute(session, "InjectWindowInput", event={"PointerMove": {"x": width / 2, "y": height / 2 - 160}})
                time.sleep(2.25)
                capture("uri-tooltip")
                execute(session, "InjectWindowInput", event={"PointerButton": {"button": "Secondary", "state": "Pressed", "x": width / 2, "y": height / 2 - 160}})
                time.sleep(.25)
                execute(session, "InjectWindowInput", event={"PointerButton": {"button": "Secondary", "state": "Released", "x": width / 2, "y": height / 2 - 160}})
                time.sleep(1)
                capture("uri-copy-menu")
                execute(session, "InjectWindowInput", event={"Key": {"key": "Escape", "state": "Pressed", "repeat": False, "text": None}})
                time.sleep(.25)
                execute(session, "InjectWindowInput", event={"Key": {"key": "Escape", "state": "Released", "repeat": False, "text": None}})
                execute(session, "InjectWindowInput", event={"PointerMove": {"x": 50.0, "y": 100.0}})
                execute(session, "OpenAssetResource", uri=source.as_uri())
                time.sleep(1)
                capture("uri-reloaded")
            if name == "text":
                execute(session, "InjectWindowInput", event={"PointerMove": {"x": 500.0, "y": 360.0}})
                time.sleep(.25)
                execute(session, "InjectWindowInput", event={"Scroll": {"x": 0.0, "y": -100.0}})
                time.sleep(3)
                capture("text-scrolled")
                execute(session, "OpenAssetResource", uri=source.as_uri())
                time.sleep(3)
                capture("text-reloaded")
        assert_render_log_clean(log)
    assert not port_is_open(port)
    (output / "validation.json").write_text(json.dumps({"port": port, "exit": 0,
        "port_released": True, "captures": captures, "visual_review": "required"}, indent=2))
    print("preview capture pass completed; inspect all views", flush=True)


if __name__ == "__main__":
    main()
