from __future__ import annotations

import unittest
from unittest.mock import patch

from scripts import generate_dev_release_notes


class GenerateDevReleaseNotesTests(unittest.TestCase):
    def test_notes_describe_signed_installers_and_keep_updater_mechanics_out(self):
        notes = generate_dev_release_notes.render_release_notes(
            release_date="2026-11-13 12:00 UTC",
            changelog_url="https://github.com/LunCoSim/lunco-sim/compare/0.6.0-dev.1...HEAD",
        )

        for installer in (
            "LunCoSim-Windows-x86_64.msi",
            "LunCoSim-macOS-Apple-Silicon.pkg",
            "LunCoSim-macOS-Intel.pkg",
            "LunCoSim-Linux-x86_64.AppImage",
        ):
            self.assertIn(installer, notes)
        self.assertIn("Authenticode-signed", notes)
        self.assertIn("download new development builds", notes)
        self.assertIn("favorite AI agent", notes)
        for updater_phrase in ("Velopack", "Download update", "Install and restart"):
            self.assertNotIn(updater_phrase, notes)

    def test_latest_baseline_uses_the_newest_dev_or_legacy_nightly_tag(self):
        refs = (
            "nightly-20260812T051950Z\t1800000000\n"
            "0.6.0-dev.1\t1800000100\n"
        )
        with patch.object(generate_dev_release_notes, "git", return_value=refs):
            self.assertEqual(generate_dev_release_notes.latest_release(), "0.6.0-dev.1")


if __name__ == "__main__":
    unittest.main()
