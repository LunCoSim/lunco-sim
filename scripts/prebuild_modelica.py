#!/usr/bin/env python3
"""Build a distributable directory of checked Modelica artifacts for one native scene."""

import argparse
import os
from pathlib import Path
import shutil
import sys
import tempfile

sys.path.insert(0, str(Path(__file__).resolve().parent / "api"))
from run_editor_scene_test import wait_for_scene
from runtime import BINARY, ROOT, ProductionSession, wait_for_ready


def publish_records(cache, output):
    # Stage on the destination filesystem so a reader can only see a complete bundle.
    with tempfile.TemporaryDirectory(dir=output.parent) as temporary:
        bundle = Path(temporary) / "bundle"
        for family in ("compiled-dae-v*", "prepared-fmi-v*"):
            records = sorted(cache.glob(f"{family}/*.bin.zst"))
            if not records:
                raise RuntimeError(f"runtime published no {family} records")
            for record in records:
                destination = bundle / record.parent.name / record.name
                destination.parent.mkdir(parents=True, exist_ok=True)
                shutil.copyfile(record, destination)
            print(f"{records[0].parent.name}  {len(records)} records")
        bundle.rename(output)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("scene", type=Path, help="existing Twin scene file")
    parser.add_argument("output", type=Path, help="new directory for the two artifact families")
    parser.add_argument("--port", type=int, required=True)
    parser.add_argument("--timeout", type=float, default=180)
    args = parser.parse_args()
    scene = args.scene.resolve(strict=True)
    output = args.output.absolute()
    if output.exists() or output.is_symlink():
        parser.error("output must be a new directory")
    output.parent.mkdir(parents=True, exist_ok=True)
    # The requested destination can be inside a mounted asset tree.
    # Keep the running simulator's scratch cache outside that tree.
    with tempfile.TemporaryDirectory() as temporary:
        staging = Path(temporary)
        cache = staging / "cache"
        cache.mkdir()
        # Use the operator's provisioned datasets while isolating all compiled records.
        assets = Path(os.environ.get("LUNCOSIM_CACHE", Path.home() / ".lunco/cache")).resolve()
        for name in ("fonts", "library", "scenarios", "terrain", "textures"):
            if (assets / name).is_dir():
                (cache / name).symlink_to(assets / name, target_is_directory=True)
        env = dict(os.environ, LUNCOSIM_CACHE=str(cache), LUNCOSIM_CONFIG=str(staging / "config"),
                   LUNCOSIM_EPHEMERAL_SETTINGS="1", LUNCOSIM_ISOLATED_RUN="1")
        env["LUNCOSIM_MODELICA_PREBUILT"] = ""
        log = staging / "runtime.log"
        try:
            with ProductionSession(args.port, extra_args=["--scene", str(scene)],
                                   log_path=log, require_readiness=False, env=env) as session:
                if sys.platform.startswith("linux"):
                    proc = Path("/proc") / str(session.process.pid)
                    if proc.joinpath("cwd").resolve() != ROOT or not proc.joinpath("exe").samefile(BINARY):
                        raise RuntimeError("prebuild session ownership mismatch")
                wait_for_scene(session, str(scene), args.timeout)
                wait_for_ready(args.port, args.timeout)
                reply = session.post({"type": "ExecuteCommand", "command": "CosimStatus",
                                      "params": {"include_values": False}})
                if not reply.get("data", {}).get("synchronization", {}).get("active_participants"):
                    raise RuntimeError(f"scene has no active co-simulation participants: {reply}")
            publish_records(cache / "modelica", output)
        except Exception:
            if log.exists():
                print(log.read_text(errors="replace")[-4000:], file=sys.stderr)
            raise
    print(output)


if __name__ == "__main__":
    main()
