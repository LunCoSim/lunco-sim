"""Exercise presentation sampling in production scene-test or native window hosts."""
import argparse
import os
from pathlib import Path

from runtime import BINARY, ROOT, ProductionSession, run_authored_scenario


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port", type=int, default=4519)
    parser.add_argument("--windowed", action="store_true")
    parser.add_argument("--output", type=Path, default=ROOT / "target/physical-presentation-time")
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    os.environ.update(LUNCOSIM_EPHEMERAL_SETTINGS="1", LUNCOSIM_CONFIG=str(output / "config"))
    extra_args = ["--scene", "assets/scenes/tests/modelica_runtime_contracts.usda"]
    if not args.windowed:
        extra_args += ["--tick-hz", "240", "--jitter", "0.35",
                       "--verdict-channel", "PHYSICAL_PRESENTATION_TIME", "--max-ticks", "5000"]
    with ProductionSession(
        args.port, subcommand=None if args.windowed else "test", windowed=args.windowed,
        extra_args=extra_args, log_path=output / "runtime.log",
    ) as session:
        pid = session.process.pid
        assert Path(f"/proc/{pid}/cwd").resolve() == ROOT
        assert Path(f"/proc/{pid}/exe").resolve() == BINARY.resolve()
        run_authored_scenario(
            session, "lunco://scenarios/tests/physical_presentation_time.rhai",
            # The native gate includes paused, unfocused reactive frames. Its
            # deadline is a liveness bound, not a throughput acceptance target.
            title="PHYSICAL PRESENTATION TIME", check_count=13,
            timeout_s=180 if args.windowed else 60,
        )
        if not args.windowed:
            code = session.process.wait(timeout=15)
            if code != 0:
                raise RuntimeError(f"presentation scene-test returned {code}")
    print("PHYSICAL PRESENTATION TIME: verdict, process exit and port release verified")


if __name__ == "__main__":
    main()
