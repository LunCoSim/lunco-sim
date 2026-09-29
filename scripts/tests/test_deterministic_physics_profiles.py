from __future__ import annotations

import importlib.util
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path


SCRIPT = Path(__file__).parents[1] / "compare_deterministic_physics_profiles.py"
sys.path.insert(0, str(SCRIPT.parent))
import scene_test_output

SPEC = importlib.util.spec_from_file_location("deterministic_physics_profiles", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
deterministic_physics_profiles = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = deterministic_physics_profiles
SPEC.loader.exec_module(deterministic_physics_profiles)


class DeterministicPhysicsProfileTests(unittest.TestCase):
    def test_runner_pass_summary_is_authoritative_without_rhai_log_markers(self) -> None:
        output = "\n".join(
            [
                "[test] startup readiness admitted after 11021 updates at SimTick=0",
                "luncosim test PASS  scene=assets/scenes/tests/multi_rover_stress_20.usda  "
                "channel=MULTI_ROVER_STRESS  ticks=780  updates=1680  sim=13.00s  "
                "threads=1  jitter=0.000  seed=6840157149251759617  tick_hz=60.0000",
                "WARN lunco_modelica_telemetry: max_channels reached",
            ]
        )

        summary = scene_test_output.require_scene_test_pass(
            output,
            expected_scene="assets/scenes/tests/multi_rover_stress_20.usda",
            expected_channel="MULTI_ROVER_STRESS",
        )

        self.assertEqual(summary.verdict, "PASS")
        self.assertEqual(summary.ticks, 780)
        self.assertNotIn("TESTS_OK", output)
        self.assertNotIn("MULTI-ROVER STRESS: PASS", output)

    def test_runner_failure_cannot_be_overridden_by_rhai_pass_log_text(self) -> None:
        output = "\n".join(
            [
                "TESTS_OK 20",
                "MULTI-ROVER STRESS: PASS",
                "luncosim test FAIL  scene=assets/scenes/tests/multi_rover_stress_20.usda  "
                "channel=MULTI_ROVER_STRESS  ticks=780  updates=1680  sim=13.00s  "
                "threads=1  jitter=0.000  seed=6840157149251759617  tick_hz=60.0000",
            ]
        )

        with self.assertRaisesRegex(ValueError, "verdict was FAIL"):
            scene_test_output.require_scene_test_pass(
                output,
                expected_scene="assets/scenes/tests/multi_rover_stress_20.usda",
                expected_channel="MULTI_ROVER_STRESS",
            )

    def test_runner_pass_summary_must_match_scene_and_verdict_channel(self) -> None:
        output = (
            "luncosim test PASS  scene=assets/scenes/tests/sensor.usda  "
            "channel=SENSORS  ticks=8  updates=24  sim=0.13s  "
            "threads=1  jitter=0.000  seed=42  tick_hz=60.0000"
        )

        with self.assertRaisesRegex(ValueError, "expected 'MULTI_ROVER_STRESS'"):
            scene_test_output.require_scene_test_pass(
                output,
                expected_scene="assets/scenes/tests/sensor.usda",
                expected_channel="MULTI_ROVER_STRESS",
            )

    def test_rhai_prints_without_terminal_runner_summary_are_not_a_pass(self) -> None:
        output = "TESTS_OK 20\nMULTI-ROVER STRESS: PASS"

        with self.assertRaisesRegex(ValueError, "exactly one terminal"):
            scene_test_output.require_scene_test_pass(
                output,
                expected_scene="assets/scenes/tests/multi_rover_stress_20.usda",
                expected_channel="MULTI_ROVER_STRESS",
            )

    def test_default_run_compares_the_checked_in_portable_reference(self) -> None:
        args = deterministic_physics_profiles.parse_arguments([])
        self.assertEqual(
            args.compare_reference,
            deterministic_physics_profiles.DEFAULT_REFERENCE_PATH,
        )

    def test_reference_file_is_excluded_from_tracked_source_fingerprint(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)

            def git(*args: str) -> None:
                subprocess.run(
                    ["git", *args], cwd=root, check=True, capture_output=True
                )

            git("init", "--quiet")
            git("config", "user.name", "Deterministic test")
            git("config", "user.email", "deterministic-test@example.invalid")
            git("config", "commit.gpgsign", "false")
            source = root / "crates" / "physics.rs"
            source.parent.mkdir(parents=True)
            source.write_text("pub const STEP_HZ: u32 = 60;\n", encoding="utf-8")
            git("add", "crates/physics.rs")
            git("commit", "--quiet", "-m", "source")
            source_fingerprint = (
                deterministic_physics_profiles.tracked_source_fingerprint(root)
            )

            reference = root / deterministic_physics_profiles.REFERENCE_FIXTURE_PATH
            reference.parent.mkdir(parents=True)
            reference.write_text('{"baseline":1}\n', encoding="utf-8")
            git("add", deterministic_physics_profiles.REFERENCE_FIXTURE_PATH.as_posix())
            git("commit", "--quiet", "-m", "add reference fixture")
            self.assertEqual(
                source_fingerprint,
                deterministic_physics_profiles.tracked_source_fingerprint(root),
            )

            source.write_text("pub const STEP_HZ: u32 = 120;\n", encoding="utf-8")
            git("add", "crates/physics.rs")
            git("commit", "--quiet", "-m", "change simulation source")
            self.assertNotEqual(
                source_fingerprint,
                deterministic_physics_profiles.tracked_source_fingerprint(root),
            )

    def _selected_state_output(self) -> str:
        ticks = ("0", "180", "360", "540", "720", "780")
        states = [f"D4_STATE_TRACE_V1|{tick};" for tick in ticks]
        return "\n".join(
            [*states, "D4_EARLY_STATE_TRACE_V1|1;", "D4_FINAL_STAGE_V1|780;"]
        )

    def test_selected_checkpoint_set_detects_missing_snapshots(self) -> None:
        complete = self._selected_state_output()
        deterministic_physics_profiles.validate_physics_trace_ticks(
            complete, "complete profile"
        )

        incomplete = "\n".join(
            line
            for line in complete.splitlines()
            if line != "D4_STATE_TRACE_V1|540;"
        )
        with self.assertRaisesRegex(RuntimeError, "expected 6 unique selected physics checkpoints"):
            deterministic_physics_profiles.validate_physics_trace_ticks(
                incomplete, "incomplete profile"
            )

    def test_final_stage_must_match_the_last_selected_state(self) -> None:
        deterministic_physics_profiles.validate_startup_trace(
            self._selected_state_output(), "complete profile"
        )
        mismatched = self._selected_state_output().replace(
            "D4_FINAL_STAGE_V1|780;", "D4_FINAL_STAGE_V1|781;"
        )
        with self.assertRaisesRegex(RuntimeError, "final-stage record differs"):
            deterministic_physics_profiles.validate_startup_trace(
                mismatched, "mismatched profile"
            )

    def test_full_modelica_state_must_include_the_actual_final_stage(self) -> None:
        model_trace = {("180", "-47.5", "/system"): "x=1.0"}
        with self.assertRaisesRegex(RuntimeError, "final-stage variables at SimTick=780"):
            deterministic_physics_profiles.require_final_modelica_stage(
                model_trace, "780", {"-47.5"}
            )

        model_trace[("780", "-47.5", "/system")] = "x=2.0"
        deterministic_physics_profiles.require_final_modelica_stage(
            model_trace, "780", {"-47.5"}
        )

    def test_portable_reference_requires_exact_final_stage_equality(self) -> None:
        reference = {
            "scene-4-serial": {
                "parameters": {"tick_hz": 60.0},
                "physics_checkpoints": [
                    {"tick": "360", "physics": ["p=10"]}
                ],
                "modelica_checkpoints": [
                    {
                        "tick": "180",
                        "systems": [{"system": "rover", "variables": "x=1"}],
                    }
                ],
                "articulated_checkpoints": [
                    {"tick": "11", "rovers": [{"bodies": "wheel=p0"}]}
                ],
                "final": {
                    "tick": "780",
                    "physics": ["p=20"],
                    "modelica": [
                        {"system": "rover", "variables": "x=2"}
                    ],
                },
            }
        }
        candidate = {
            "scene-4-serial": {
                "parameters": {"tick_hz": 60.0},
                "physics_checkpoints": [
                    {"tick": "360", "physics": ["p=10"]}
                ],
                "modelica_checkpoints": [
                    {
                        "tick": "180",
                        "systems": [{"system": "rover", "variables": "x=1"}],
                    }
                ],
                "articulated_checkpoints": [
                    {"tick": "11", "rovers": [{"bodies": "wheel=p0"}]}
                ],
                "final": {
                    "tick": "780",
                    "physics": ["p=20"],
                    "modelica": [
                        {"system": "rover", "variables": "x=2"}
                    ],
                },
            }
        }
        deterministic_physics_profiles.compare_profile_cases(reference, candidate)

        candidate["scene-4-serial"]["parameters"]["tick_hz"] = 30.0
        with self.assertRaisesRegex(RuntimeError, "parameters do not match"):
            deterministic_physics_profiles.compare_profile_cases(reference, candidate)
        candidate["scene-4-serial"]["parameters"]["tick_hz"] = 60.0

        candidate["scene-4-serial"]["articulated_checkpoints"] = [
            {"tick": "11", "rovers": [{"bodies": "wheel=p1"}]}
        ]
        with self.assertRaisesRegex(RuntimeError, "articulated-body checkpoint"):
            deterministic_physics_profiles.compare_profile_cases(reference, candidate)

        candidate["scene-4-serial"]["articulated_checkpoints"] = [
            {"tick": "11", "rovers": [{"bodies": "wheel=p0"}]}
        ]
        candidate["scene-4-serial"]["modelica_checkpoints"] = [
            {
                "tick": "180",
                "systems": [{"system": "rover", "variables": "x=1.1"}],
            }
        ]
        with self.assertRaisesRegex(RuntimeError, "selected Modelica checkpoint"):
            deterministic_physics_profiles.compare_profile_cases(reference, candidate)

        candidate["scene-4-serial"]["modelica_checkpoints"] = [
            {
                "tick": "180",
                "systems": [{"system": "rover", "variables": "x=1"}],
            }
        ]
        candidate["scene-4-serial"]["final"] = {
            "tick": "780",
            "physics": ["p=20"],
            "modelica": [
                {"system": "rover", "variables": "x=2.000000000000004"}
            ],
        }
        with self.assertRaisesRegex(
            RuntimeError, "final physics, Modelica.*differs exactly"
        ):
            deterministic_physics_profiles.compare_profile_cases(reference, candidate)

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
        return "D4_STATE_TRACE_V1|" + ";".join(rows)

    def test_matrix_can_detect_divergence_in_an_eight_rover_only_lane(self) -> None:
        baseline = self._eight_rover_trace(0.0)
        changed = self._eight_rover_trace(1.0)
        changed_contacts = self._eight_rover_trace(0.0, contact_count=2)
        baseline_tick = deterministic_physics_profiles.TRACE_PATTERN.findall(
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

    def test_first_behavior_tick_is_included_alongside_selected_checkpoints(self) -> None:
        checkpoint = self._eight_rover_trace(0.0).replace(
            "D4_STATE_TRACE_V1|1;", "D4_STATE_TRACE_V1|0;"
        )
        early_behavior = self._eight_rover_trace(0.0).replace(
            "D4_STATE_TRACE_V1|1;", "D4_EARLY_STATE_TRACE_V1|1;"
        )
        checkpoint_trace = deterministic_physics_profiles.TRACE_PATTERN.findall(
            checkpoint
        )[0]
        roster = deterministic_physics_profiles.authored_roster(checkpoint_trace, 8)

        _, physics = deterministic_physics_profiles.canonical_physics_trace(
            checkpoint + "\n" + early_behavior,
            8,
            set(roster.values()),
            {"0"},
        )

        self.assertEqual(set(physics), {"0", "1"})

    def test_state_comparison_uses_zero_numeric_tolerance(self) -> None:
        baseline = self._eight_rover_trace(0.0)
        slightly_changed = self._eight_rover_trace(1e-15)
        baseline_tick = deterministic_physics_profiles.TRACE_PATTERN.findall(
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
