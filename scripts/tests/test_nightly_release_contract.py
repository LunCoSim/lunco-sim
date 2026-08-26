from pathlib import Path
import unittest


ROOT = Path(__file__).parents[2]


class NightlyReleaseContractTests(unittest.TestCase):
    def test_human_release_keeps_updater_details_out_of_release_notes(self) -> None:
        workflow = (ROOT / ".github/workflows/nightly.yml").read_text(encoding="utf-8")

        self.assertIn("- cron: '0 22 * * *'", workflow)
        self.assertNotIn("- cron: '0 2 * * *'", workflow)
        self.assertIn("LunCoSim/lunco-sim-updates", workflow)
        self.assertIn("secrets.LUNCOSIM_UPDATES_TOKEN", workflow)
        self.assertIn("--draft", workflow)
        self.assertIn("--draft=false", workflow)
        self.assertNotIn('git push origin "${{ needs.meta.outputs.tag }}"', workflow)
        self.assertNotIn("--verify-tag", workflow)
        self.assertIn('--target "${{ github.sha }}"', workflow)
        main_release_edit = workflow.index(
            'GH_TOKEN="$MAIN_GH_TOKEN" gh release edit "$RELEASE_TAG"'
        )
        main_release_block = workflow[main_release_edit : main_release_edit + 220]
        self.assertIn("--latest=true", main_release_block)
        self.assertNotIn("--prerelease", main_release_block)
        updates_release_edit = workflow.index(
            'GH_TOKEN="$UPDATES_GH_TOKEN" gh release edit "$RELEASE_TAG"'
        )
        updates_release_block = workflow[
            updates_release_edit : updates_release_edit + 260
        ]
        self.assertIn("--latest=false", updates_release_block)
        release_notes_start = workflow.index(
            "python3 skills/nightly-changelog/scripts/generate_changelog.py"
        )
        release_notes_end = workflow.index(
            "# Check for an existing immutable traceability tag", release_notes_start
        )
        release_notes_block = workflow[release_notes_start:release_notes_end]
        self.assertNotIn("Download update", release_notes_block)
        self.assertNotIn("Install and restart", release_notes_block)
        self.assertNotIn("Settings → Updates", release_notes_block)
        self.assertNotIn("Velopack update", release_notes_block)
        self.assertNotIn("cat >> release_notes.md", release_notes_block)
        self.assertIn("--format release-notes", release_notes_block)
        self.assertNotIn("Changes since previous nightly", release_notes_block)
        self.assertIn("--output release_notes.md", release_notes_block)
        self.assertNotIn("nightly-updates", workflow)
        self.assertNotIn("--clobber", workflow)

    def test_application_reads_the_machine_only_repository(self) -> None:
        updater = (
            ROOT / "crates/lunco-luncosim/src/ui/update.rs"
        ).read_text(encoding="utf-8")

        self.assertIn(
            '"https://github.com/LunCoSim/lunco-sim-updates"', updater
        )
        self.assertIn("TimeoutGithubSource::new(UPDATE_REPOSITORY, true)", updater)
        self.assertIn("UPDATE_HTTP_TIMEOUT", updater)
        self.assertIn('.header("Range", &range)', updater)
        self.assertIn('const UPDATE_CHANNEL: &str = "linux-x64";', updater)
        self.assertIn("Download update", updater)
        self.assertIn("Install and restart", updater)
        self.assertIn('const UPDATE_CHANNEL: &str = "win-x64";', updater)
        self.assertIn('const UPDATE_CHANNEL: &str = "osx-x64";', updater)
        self.assertIn('const UPDATE_CHANNEL: &str = "osx-arm64";', updater)
        self.assertNotIn("let source = GithubSource::new", updater)
        self.assertNotIn("HttpSource", updater)

    def test_linux_download_explains_the_update_managed_appimage_path(self) -> None:
        app_guide = (ROOT / "docs/apps/luncosim/README.md").read_text(encoding="utf-8")
        package_builder = (ROOT / "scripts/build_native.sh").read_text(encoding="utf-8")

        for text in (app_guide, package_builder):
            self.assertIn(".AppImage", text)
            self.assertIn("writable", text)
            self.assertIn("same AppImage", text)
            self.assertIn("Download update", text)
            self.assertIn("Install and restart", text)

        self.assertIn("LunCoSim/lunco-sim-updates", app_guide)
        self.assertIn("LunCoSim-Windows-x86_64-Setup.exe", app_guide)
        self.assertIn("LunCoSim-macOS-Apple-Silicon.pkg", app_guide)
        self.assertIn("LunCoSim-macOS-Intel.pkg", app_guide)


if __name__ == "__main__":
    unittest.main()
