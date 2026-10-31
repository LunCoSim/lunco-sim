#!/usr/bin/env python3
"""Check real transport throughput in a normally paced desktop host."""
import argparse
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import time

from runtime import BINARY, ROOT, ProductionSession, assert_render_log_clean, run_authored_scenario


def capture_timing(session, output):
    diagnostics = {}
    for name, params in (("SimulationTimingProfile", {}),
                         ("CosimStatus", {"include_values": False}),
                         ("PhysicsPerformance", {}), ("EnginePerformance", {}),
                         ("ReadExposures", {"surface": "engine-health"}),
                         ("RuntimeDiagnostics", {}), ("TerrainLodStatus", {})):
        diagnostics[name] = session.post({"type": "ExecuteCommand", "command": name,
                                          "params": params})
    (output / "timing.json").write_text(json.dumps(diagnostics, indent=2) + "\n")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port", type=int, default=4792)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--twin", type=Path)
    parser.add_argument("--scene", default="assets/scenes/tests/modelica_runtime_contracts.usda")
    parser.add_argument("--settle-seconds", type=float, default=0.0)
    parser.add_argument("--rates", type=float, nargs="+", default=[2., 4., 8.])
    parser.add_argument("--sample-seconds", type=float, default=3.0)
    parser.add_argument("--warmup-seconds", type=float, default=3.0,
                        help="run each selected rate before measuring, without resetting fixed debt")
    parser.add_argument("--participants", type=int, default=0)
    parser.add_argument("--check-modelica-hud", action="store_true")
    parser.add_argument("--keep-focus", action="store_true",
                        help="leave desktop focus unchanged instead of testing background pacing")
    parser.add_argument("--no-vsync", action="store_true",
                        help="diagnostic presentation comparison; not normal desktop acceptance")
    parser.add_argument("--no-throttle", action="store_true",
                        help="diagnostic host cadence comparison; not normal desktop acceptance")
    args = parser.parse_args()
    if not 0 <= args.settle_seconds <= 60:
        raise ValueError("settle-seconds must be between zero and 60")
    if len(set(args.rates)) != len(args.rates):
        raise ValueError("transport rate samples must be distinct")
    if not 3 <= args.sample_seconds <= 60:
        raise ValueError("sample-seconds must be between three and 60")
    if not 0 <= args.warmup_seconds <= 10:
        raise ValueError("warmup-seconds must be between zero and ten")
    check_count = 2 + 2 * len(args.rates)
    if args.check_modelica_hud and args.participants <= 0:
        raise ValueError("the Modelica HUD gate requires participants")
    if not args.keep_focus and not shutil.which("xmessage"):
        raise RuntimeError("this native focus regression requires X11 xmessage")
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    os.environ.update(LUNCOSIM_EPHEMERAL_SETTINGS="1", LUNCOSIM_ISOLATED_RUN="1",
                      LUNCO_ASSET_ROOT=str(ROOT / "assets"),
                      LUNCOSIM_CONFIG=str(output / "config"))
    log_path = output / "app.log"
    extra_args = ("--scene", args.scene) + (("--no-vsync",) if args.no_vsync else ())
    if args.no_throttle:
        extra_args += ("--no-throttle",)
    with ProductionSession(args.port, windowed=True,
                           extra_args=extra_args,
                           log_path=log_path) as session:
        pid = session.process.pid
        assert Path(f"/proc/{pid}/cwd").resolve() == ROOT
        assert Path(f"/proc/{pid}/exe").resolve() == BINARY.resolve()
        response = session.post({"type": "ExecuteCommand", "command": "TogglePerfHud",
                                 "params": {"enabled": True}})
        if response.get("error"):
            raise RuntimeError(f"performance HUD admission failed: {response}")
        if args.twin:
            session.post({"type": "ExecuteCommand", "command": "OpenTwin",
                          "params": {"path": str(args.twin.resolve())}})
        deadline = time.monotonic() + 120
        while time.monotonic() < deadline:
            response = session.post({"type": "ExecuteCommand", "command": "RunRhai", "params": {
                "code": "let c=clock_snapshot(); print(c.world_sim_s > 0.0 && "
                        f"c.barrier_active_participants == {args.participants} && "
                        "!c.simulation_progress.held);"}})
            if json.loads(response["data"]["stdout"]) is True:
                break
            time.sleep(.1)
        else:
            raise RuntimeError("the requested scene did not begin simulation")
        time.sleep(args.settle_seconds)
        focus = None if args.keep_focus else subprocess.Popen(
            ["xmessage", "-center", "-buttons", "", "LunCoSim transport acceptance: background pacing"],
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
        )
        try:
            deadline = time.monotonic() + 5
            while focus is not None and time.monotonic() < deadline:
                with log_path.open("rb") as log:
                    focus_log = log.read().decode(errors="replace")
                focus_states = re.findall(r"\[pacing\].*focused=Some\((true|false)\)", focus_log)
                if focus_states and focus_states[-1] == "false":
                    break
                time.sleep(.05)
            if focus is not None and (not focus_states or focus_states[-1] != "false"):
                raise RuntimeError("the desktop did not unfocus the simulation window")
            offset = log_path.stat().st_size
            response = session.post({"type": "ExecuteCommand", "command": "RunScenarioAsset",
                                     "params": {"source_asset": "lunco://scenarios/tests/simulation_transport_rate.rhai",
                                                "params": {"participants": args.participants,
                                                           "rates": args.rates,
                                                           "sample_seconds": args.sample_seconds,
                                                           "warmup_seconds": args.warmup_seconds}}})
            if response.get("error"):
                raise RuntimeError(f"scenario admission failed: {response}")
            # Asset admission precedes the authored warmup and sample clocks.
            # Give each observed phase its own bounded completion deadline.
            deadline = time.monotonic() + 30
            observed = {}
            # HTTP requests wake the window. Observe only the authored verdict
            # while the host is responsible for its own update cadence.
            while time.monotonic() < deadline:
                with log_path.open("rb") as log:
                    log.seek(offset)
                    verdict = log.read().decode(errors="replace")
                observed_at = time.monotonic()
                for match in re.finditer(
                    r"TRANSPORT_RATE_(WARMUP|BEGIN|SAMPLE) requested=([\d.eE+-]+)"
                    r"(?: actual=([\d.eE+-]+) wall_s=([\d.eE+-]+))?(?=[^\n]*\n)", verdict,
                ):
                    key = (match[1], float(match[2]))
                    if key not in observed:
                        observed[key] = (observed_at, match[3], match[4])
                        if match[1] == "WARMUP":
                            deadline = observed_at + args.warmup_seconds + 10
                        elif match[1] == "BEGIN":
                            deadline = observed_at + args.sample_seconds + 10
                if any(failure in verdict for failure in (
                    "TESTS_FAIL", "task leaf failed", "failed to load requested scenario asset",
                    "on_start() failed", "on_tick() failed",
                )):
                    if "TESTS_FAIL" in verdict:
                        capture_timing(session, output)
                        session.capture_screenshot(output / "failed.png")
                        assert_render_log_clean(log_path)
                    raise RuntimeError(f"authored transport gate failed; see {log_path}")
                if "SIMULATION TRANSPORT RATE: PASS" in verdict and f"TESTS_OK {check_count}\n" in verdict:
                    break
                if session.process.poll() is not None or (focus is not None and focus.poll() is not None):
                    raise RuntimeError("a required acceptance process exited early")
                time.sleep(.01)
            else:
                capture_timing(session, output)
                session.capture_screenshot(output / "timed-out.png")
                raise RuntimeError(f"no authored transport verdict; see {log_path}")
            if not args.keep_focus and "focused=Some(true)" in verdict:
                raise RuntimeError("the simulation regained focus during measurement")
            samples = [(rate, values) for (kind, rate), values in observed.items()
                       if kind == "SAMPLE"]
            if len(samples) != len(args.rates):
                raise RuntimeError("the authored gate did not publish every rate sample")
            for rate, (finished, native_rate, native_wall) in samples:
                started = observed[("BEGIN", rate)][0]
                if finished <= started:
                    raise RuntimeError("the external clock could not resolve the sample interval")
                actual = float(native_rate) * float(native_wall) / (finished - started)
                print(f"requested {rate:g}x: {actual:.3f}x against the process monotonic clock")
                if not rate * .99 <= actual <= rate * 1.01:
                    capture_timing(session, output)
                    raise RuntimeError(f"{rate:g}x transport failed the independent wall-clock check")
            capture_timing(session, output)
            session.capture_screenshot(output / "completed.png")
            assert_render_log_clean(log_path)
            if args.check_modelica_hud:
                session.post({"type": "ExecuteCommand", "command": "SetTimeTransport",
                              "params": {"playing": True, "rate": 1.0}})
                time.sleep(3)
                session.capture_screenshot(output / "modelica-running.png")
                run_authored_scenario(
                    session, "lunco://scenarios/tests/modelica_perf_hud.rhai",
                    title="MODELICA PERFORMANCE HUD", check_count=10,
                    params={"participants": args.participants},
                )
                session.capture_screenshot(output / "modelica-cleared.png")
                assert_render_log_clean(log_path)
        finally:
            if focus is not None:
                if focus.poll() is None:
                    focus.terminate()
                focus.wait(timeout=5)
    print(f"SIMULATION TRANSPORT RATE: {check_count} authored checks passed; port {args.port} released")


if __name__ == "__main__":
    main()
