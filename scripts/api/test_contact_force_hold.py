"""Exercise contact reporting across a held cycle through the scene-test host."""

from pathlib import Path

from runtime import BINARY, ROOT, ProductionSession, run_authored_scenario


def main() -> None:
    with ProductionSession(
        4396,
        subcommand="test",
        extra_args=("--scene", "assets/scenes/tests/landing_legs.usda",
                    "--verdict-channel", "CONTACT_HOLD", "--max-ticks", "1200"),
        log_path=Path(".cache/codex-evidence/celestial-integrated/contact-hold.log"),
    ) as session:
        pid = session.process.pid
        assert Path(f"/proc/{pid}/cwd").resolve() == ROOT
        assert Path(f"/proc/{pid}/exe").resolve() == BINARY.resolve()
        print(f"OWNED pid={pid} port=4396 cwd={ROOT}", flush=True)
        try:
            run_authored_scenario(
                session, "lunco://scenarios/tests/contact_force_hold.rhai",
                title="CONTACT HOLD", check_count=6,
                params={"path": "/LandingLegsTest/Lander/LegPZ"}, timeout_s=120,
            )
        finally:
            # The scene-test host owns terminal exit after its verdict. Await
            # that exit before the shared API-session shutdown boundary.
            code = session.process.wait(timeout=15)
        if code != 0:
            raise RuntimeError(f"contact scene-test returned {code}")
    print("CONTACT HOLD: owned scene-test exit and port release verified")


if __name__ == "__main__":
    main()
