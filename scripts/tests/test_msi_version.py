import unittest

from scripts.msi_version import (
    check_published_versions,
    nightly_msi_version,
    next_nightly_run_number,
)


class MsiVersionTests(unittest.TestCase):
    def test_nightly_versions_are_unique_and_ordered_across_rollover(self):
        versions = [nightly_msi_version(run, attempt) for run, attempt in [
            (135, 1), (135, 2), (136, 1), (655, 36), (655, 37), (656, 1)
        ]]
        numeric = [tuple(map(int, version.split("."))) for version in versions]
        self.assertEqual(len(set(numeric)), len(numeric))
        self.assertEqual(numeric, sorted(numeric))
        self.assertEqual(nightly_msi_version(135, 2), "1.0.13502")
        self.assertEqual(nightly_msi_version(655, 36), "1.1.0")

    def test_invalid_or_exhausted_sequences_fail(self):
        for run, attempt in [(0, 1), (1, 0), (1, 100), (167773, 1)]:
            with self.subTest(run=run, attempt=attempt):
                with self.assertRaises(ValueError):
                    nightly_msi_version(run, attempt)

    def test_published_versions_must_increase_in_both_orderings(self):
        assets = ["luncosim-0.6.0-nightly.135.2-win-x64-full.nupkg"]
        check_published_versions("0.6.0-nightly.136.1", assets)
        for candidate in (
            "0.6.0-nightly.135.1", "0.6.0-nightly.135.2",
            "0.5.0-nightly.136.1", "0.7.0-nightly.134.1",
        ):
            with self.subTest(candidate=candidate):
                with self.assertRaises(ValueError):
                    check_published_versions(candidate, assets)
        with self.assertRaises(ValueError):
            check_published_versions("0.6.0-nightly.136.1", ["luncosim-0.6.0-win-x64-full.nupkg"])

    def test_next_run_continues_the_existing_nightly_sequence(self):
        assets = [
            "luncosim-0.6.0-nightly.135.2-win-x64-full.nupkg",
            "luncosim-0.6.0-nightly.151.1-win-x64-full.nupkg",
            "luncosim-0.6.0-win-x64-full.nupkg",
        ]
        self.assertEqual(next_nightly_run_number(assets), 152)
        self.assertEqual(next_nightly_run_number([]), 1)
