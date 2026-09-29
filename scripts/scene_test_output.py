"""Parse the terminal result emitted by the production scene-test runner."""

from __future__ import annotations

from dataclasses import dataclass
import re


_SCENE_TEST_SUMMARY = re.compile(
    r"^luncosim test "
    r"(?P<verdict>PASS|FAIL|NO-VERDICT|INCONCLUSIVE|ERROR|UNVERIFIED)"
    r"\s+scene=(?P<scene>.*?)\s+"
    r"(?:channel=(?P<channel>\S+)\s+)?"
    r"ticks=(?P<ticks>\d+)(?:\s|$)",
    re.MULTILINE,
)


@dataclass(frozen=True)
class SceneTestSummary:
    verdict: str
    scene: str
    channel: str | None
    ticks: int


def parse_scene_test_summary(output: str) -> SceneTestSummary:
    """Read the single terminal scene-test result, independent of Rhai log output."""
    matches = list(_SCENE_TEST_SUMMARY.finditer(output))
    if len(matches) != 1:
        raise ValueError(
            "expected exactly one terminal `luncosim test` summary, "
            f"found {len(matches)}"
        )
    match = matches[0]
    return SceneTestSummary(
        verdict=match.group("verdict"),
        scene=match.group("scene"),
        channel=match.group("channel"),
        ticks=int(match.group("ticks")),
    )


def require_scene_test_pass(
    output: str, *, expected_scene: str, expected_channel: str
) -> SceneTestSummary:
    """Require the runner's PASS result for the exact authored scene and channel."""
    summary = parse_scene_test_summary(output)
    if summary.verdict != "PASS":
        raise ValueError(f"scene-test verdict was {summary.verdict}, expected PASS")
    if summary.scene != expected_scene:
        raise ValueError(
            f"scene-test summary named scene {summary.scene!r}, "
            f"expected {expected_scene!r}"
        )
    if summary.channel != expected_channel:
        raise ValueError(
            f"scene-test summary named channel {summary.channel!r}, "
            f"expected {expected_channel!r}"
        )
    return summary
