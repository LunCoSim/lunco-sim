from pathlib import Path
import re
import unittest


ROOT = Path(__file__).parents[2]
EXPECTED_FONT_TARGETS = [
    "lunica-native",
    "luncosim-native",
    "lunica-web",
    "luncosim-web",
]


class RuntimeFontBundleContractTests(unittest.TestCase):
    def test_dejavu_is_declared_for_every_application_build(self) -> None:
        manifest = (ROOT / "assets/manifests/fonts.toml").read_text(encoding="utf-8")
        section = re.search(r"(?ms)^\[dejavu_sans\]\s*(.*?)(?=^\[|\Z)", manifest)
        self.assertIsNotNone(section, "fonts.toml must declare the DejaVu Sans artifact")
        entry = section.group(1)
        bundle = re.search(r"(?m)^bundle\s*=\s*\[([^\]]*)\]\s*$", entry)
        self.assertIsNotNone(bundle, "DejaVu Sans must declare package bundle targets")
        targets = re.findall(r'"([^"]+)"', bundle.group(1))

        self.assertEqual(targets, EXPECTED_FONT_TARGETS)
        self.assertIn('dest = "fonts/DejaVuSans.ttf"', entry)


if __name__ == "__main__":
    unittest.main()
