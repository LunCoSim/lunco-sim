"""Exercise presentation sampling across transport changes in the production test host."""
import argparse
from pathlib import Path

from runtime import BINARY, ROOT, ProductionSession, run_authored_scenario


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port", type=int, default=4519)
    args = parser.parse_args()
    with ProductionSession(
        args.port, subcommand="test",
        extra_args=("--scene", "assets/scenes/tests/modelica_runtime_contracts.usda",
                    "--tick-hz", "240", "--jitter", "0.35",
                    "--verdict-channel", "PHYSICAL_PRESENTATION_TIME", "--max-ticks", "5000"),
        log_path=Path("/tmp/physical-presentation-time.log"),
    ) as session:
        pid = session.process.pid
        assert Path(f"/proc/{pid}/cwd").resolve() == ROOT
        assert Path(f"/proc/{pid}/exe").resolve() == BINARY.resolve()
        run_authored_scenario(
            session, "lunco://scenarios/tests/physical_presentation_time.rhai",
            title="PHYSICAL PRESENTATION TIME", check_count=13, timeout_s=60,
        )
        code = session.process.wait(timeout=15)
        if code != 0:
            raise RuntimeError(f"presentation scene-test returned {code}")
    print("PHYSICAL PRESENTATION TIME: verdict, process exit and port release verified")


if __name__ == "__main__":
    main()
