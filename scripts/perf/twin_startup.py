#!/usr/bin/env python3
"""Measure an owned production Twin load and a source-preserved settled window."""

import argparse
import gzip
import hashlib
import json
import math
import os
from pathlib import Path
import statistics
import subprocess
import sys
import time

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "api"))
from runtime import (BINARY, ROOT, ProductionSession, assert_render_log_clean,
                     port_is_open, wait_for_ready)
from run_editor_scene_test import wait_for_scene, wait_for_scene_time_selection


def source_snapshot(root):
    names = subprocess.check_output(
        ["git", "-C", str(root), "ls-files", "-co", "--exclude-standard", "-z"]
    ).decode().split("\0")
    suffixes = {".mo", ".rhai", ".usd", ".usda", ".usdc", ".toml", ".sysml",
                ".kerml", ".wesl", ".gltf", ".glb", ".obj", ".png", ".jpg", ".ktx2",
                ".json", ".html", ".css"}
    return {name: hashlib.sha256((root / name).read_bytes()).hexdigest()
            for name in sorted(set(names))
            if name and (root / name).is_file() and Path(name).suffix.lower() in suffixes}


def command(session, name, params=None):
    reply = session.post({"type": "ExecuteCommand", "command": name, "params": params or {}})
    if reply.get("error"):
        raise RuntimeError(f"{name}: {reply['error']}")
    return reply["data"]


def workloads():
    return subprocess.check_output(["ps", "-eo", "pid,pcpu,args", "--sort=-pcpu"], text=True).splitlines()[:12]


def set_owned_pointer_passthrough(pid, window_id):
    """Set only the owned X11 window's input shape; preserve its visible shape."""
    from Xlib import X
    from Xlib.display import Display
    from Xlib.ext import shape

    display = Display()
    try:
        window = display.create_resource_object("window", window_id)
        owner = window.get_full_property(display.intern_atom("_NET_WM_PID"), 0)
        assert owner is not None and list(owner.value) == [pid], "native window owner changed"
        assert window.get_attributes().map_state == X.IsViewable, "owned window is not viewable"
        if window.shape_get_rectangles(shape.SK.Input).rectangles:
            window.shape_rectangles(shape.SO.Set, shape.SK.Input, X.Unsorted, 0, 0, [])
            display.sync()
        assert not window.shape_get_rectangles(shape.SK.Input).rectangles, "pointer input shape was not cleared"
    finally:
        display.close()


