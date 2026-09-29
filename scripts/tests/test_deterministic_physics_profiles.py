from __future__ import annotations

import importlib.util
import sys
import unittest
from pathlib import Path


SCRIPT = Path(__file__).parents[1] / "compare_deterministic_physics_profiles.py"
SPEC = importlib.util.spec_from_file_location("deterministic_physics_profiles", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
deterministic_physics_profiles = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = deterministic_physics_profiles
SPEC.loader.exec_module(deterministic_physics_profiles)


class DeterministicPhysicsProfileTests(unittest.TestCase):
    def test_required_sample_tick_sequence_detects_missing_ticks(self) -> None:
        expected = deterministic_physics_profiles.EXPECTED_STRESS_SAMPLE_TICKS
        complete = "\n".join(
            f"D4_TICK_STATE_TRACE_V1|{tick};" for tick in expected
        )
        deterministic_physics_profiles.validate_physics_trace_ticks(
            complete, "complete profile"
        )

        missing_tick = "19"
        incomplete = "\n".join(
            f"D4_TICK_STATE_TRACE_V1|{tick};"
            for tick in expected
            if tick != missing_tick
        )
        with self.assertRaisesRegex(RuntimeError, r"missing=\['19'\]"):
            deterministic_physics_profiles.validate_physics_trace_ticks(
                incomplete, "incomplete profile"
            )

    def _eight_rover_trace(
        self, extra_lane_velocity: float, contact_count: int = 1
    ) -> str:
        rows = ["1"]
        for index in range(8):
            lane_x = -17.5 + index * 5.0
            velocity_x = extra_lane_velocity if index == 0 else 0.0
            rows.append(
                f"/MultiRoverStress/Rovers/Rover{index + 1:02d}"
                "|clock=0.016666667,0.016666667,0.016666667,"
                "0.016666667,0.0,false,false"
                f"|p={lane_x},1.6,0|laneX={lane_x}"
                f"|authoredTf={lane_x},1.6,0|v={velocity_x},0,0"
                f"|avianContact={contact_count},{contact_count},{contact_count},0,0,0,0"
            )
        return "D4_TICK_STATE_TRACE_V1|" + ";".join(rows)

    def test_matrix_can_detect_divergence_in_an_eight_rover_only_lane(self) -> None:
        baseline = self._eight_rover_trace(0.0)
        changed = self._eight_rover_trace(1.0)
        changed_contacts = self._eight_rover_trace(0.0, contact_count=2)
        baseline_tick = deterministic_physics_profiles.TICK_TRACE_PATTERN.findall(
            baseline
        )[0]
        roster = deterministic_physics_profiles.authored_roster(baseline_tick, 8)
        shared_positions = {"-7.5", "-2.5", "2.5", "7.5"}

        _, baseline_shared = deterministic_physics_profiles.canonical_physics_trace(
            baseline, 8, shared_positions, {"1"}
        )
        _, changed_shared = deterministic_physics_profiles.canonical_physics_trace(
            changed, 8, shared_positions, {"1"}
        )
        self.assertEqual(baseline_shared, changed_shared)

        all_positions = set(roster.values())
        _, baseline_full = deterministic_physics_profiles.canonical_physics_trace(
            baseline,
            8,
            all_positions,
            {"1"},
            normalize_fixture_contact_counts=False,
        )
        _, changed_full = deterministic_physics_profiles.canonical_physics_trace(
            changed,
            8,
            all_positions,
            {"1"},
            normalize_fixture_contact_counts=False,
        )
        self.assertNotEqual(baseline_full, changed_full)

        _, baseline_normalized_full = (
            deterministic_physics_profiles.canonical_physics_trace(
                baseline, 8, all_positions, {"1"}
            )
        )
        _, normalized_contacts = (
            deterministic_physics_profiles.canonical_physics_trace(
                changed_contacts, 8, all_positions, {"1"}
            )
        )
        self.assertEqual(baseline_normalized_full, normalized_contacts)

        _, retained_contacts = deterministic_physics_profiles.canonical_physics_trace(
            changed_contacts,
            8,
            all_positions,
            {"1"},
            normalize_fixture_contact_counts=False,
        )
        self.assertNotEqual(baseline_full, retained_contacts)

    def test_state_comparison_uses_zero_numeric_tolerance(self) -> None:
        baseline = self._eight_rover_trace(0.0)
        slightly_changed = self._eight_rover_trace(1e-15)
        baseline_tick = deterministic_physics_profiles.TICK_TRACE_PATTERN.findall(
            baseline
        )[0]
        roster = deterministic_physics_profiles.authored_roster(baseline_tick, 8)
        all_positions = set(roster.values())

        _, baseline_state = deterministic_physics_profiles.canonical_physics_trace(
            baseline, 8, all_positions, {"1"}, normalize_fixture_contact_counts=False
        )
        _, changed_state = deterministic_physics_profiles.canonical_physics_trace(
            slightly_changed,
            8,
            all_positions,
            {"1"},
            normalize_fixture_contact_counts=False,
        )

        self.assertNotEqual(baseline_state, changed_state)

    def test_state_comparison_ignores_optional_authored_transform_diagnostic(self) -> None:
        state = (
            "/MultiRoverStress/Rovers/Rover01|p=-7.5,1.6,0|laneX=-7.5"
            "|authoredTf=-7.5,1.6,0|v=0,0,0"
        )
        without_diagnostic = state.replace("|authoredTf=-7.5,1.6,0", "")

        self.assertEqual(
            deterministic_physics_profiles.canonical_scenario_physics_row(state),
            deterministic_physics_profiles.canonical_scenario_physics_row(
                without_diagnostic
            ),
        )

    def test_modelica_fixture_comparison_uses_overlapping_authored_samples(self) -> None:
        early = ("2", "-7.5", "System")
        shared = ("10", "-7.5", "System")
        reference = {early: "early", shared: "same"}
        candidate = {shared: "same"}

        difference, compared_samples = (
            deterministic_physics_profiles.first_modelica_trace_difference(
                reference, candidate, common_samples_only=True
            )
        )

        self.assertIsNone(difference)
        self.assertEqual(compared_samples, 1)

        candidate[shared] = "changed"
        difference, compared_samples = (
            deterministic_physics_profiles.first_modelica_trace_difference(
                reference, candidate, common_samples_only=True
            )
        )
        self.assertEqual(difference, shared)
        self.assertEqual(compared_samples, 1)

    def test_state_comparison_excludes_transient_delta_but_keeps_elapsed_clocks(self) -> None:
        baseline = self._eight_rover_trace(0.0)
        changed_delta = baseline.replace(
            "clock=0.016666667,0.016666667,0.016666667,"
            "0.016666667,0.0,false,false",
            "clock=0.016666667,0.016666667,0.016666667,"
            "0.0,0.0,false,false",
        )
        changed_elapsed = baseline.replace(
            "clock=0.016666667,0.016666667,0.016666667,"
            "0.016666667,0.0,false,false",
            "clock=0.016666667,0.016666667,0.016666667,"
            "0.016666667,0.016666667,false,false",
        )
        all_positions = {
            format(-17.5 + index * 5.0, ".17g") for index in range(8)
        }

        _, reference = deterministic_physics_profiles.canonical_physics_trace(
            baseline, 8, all_positions, {"1"}, normalize_fixture_contact_counts=False
        )
        _, delta_variant = deterministic_physics_profiles.canonical_physics_trace(
            changed_delta,
            8,
            all_positions,
            {"1"},
            normalize_fixture_contact_counts=False,
        )
        _, elapsed_variant = deterministic_physics_profiles.canonical_physics_trace(
            changed_elapsed,
            8,
            all_positions,
            {"1"},
            normalize_fixture_contact_counts=False,
        )

        self.assertEqual(reference, delta_variant)
        self.assertNotEqual(reference, elapsed_variant)


if __name__ == "__main__":
    unittest.main()
