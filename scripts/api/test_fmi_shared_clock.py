#!/usr/bin/env python3
"""Qualify external FMI 3 participants through the authored production gate."""
from __future__ import annotations

import argparse
import hashlib
import io
import os
from pathlib import Path
from urllib.request import urlopen
import zipfile

from runtime import ROOT, BINARY, ProductionSession, run_authored_scenario, wait_for_ready

REFERENCE_VERSION = "0.0.39"
REFERENCE_SHA256 = "6863d55e5818e1ca4e4614c4d4ba4047a921b4495f6336e7002874ed791f6c2a"


def reference_fmus():
    cache = ROOT / "target" / "fmi-reference"
    cache.mkdir(exist_ok=True)
    archive = cache / f"Reference-FMUs-{REFERENCE_VERSION}.zip"
    if not archive.exists():
        url = ("https://github.com/modelica/Reference-FMUs/releases/download/"
               f"v{REFERENCE_VERSION}/Reference-FMUs-{REFERENCE_VERSION}.zip")
        with urlopen(url, timeout=60) as response:
            data = response.read(16 * 1024 * 1024 + 1)
        if len(data) > 16 * 1024 * 1024:
            raise RuntimeError("Reference FMU fixture exceeds its download bound")
        if hashlib.sha256(data).hexdigest() != REFERENCE_SHA256:
            raise RuntimeError("Reference FMU fixture digest mismatch")
        archive.write_bytes(data)
    data = archive.read_bytes()
    if hashlib.sha256(data).hexdigest() != REFERENCE_SHA256:
        raise RuntimeError("cached Reference FMU fixture digest mismatch")
    paths = {}
    with zipfile.ZipFile(io.BytesIO(data)) as contents:
        for key, name in (("decay", "Dahlquist"), ("bounce", "BouncingBall")):
            path = cache / f"{name}.fmu"
            path.write_bytes(contents.read(f"3.0/{name}.fmu"))
            paths[key] = str(path.resolve())
    return paths


def execute(session, command, **params):
    result = session.post({"type": "ExecuteCommand", "command": command, "params": params})
    if result.get("error"):
        raise RuntimeError(f"{command}: {result}")
    return result.get("data", {})


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port", type=int, default=4393)
    parser.add_argument("--windowed", action="store_true")
    parser.add_argument("--negative", action="store_true")
    parser.add_argument("--invalid-parameter", action="store_true")
    args = parser.parse_args()
    fixtures = reference_fmus()
    negative = args.negative or args.invalid_parameter
    fixtures["parameter_case"] = args.invalid_parameter
    scenario = "fmi_admission_rejection" if negative else "fmi_shared_clock"
    log = ROOT / "target" / (scenario + ("-parameter" if args.invalid_parameter else "") + ("-ui" if args.windowed else "") + ".log")
    os.environ["LUNCOSIM_EPHEMERAL_SETTINGS"] = "1"
    os.environ["LUNCOSIM_ISOLATED_RUN"] = "1"
    with ProductionSession(args.port, windowed=args.windowed, log_path=log) as session:
        pid = session.process.pid
        process = Path("/proc") / str(pid)
        if (process / "cwd").resolve() != ROOT or (process / "exe").resolve() != BINARY.resolve():
            raise RuntimeError("owned process does not match this checkout/binary")
        print(f"OWNED PID={pid} port={args.port} binary={BINARY}", flush=True)
        execute(session, "LoadScene", path="lunco://scenes/tests/twin_session_retirement/replacement/replacement.usda")
        wait_for_ready(args.port)
        run_authored_scenario(
            session, f"lunco://scenarios/tests/{scenario}.rhai",
            title="FMI ADMISSION REJECTION" if negative else "FMI SHARED CLOCK",
            check_count=3 if negative else 43, params=fixtures, allow_runtime_fault=negative,
        )
        if args.windowed:
            session.capture_screenshot(ROOT / "target" / "fmi-shared-clock.png")
    print(f"API Exit completed; port {args.port} released", flush=True)


if __name__ == "__main__":
    main()
