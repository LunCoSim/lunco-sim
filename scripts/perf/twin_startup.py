#!/usr/bin/env python3
"""Measure an owned production Twin load and a source-preserved settled window."""

import argparse
import gzip
import hashlib
import json
import os
from pathlib import Path
import statistics
import subprocess
import sys
import time

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "api"))
from runtime import BINARY, ROOT, ProductionSession, assert_render_log_clean, port_is_open


def source_snapshot(root):
    names = subprocess.check_output(
        ["git", "-C", str(root), "ls-files", "-co", "--exclude-standard", "-z"]
    ).decode().split("\0")
    suffixes = {".mo", ".rhai", ".usd", ".usda", ".usdc", ".toml", ".sysml",
                ".kerml", ".wesl", ".gltf", ".glb", ".obj", ".png", ".jpg", ".ktx2"}
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


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port", type=int, required=True)
    parser.add_argument("--twin", type=Path, required=True)
    parser.add_argument("--twin-name", required=True)
    parser.add_argument("--source-root", type=Path, required=True)
    parser.add_argument("--body", required=True)
    parser.add_argument("--handoff-property")
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--symbols", action="store_true")
    parser.add_argument("--tracy-port", type=int)
    parser.add_argument("--timeout", type=float, default=180)
    args = parser.parse_args()
    if not os.environ.get("LUNCOSIM_BIN"):
        parser.error("set LUNCOSIM_BIN to the production executable")
    if not BINARY.is_file():
        parser.error(f"production executable is missing: {BINARY}")
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
    }
    frames = {}
    started = time.monotonic()
    try:
        bootstrap = ROOT / "assets/scenes/tests/twin_session_retirement/replacement/replacement.usda"
        with ProductionSession(args.port, windowed=True, extra_args=["--scene", str(bootstrap),
                               "--no-throttle", "--no-vsync", "--render-quality", "high"],
                               log_path=output / "runtime.log") as session:
            pid = session.process.pid
            receipt["pid"] = pid
            assert Path(f"/proc/{pid}/cwd").resolve() == ROOT
            assert Path(f"/proc/{pid}/exe").resolve() == BINARY.resolve()
            print("OWNED", pid, args.port, flush=True)
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
            receipt["bootstrap_ready_s"] = time.monotonic() - started
            opened = time.monotonic()
            receipt["open_response"] = command(session, "OpenTwin", {"path": str(args.twin.resolve())})
            last_readiness = None
            clear_since = measurement = None
            initial_frame = initial_tick = measured_body = None
            while time.monotonic() - opened < args.timeout:
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
                                    receipt.setdefault("first_scene_ready_s", time.monotonic() - opened)
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
                        timing = command(session, "SimulationTimingProfile")
                        engine = command(session, "EnginePerformance")
                        last, history = engine["frame_sample_count"], engine["frame_time_samples_ms"]
                        frames.update({i: value for i, value in enumerate(history, last - len(history) + 1) if i > initial_frame})
                        receipt["samples"].append({"seconds": now, "PhysicsPerformance": physics,
                                                   "SimulationTimingProfile": timing, "EnginePerformance": engine,
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
        assert not any(marker in log for marker in ("[rhai] on_tick() failed", "[rhai] on_start() failed", "[rhai] task leaf failed")), "authored script failed"
        values = sorted(frames.values())
        receipt["frame_summary"] = {"count": len(values), "p50_ms": statistics.median(values),
                                    "p95_ms": values[min(len(values) - 1, int(.95 * len(values)))], "max_ms": max(values)}
        receipt["fixed_tick_summary"] = receipt["samples"][-1]["SimulationTimingProfile"]["fixed_tick"]
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
