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
    def test_release_notes_name_only_the_four_human_installers(self) -> None:
        original_git = generate_changelog.git
        generate_changelog.git = lambda *args: "a" * 40
        try:
            notes = generate_changelog.render(
                from_ref="nightly-20260810T000000Z",
                to_ref="HEAD",
                release_date="2026-08-11 02:00 UTC",
                release_tag="nightly-20260811T020000Z",
                repo="https://github.com/LunCoSim/lunco-sim",
                commits=[],
            )
        finally:
            generate_changelog.git = original_git

        for installer in (
            "LunCoSim-Windows-x86_64-Setup.exe",
            "LunCoSim-macOS-Apple-Silicon.pkg",
            "LunCoSim-macOS-Intel.pkg",
            "LunCoSim-Linux-x86_64.AppImage",
        ):
            self.assertIn(installer, notes)
        self.assertNotIn("Portable.zip", notes)
        self.assertNotIn("run.sh", notes)
        self.assertNotIn("run.bat", notes)


if __name__ == "__main__":
    unittest.main()
