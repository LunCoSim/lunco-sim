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
        self.assertIn("LunCoSim-Linux-x86_64.AppImage", workflow)
        self.assertIn("Download update", workflow)
        self.assertIn("Install and restart", workflow)
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
        self.assertIn('const UPDATE_CHANNEL: &str = "linux-x64";', updater)
        self.assertIn("Download update", updater)
        self.assertIn("Install and restart", updater)
        self.assertNotIn("HttpSource", updater)

    def test_linux_download_explains_the_update_managed_appimage_path(self) -> None:
        app_guide = (ROOT / "docs/apps/luncosim/README.md").read_text(encoding="utf-8")
        package_builder = (ROOT / "scripts/build_native.sh").read_text(encoding="utf-8")

        for text in (app_guide, package_builder):
            self.assertIn(".AppImage", text)
            self.assertIn("writable", text)
            self.assertIn("same file", text)
            self.assertIn("Download update", text)
            self.assertIn("Install and restart", text)

        self.assertIn("LunCoSim/lunco-sim-updates", app_guide)


if __name__ == "__main__":
    unittest.main()
