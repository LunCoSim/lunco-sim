#!/usr/bin/env python3
"""Drive authored UsdShade terminal verdicts through the production command clock."""
from __future__ import annotations

import argparse
import sys
import time
from pathlib import Path

from runtime import BINARY, ROOT, POLL_INTERVAL_S, ProductionSession
from run_editor_scene_test import wait_for_scene, wait_for_scene_time_selection
from test_native_trim_precision import execute, run, apply_fixture


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port", type=int, default=4225)
    parser.add_argument("--timeout", type=float, default=90)
    parser.add_argument("--log", type=Path, default=Path("/tmp/luncosim-native-material-terminals.log"))
    args = parser.parse_args()
    source = (ROOT / "assets/scenarios/tests/native_material_terminals.rhai").read_text()
    scene = ROOT / "assets/scenes/tests/twin_session_retirement/replacement/replacement.usda"
    try:
        with ProductionSession(args.port, extra_args=("--no-throttle", "--scene", str(scene)),
                               log_path=args.log, windowed=True) as session:
            pid = session.process.pid
            if sys.platform.startswith("linux") and (
                Path(f"/proc/{pid}/cwd").resolve() != ROOT
                or Path(f"/proc/{pid}/exe").resolve() != BINARY.resolve()
            ):
                raise RuntimeError("session does not match this checkout and binary")
            print(f"OWNED pid={pid} port={args.port} cwd={ROOT}", flush=True)
            wait_for_scene(session, str(scene), args.timeout)
            wait_for_scene_time_selection(session, args.log, args.timeout)
            documents = execute(session, "ListOpenDocuments")["open_documents"]
            doc = next(int(item["doc_id"]) for item in documents if item.get("active") and item["kind"] == "usd")
            for prepare, verify, count in [
                ("material_prepare", "material_valid", 3),
                ("material_edit", "material_edited", 3),
                ("material_cycle", "material_invalid", 5),
            ]:
                if prepare:
                    apply_fixture(session, source, prepare, doc)
                deadline = time.monotonic() + args.timeout
                while time.monotonic() < deadline:
                    output = run(session, source + f"\n{verify}({doc});")
                    if "MATERIAL_PENDING" not in output:
                        break
                    time.sleep(POLL_INTERVAL_S)
                else:
                    raise RuntimeError("material projection did not settle")
                print(output, flush=True)
                if f"TESTS_OK {count}" not in output or "TESTS_FAIL" in output:
                    raise RuntimeError("authored material verdict failed")
    except Exception as error:
        print(f"FAIL — {error}", file=sys.stderr)
        return 1
    print("PASS — eleven authored checks; API Exit and port release verified")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
