#!/usr/bin/env python3
"""Drive authored UsdShade terminal verdicts through the production command clock."""
from __future__ import annotations

import argparse
import sys
import time
from pathlib import Path

from runtime import BINARY, ROOT, POLL_INTERVAL_S, ProductionSession, run_authored_scenario
from run_editor_scene_test import wait_for_scene, wait_for_scene_time_selection
from test_native_trim_precision import execute, run, apply_fixture


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--offscreen", action="store_true", help="use the offscreen renderer")
    parser.add_argument("--port", type=int, default=4225)
    parser.add_argument("--timeout", type=float, default=90)
    parser.add_argument("--log", type=Path, default=Path("/tmp/luncosim-native-material-terminals.log"))
    args = parser.parse_args()
    source = (ROOT / "assets/scenarios/tests/native_material_terminals.rhai").read_text()
    scene = ROOT / "assets/scenes/tests/twin_session_retirement/replacement/replacement.usda"
    try:
        render_args = ["--offscreen"] if args.offscreen else []
        with ProductionSession(args.port, extra_args=("--no-throttle", "--scene", str(scene), *render_args),
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
                ("material_rebind", "material_rebound", 3),
                ("material_annotate", "material_annotated", 1),
                ("material_restore", "material_valid", 3),
                ("material_switch_terminal", "material_terminal_switched", 1),
                ("material_switch_custom", "material_custom", 1),
                ("material_restore_terminal", "material_valid", 3),
                ("material_bad_input", "material_bad_input_verdict", 2),
                ("material_repair_input", "material_repaired_input", 2),
                ("material_edit", "material_edited", 3),
                ("material_cycle", "material_invalid", 5),
                ("material_repair_cycle", "material_restored_graph", 2),
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
                if prepare == "material_prepare":
                    twin = execute(session, "GetToolLibrary", name="runtime_ui")["active_twin"]
                    run_authored_scenario(
                        session, "lunco://scenarios/tests/usd_visual_completion.rhai",
                        title="USD VISUAL COMPLETION", check_count=2, owner_twin_id=twin,
                        params={"doc_id": doc, "mesh": "/World/NativeMaterialTerminal/Shape"},
                        timeout_s=args.timeout,
                    )
    except Exception as error:
        print(f"FAIL — {error}", file=sys.stderr)
        return 1
    print("PASS — authored material checks; API Exit and port release verified")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
