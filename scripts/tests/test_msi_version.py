import unittest

from scripts.msi_version import (
    check_published_versions,
    next_development_version,
    next_nightly_run_number,
    windows_installer_version,
)


class MsiVersionTests(unittest.TestCase):
    def test_dev_semver_maps_to_monotonic_msi_versions(self):
        self.assertEqual(windows_installer_version("0.6.0-dev.1"), "1.6.1")
        self.assertEqual(windows_installer_version("0.6.0-dev.2"), "1.6.2")
        self.assertEqual(windows_installer_version("0.6.0"), "1.6.999")

    def test_legacy_nightly_versions_keep_their_msi_sequence(self):
        self.assertEqual(
            windows_installer_version("0.6.0-nightly.139.1"), "1.0.13901"
        )
        with self.assertRaisesRegex(ValueError, "unsupported legacy nightly"):
            windows_installer_version("0.6.0-nightly.139.100")

    def test_release_allocator_uses_the_next_dev_sequence(self):
        assets = [
            "luncosim-0.6.0-dev.1-win-x64-full.nupkg",
            "luncosim-0.6.0-dev.2-win-x64-full.nupkg",
        ]
        self.assertEqual(next_development_version("0.6.0", assets), "0.6.0-dev.3")
        self.assertEqual(
            next_development_version(
                "0.6.0",
                ["luncosim-0.6.0-nightly.160.1-win-x64-full.nupkg"],
            ),
            "0.6.0-dev.1",
        )

    def test_nightly_run_number_continues_published_windows_packages(self):
        assets = [
            "luncosim-0.6.0-nightly.135.2-win-x64-full.nupkg",
            "LunCoSim-win-x64-0.6.0-nightly.151.1-full.nupkg",
            "luncosim-0.6.0-nightly.999.1-osx-x64-full.nupkg",
            "luncosim-0.6.0-dev.3-win-x64-full.nupkg",
        ]
        self.assertEqual(next_nightly_run_number(assets), 152)
        self.assertEqual(next_nightly_run_number([]), 1)

    def test_publication_requires_newer_semver_and_msi_versions(self):
        assets = ["luncosim-0.6.0-dev.1-win-x64-full.nupkg"]
        check_published_versions("0.6.0-dev.2", assets)
        for candidate in ("0.6.0-dev.1", "0.6.0", "0.5.0-dev.2"):
            with self.subTest(candidate=candidate):
                with self.assertRaises(ValueError):
                    check_published_versions(candidate, assets)

        # Existing update releases used the runtime before and after the version.
        check_published_versions(
            "0.6.0-dev.1",
            ["LunCoSim-win-x64-0.6.0-nightly.160.1-win-x64-full.nupkg"],
        )
        check_published_versions(
            "0.6.0-dev.1",
            ["LunCoSim-win-x64-0.6.0-nightly.37.1-win-x64-full.nupkg"],
        )

    def test_invalid_or_exhausted_dev_versions_fail(self):
        for version in ("0.6.0-dev.0", "0.6.0-dev.999", "0.6.0.1", "0.256.0"):
            with self.subTest(version=version):
                with self.assertRaises(ValueError):
                    windows_installer_version(version)

        with self.assertRaisesRegex(ValueError, "sequence exhausted"):
            next_development_version(
                "0.6.0",
                ["luncosim-0.6.0-dev.998-win-x64-full.nupkg"],
            )


if __name__ == "__main__":
    unittest.main()
