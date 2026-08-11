from pathlib import Path
import unittest


ROOT = Path(__file__).parents[2]


class NightlyReleaseContractTests(unittest.TestCase):
    def test_human_and_updater_releases_have_separate_immutable_versions(self) -> None:
        workflow = (ROOT / ".github/workflows/nightly.yml").read_text(encoding="utf-8")

        self.assertIn("LunCoSim/lunco-sim-updates", workflow)
        self.assertIn("secrets.LUNCOSIM_UPDATES_TOKEN", workflow)
        self.assertIn("--draft", workflow)
        self.assertIn("--draft=false", workflow)
        self.assertNotIn("nightly-updates", workflow)
        self.assertNotIn("--clobber", workflow)

    def test_application_reads_the_machine_only_repository(self) -> None:
        updater = (
            ROOT / "crates/lunco-luncosim/src/ui/update.rs"
        ).read_text(encoding="utf-8")

        self.assertIn(
            '"https://github.com/LunCoSim/lunco-sim-updates"', updater
        )
        self.assertIn("GithubSource::new(UPDATE_REPOSITORY, None, true)", updater)
        self.assertNotIn("HttpSource", updater)


if __name__ == "__main__":
    unittest.main()
