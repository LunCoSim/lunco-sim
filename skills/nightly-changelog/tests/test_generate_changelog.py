from __future__ import annotations

import importlib.util
import sys
import unittest
from pathlib import Path


SCRIPT = Path(__file__).parents[1] / "scripts" / "generate_changelog.py"
SPEC = importlib.util.spec_from_file_location("generate_changelog", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
generate_changelog = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = generate_changelog
SPEC.loader.exec_module(generate_changelog)


class GenerateChangelogTests(unittest.TestCase):
    def test_release_notes_are_short_and_link_to_the_changelog(self) -> None:
        notes = generate_changelog.render_release_notes(
            release_date="2026-08-11 02:00 UTC",
            changelog_url="https://github.com/LunCoSim/lunco-sim/blob/main/docs/releases/nightly-20260811.md",
        )

        for installer in (
            "LunCoSim-Windows-x86_64-Setup.exe",
            "LunCoSim-macOS-Apple-Silicon.pkg",
            "LunCoSim-macOS-Intel.pkg",
            "LunCoSim-Linux-x86_64.AppImage",
        ):
            self.assertIn(installer, notes)
        self.assertIn("favorite AI agent", notes)
        self.assertIn("bundled documentation, skills, and examples", notes)
        self.assertIn("autoupdater", notes)
        self.assertIn("latest nightly build", notes)
        self.assertIn(
            "https://github.com/LunCoSim/lunco-sim/blob/main/docs/releases/nightly-20260811.md",
            notes,
        )
        self.assertNotIn("Changes since previous nightly", notes)
        self.assertNotIn("View all", notes)
        for updater_phrase in (
            "Velopack",
            "Download update",
            "Install and restart",
            "Settings → Updates",
        ):
            self.assertNotIn(updater_phrase, notes)

    def test_changelog_lists_all_changes_since_previous_nightly(self) -> None:
        changelog = generate_changelog.render_changelog(
            from_ref="nightly-20260810T000000Z",
            to_ref="HEAD",
            release_date="2026-08-11 02:00 UTC",
            repo="https://github.com/LunCoSim/lunco-sim",
            commits=[
                {
                    "full": "a" * 40,
                    "short": "a" * 9,
                    "date": "2026-08-10",
                    "title": "first change",
                },
                {
                    "full": "b" * 40,
                    "short": "b" * 9,
                    "date": "2026-08-11",
                    "title": "second change",
                },
            ],
        )

        self.assertIn("Changes since previous nightly", changelog)
        self.assertIn("Included commits: **2 non-merge commits**", changelog)
        self.assertIn("first change", changelog)
        self.assertIn("second change", changelog)
        self.assertIn("2026-08-10", changelog)
        self.assertIn("2026-08-11", changelog)
        self.assertIn(
            "https://github.com/LunCoSim/lunco-sim/compare/nightly-20260810T000000Z...HEAD",
            changelog,
        )


if __name__ == "__main__":
    unittest.main()