def readiness_probe(twin_name, body_path):
    """Qualify one World snapshot without adding inter-query frame delays."""
    return f"""
let readiness = query("GetReadiness");
let status = "pending";
if readiness.faulted {{
    status = "faulted";
}} else if readiness.ready && !readiness.world_hold {{
    let twin = query("ReadActiveTwinContract");
    if twin.name == {json.dumps(twin_name)} {{
        let body = find_path({json.dumps(body_path)});
        if body >= 0 {{
            let prim = query("QueryUsdPrim", #{{path: {json.dumps(body_path)}}});
            if prim.spawned {{
                let physics = query("QueryPhysicsState", #{{id: body}});
                if physics.simulation_object_paused {{
                    status = "paused " + body;
                }} else if physics.physics_state_ready {{
                    status = "ready " + body;
                }}
            }}
        }}
    }}
}}
print(status);
"""


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port", type=int, required=True)
    parser.add_argument("--twin", type=Path, required=True)
    parser.add_argument("--twin-name", required=True)
    parser.add_argument("--scene", type=Path,
                        help="explicit Twin-relative scene, opened through OpenFile")
    parser.add_argument("--source-root", type=Path, required=True)
    parser.add_argument("--body", required=True)
    parser.add_argument("--handoff-property")
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--symbols", action="store_true")
    parser.add_argument("--tracy-port", type=int)
    parser.add_argument("--timeout", type=float, default=180)
    parser.add_argument("--max-load-seconds", type=float,
                        help="require scene-open-to-physics readiness strictly below this limit")
    parser.add_argument("--window-size", help="require the active camera target size, e.g. 2560x1440")
    parser.add_argument("--pointer-passthrough", action="store_true",
                        help="make the owned X11 window click-through; requires --window-size")
    parser.add_argument("--min-fps", type=float,
                        help="require mean FPS and 95 percent of frame times to meet this budget")
    args = parser.parse_args()
    if args.pointer_passthrough and args.window_size is None:
        parser.error("--pointer-passthrough requires --window-size")
    if args.max_load_seconds is not None and (
            not math.isfinite(args.max_load_seconds) or args.max_load_seconds <= 0):
        parser.error("--max-load-seconds must be finite and greater than zero")
    if args.min_fps is not None and (not math.isfinite(args.min_fps) or args.min_fps <= 0):
        parser.error("--min-fps must be finite and greater than zero")
    if args.min_fps is not None and args.tracy_port is not None:
        parser.error("FPS acceptance requires a separate run without Tracy")
    window_size = None
    if args.window_size is not None:
        try:
            window_size = [int(value) for value in args.window_size.split("x")]
            assert len(window_size) == 2 and all(value > 0 for value in window_size)
        except (ValueError, AssertionError):
            parser.error("--window-size must contain two positive pixel dimensions, e.g. 2560x1440")
        if not sys.platform.startswith("linux") or not os.environ.get("DISPLAY"):
            parser.error("--window-size currently requires Linux with X11/XWayland and python3-xlib")
        from Xlib.display import Display
        os.environ.pop("WAYLAND_DISPLAY", None)
        os.environ["WINIT_UNIX_BACKEND"] = "x11"
    if not os.environ.get("LUNCOSIM_BIN"):
        parser.error("set LUNCOSIM_BIN to the production executable")
    if not BINARY.is_file():
        parser.error(f"production executable is missing: {BINARY}")
    scene = None
    if args.scene is not None:
        scene = (args.twin.resolve() / args.scene).resolve()
        if not scene.is_relative_to(args.twin.resolve()) or not scene.is_file():
            parser.error("--scene must name an existing file inside the Twin")
    open_command = "OpenFile" if scene is not None else "OpenTwin"
    open_path = scene if scene is not None else args.twin.resolve()
    output = args.output.resolve()
    ignored = subprocess.run(["git", "check-ignore", "-q", str(output)], cwd=ROOT)
    if ignored.returncode:
        parser.error("the output directory must be ignored by Git")
    output.mkdir(parents=True, exist_ok=True)
    source_root = args.source_root.resolve()
    os.environ.update(LUNCOSIM_EPHEMERAL_SETTINGS="1", LUNCOSIM_ISOLATED_RUN="1",
                      LUNCOSIM_CONFIG=str(output / "config"))
    for flag in ("LUNCO_DIAGNOSTIC_CPU_LIGHT_CLUSTERING", "LUNCO_DIAGNOSTIC_SINGLE_LIGHT_CLUSTER"):
        if os.environ.get(flag):
            parser.error(f"product measurements cannot use {flag}")
    receipt = {
        "binary_sha256": hashlib.sha256(BINARY.read_bytes()).hexdigest(),
        "driver_sha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
        "application_head": subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip(),
        "application_diff_sha256": hashlib.sha256(subprocess.check_output(["git", "diff"])).hexdigest(),
        "model_head": subprocess.check_output(["git", "-C", str(source_root), "rev-parse", "HEAD"], text=True).strip(),
        "source_before": source_snapshot(source_root), "workloads": workloads(),
        "port": args.port, "tracy_port": args.tracy_port, "twin": str(args.twin.resolve()),
        "physical_root": args.body, "samples": [], "readiness_events": [],
        "open_command": open_command, "open_path": str(open_path),
        "max_load_seconds": args.max_load_seconds,
        "required_camera_size": window_size, "min_fps": args.min_fps,
        "pointer_passthrough": args.pointer_passthrough,
    }
    frames = {}
    started = time.monotonic()
    try:
        bootstrap = ROOT / "assets/scenes/tests/twin_session_retirement/replacement/replacement.usda"
        extra_args = ["--scene", str(bootstrap), "--no-throttle", "--no-vsync", "--render-quality", "high"]
        with ProductionSession(args.port, windowed=True, extra_args=extra_args,
                               log_path=output / "runtime.log") as session:
            pid = session.process.pid
            receipt["pid"] = pid
            assert Path(f"/proc/{pid}/cwd").resolve() == ROOT
            assert Path(f"/proc/{pid}/exe").resolve() == BINARY.resolve()
            if window_size is not None:
                display = Display()
                try:
                    clients = display.screen().root.get_full_property(display.intern_atom("_NET_CLIENT_LIST"), 0)
                    assert clients is not None, "X11 window manager did not publish its client inventory"
                    windows = []
                    for identity in clients.value:
                        window = display.create_resource_object("window", int(identity))
                        owner = window.get_full_property(display.intern_atom("_NET_WM_PID"), 0)
                        if owner is not None and list(owner.value) == [pid]:
                            windows.append(window)
                    assert len(windows) == 1, f"expected one owned native window: {windows}"
                    windows[0].configure(width=window_size[0], height=window_size[1])
                    display.sync()
                    receipt["native_window"] = windows[0].id
                finally:
                    display.close()
            print("OWNED", pid, args.port, flush=True)
            if args.pointer_passthrough:
                set_owned_pointer_passthrough(pid, receipt["native_window"])
            if args.tracy_port:
                sockets = subprocess.check_output(["ss", "-tnp"], text=True)
                owned = [line for line in sockets.splitlines() if f"pid={pid}," in line]
                assert any(f":{args.tracy_port} " in line for line in owned), "owned Tracy connection absent"
                receipt["profiler_sockets"] = sockets
            if args.symbols:
                (output / "process-maps.txt").write_text(Path(f"/proc/{pid}/maps").read_text())
                (output / "binary-elf.txt").write_text(subprocess.check_output(["readelf", "-hl", str(BINARY)], text=True))
                with gzip.open(output / "binary-symbols.txt.gz", "wt", compresslevel=1) as symbols:
                    index = subprocess.Popen(["nm", "--numeric-sort", "--defined-only", str(BINARY)], stdout=subprocess.PIPE, text=True)
                    for line in index.stdout:
                        fields = line.split(maxsplit=2)
                        if len(fields) == 3 and fields[1] in ("t", "T"):
                            symbols.write(line)
                    assert index.wait() == 0
            wait_for_scene(session, str(bootstrap), args.timeout)
            wait_for_scene_time_selection(session, output / "runtime.log", args.timeout)
            wait_for_ready(session.port, args.timeout)
            bootstrap_prim = command(session, "QueryUsdPrim", {"path": "/World/TestLight"})
            if not bootstrap_prim.get("spawned"):
                raise RuntimeError("bootstrap readiness did not admit its scene marker")
            receipt["bootstrap_qualification"] = {
                "prim": bootstrap_prim,
                "readiness": command(session, "GetReadiness"),
                "twin": command(session, "ReadActiveTwinContract"),
            }
            receipt["bootstrap_ready_s"] = time.monotonic() - started
            probe = readiness_probe(args.twin_name, args.body)
            assert command(session, "RunRhai", {"code": probe})["stdout"].strip() == "pending", \
                "bootstrap incorrectly qualified as the target Twin"
            receipt["readiness_probe"] = "atomic-world-v1"
            receipt["load_start_frame"] = command(session, "EnginePerformance")["frame_sample_count"]
            opened = time.monotonic()
            receipt["open_response"] = command(session, open_command, {"path": str(open_path)})
            last_readiness = None
            clear_since = measurement = None
            initial_frame = initial_tick = measured_body = None
            while time.monotonic() - opened < args.timeout:
                if args.pointer_passthrough:
                    set_owned_pointer_passthrough(pid, receipt["native_window"])
                if "first_scene_ready_s" not in receipt:
                    status = command(session, "RunRhai", {"code": probe})["stdout"].strip()
                    if status == "pending":
                        time.sleep(0.05)
                        continue
                    if status == "faulted":
                        raise RuntimeError(f"runtime fault: {command(session, 'GetReadiness')}")
                    if status.startswith("paused "):
                        body_state = command(session, "QueryPhysicsState", {"id": int(status.split()[1])})
                        raise RuntimeError(f"coupled object paused: {body_state}")
                    if not status.startswith("ready "):
                        raise RuntimeError(f"invalid readiness probe result: {status!r}")
                    receipt["first_scene_ready_s"] = time.monotonic() - opened
                    receipt["first_ready_body"] = int(status.split()[1])
                    print("SCENE_READY", receipt["first_scene_ready_s"], flush=True)
                    receipt["first_ready_workloads"] = workloads()
                    receipt["first_ready_engine"] = command(session, "EnginePerformance")
                    session.capture_screenshot(output / "first-ready.png")
                    receipt["first_ready_capture_s"] = time.monotonic() - opened
                now = time.monotonic() - opened
                ready = command(session, "GetReadiness")
                qualification = {"readiness": ready}
                state = ready.get("ready"), ready.get("world_hold"), ready.get("faulted")
                if state != last_readiness:
                    receipt["readiness_events"].append({"seconds": now, "readiness": ready})
                    last_readiness = state
                if ready.get("faulted"):
                    raise RuntimeError(f"runtime fault: {ready}")
                eligible = False
                if ready.get("ready") and not ready.get("world_hold"):
                    contract = command(session, "ReadActiveTwinContract")
                    qualification["twin_name"] = contract.get("name")
                    if contract.get("name") == args.twin_name:
                        try:
                            prim = command(session, "QueryUsdPrim", {"path": args.body})
                            if prim.get("spawned"):
                                found = command(session, "RunRhai", {"code": f"print(find_path({json.dumps(args.body)}));"})
                                body = int(found["stdout"].strip())
                                body_state = command(session, "QueryPhysicsState", {"id": body})
                                qualification.update(body=body, body_state=body_state)
                                if body_state.get("simulation_object_paused"):
                                    raise RuntimeError(f"coupled object paused: {body_state.get('simulation_object_pause_reason')}")
                                if body_state.get("physics_state_ready"):
                                    if body != receipt["first_ready_body"]:
                                        raise RuntimeError("qualified body lease changed after first readiness")
                                    physics = command(session, "PhysicsPerformance")
                                    terrain = command(session, "TerrainLodStatus")
                                    qualification["terrain"] = terrain
                                    stream, derived = terrain["stream"], terrain["derived"]
                                    eligible = (physics["step_number"] > 0 and stream["wanted"] == stream["resident"]
                                                and not stream["pending"] and not derived["active"]
                                                and not derived["pending"] and derived["ready"] == derived["total"])
                                    if eligible and args.handoff_property:
                                        signal = command(session, "RunRhai", {"code": f"print(get(find_path({json.dumps(args.body)}),{json.dumps(args.handoff_property)}));"})
                                        qualification["handoff"] = float(signal["stdout"].strip())
                                        eligible = qualification["handoff"] >= 0.5
                        except RuntimeError as error:
                            # Separate requests can straddle initial scene publication.
                            pending = ("no USD stage loaded", "not found in the requested document or live stage",
                                       "no completed Avian steps", "projection is not current",
                                       "exactly one mounted live stage is required")
                            if measurement is not None or not any(text in str(error) for text in pending):
                                raise
                if not eligible:
                    if measurement is not None:
                        receipt["qualification_loss"] = qualification
                        raise RuntimeError("measured scene lost readiness")
                    clear_since = None
                else:
                    if clear_since is None:
                        clear_since = now
                    if measurement is None and now - clear_since >= 5:
                        receipt.update(usable_s=clear_since, qualified_at_s=now,
                                       source_admitted=source_snapshot(source_root), qualified_workloads=workloads())
                        assert receipt["source_before"] == receipt["source_admitted"], "source changed before measurement"
                        measurement = time.monotonic()
                        measured_body = body
                        initial_frame = command(session, "EnginePerformance")["frame_sample_count"]
                        initial_tick = command(session, "SimulationTimingProfile")["fixed_tick"]["ticks_total"]
                        print("QUALIFIED", clear_since, flush=True)
                    if measurement is not None:
                        assert body == measured_body, "measured body lease changed"
                        camera = command(session, "SceneCameraAudit")
                        if window_size is not None:
                            active = [row for row in camera["candidates"] if row.get("camera_active")]
                            assert len(active) == 1, "expected exactly one active scene camera"
                            assert active[0]["physical_target_size"] == window_size, active
                        timing = command(session, "SimulationTimingProfile")
                        engine = command(session, "EnginePerformance")
                        last, history = engine["frame_sample_count"], engine["frame_time_samples_ms"]
                        frames.update({i: value for i, value in enumerate(history, last - len(history) + 1) if i > initial_frame})
                        receipt["samples"].append({"seconds": now, "PhysicsPerformance": physics,
                                                   "SimulationTimingProfile": timing, "EnginePerformance": engine,
                                                   "EngineHealthExposures": command(session, "ReadExposures", {"surface": "engine-health"}),
                                                   "camera": camera,
                                                   "qualification": qualification})
                        if time.monotonic() - measurement >= 10 and timing["fixed_tick"]["ticks_total"] - initial_tick >= 240:
                            break
                time.sleep(0.25 if measurement is None else 0.5)
            else:
                raise RuntimeError("no sustained source-preserved measurement window before timeout")
            receipt["final_cosim"] = command(session, "CosimStatus", {"include_values": False})
            receipt["final_diagnostics"] = command(session, "RuntimeDiagnostics")
            receipt["final_body_state"] = command(session, "QueryPhysicsState", {"id": body})
            assert not receipt["final_diagnostics"]["errors"], receipt["final_diagnostics"]
            session.capture_screenshot(output / "final.png")
            assert_render_log_clean(output / "runtime.log")
        receipt["app_closed"] = True
        receipt["source_after"] = source_snapshot(source_root)
        assert receipt["source_before"] == receipt["source_after"], "source changed during run"
        log = (output / "runtime.log").read_text(errors="replace")
        if args.pointer_passthrough:
            assert "name=scene.pointer," not in log, "pointer input entered the isolated measurement"
        assert not any(marker in log for marker in ("[rhai] on_tick() failed", "[rhai] on_start() failed", "[rhai] task leaf failed")), "authored script failed"
        values = sorted(frames.values())
        assert frames and len(frames) == max(frames) - initial_frame, "canonical frame history overran"
        receipt["frame_summary"] = {"count": len(values), "p50_ms": statistics.median(values),
                                    "p95_ms": values[min(len(values) - 1, int(.95 * len(values)))],
                                    "mean_fps": 1000 / statistics.mean(values), "max_ms": max(values)}
        receipt["fixed_tick_summary"] = receipt["samples"][-1]["SimulationTimingProfile"]["fixed_tick"]
        if args.min_fps is not None:
            budget = 1000 / args.min_fps
            receipt["frame_summary"]["within_budget_percent"] = 100 * sum(v <= budget for v in values) / len(values)
            receipt["fps_budget_passed"] = (receipt["frame_summary"]["mean_fps"] >= args.min_fps
                                             and receipt["frame_summary"]["p95_ms"] <= budget)
            if not receipt["fps_budget_passed"]:
                raise RuntimeError(f"FPS budget failed: {receipt['frame_summary']}; required {args.min_fps} FPS")
        if args.max_load_seconds is not None:
            receipt["load_budget_passed"] = receipt["first_scene_ready_s"] < args.max_load_seconds
            if not receipt["load_budget_passed"]:
                raise RuntimeError(
                    f"Twin load took {receipt['first_scene_ready_s']:.3f}s; "
                    f"required less than {args.max_load_seconds:.3f}s")
        print("COMPLETE", receipt["first_scene_ready_s"], receipt["frame_summary"], receipt["fixed_tick_summary"], flush=True)
    except Exception as error:
        receipt["error"] = str(error)
        raise
    finally:
        receipt["terminal_process_absent"] = "pid" in receipt and not Path(f"/proc/{receipt['pid']}").exists()
        receipt["terminal_port_released"] = not port_is_open(args.port)
        if "source_after" not in receipt:
            receipt["source_after"] = source_snapshot(source_root)
        (output / "session.json").write_text(json.dumps(receipt, indent=2) + "\n")


if __name__ == "__main__":
    main()
