#!/usr/bin/env python3
"""Verify terminal scene-load errors and recovery in the visible production app."""
import argparse
import json
import time
from pathlib import Path

from runtime import ROOT, ProductionSession, wait_for_ready
from run_editor_scene_test import wait_for_scene, wait_for_scene_time_selection
from test_native_trim_precision import execute, run


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--port', type=int, required=True)
    parser.add_argument('--log', type=Path, required=True)
    args = parser.parse_args()
    scene = ROOT / 'assets/scenes/tests/twin_session_retirement/replacement/replacement.usda'
    valid = 'lunco://scenes/tests/twin_session_retirement/replacement/replacement.usda'
    missing = 'lunco://scenes/tests/__missing_asset_status_gate__.usda'
    with ProductionSession(args.port, extra_args=('--no-throttle', '--scene', str(scene)),
                           log_path=args.log, windowed=True) as session:
        wait_for_scene(session, str(scene), 60)
        wait_for_scene_time_selection(session, args.log, 60)
        wait_for_ready(args.port, 60)
        execute(session, 'LoadScene', path=missing, root_prim='')
        deadline = time.monotonic() + 30
        while time.monotonic() < deadline:
            state = execute(session, 'GetReadiness')
            if state.get('faulted'):
                break
            time.sleep(.1)
        else:
            raise RuntimeError('missing asset did not publish a terminal fault')
        args.log.with_suffix('.failed.json').write_text(json.dumps(state, indent=2) + '\n')
        session.capture_screenshot(ROOT / 'target' / 'asset-failure-evidence' / (args.log.stem + '.failed.png'))
        tick = int(run(session, 'print(clock_snapshot().sim_tick);').strip())
        frame = execute(session, 'EnginePerformance')['frame_sample_count']
        while execute(session, 'EnginePerformance')['frame_sample_count'] < frame + 8:
            if time.monotonic() >= deadline:
                raise RuntimeError('failed scene stopped command/UI processing')
            time.sleep(.02)
        output = run(session, f'let stopped_tick = {tick};\n' + '''
let state = query("GetReadiness");
report_verdict([
    t_true(state.ready == false, "failed scene never reports ready"),
    t_true(clock_snapshot().sim_tick == stopped_tick, "failed scene stops simulation while commands remain available"),
    t_true(state.faulted == true, "failed scene exposes a terminal fault"),
    t_true(state.fault.kind == "scene-load-failed", "fault identifies scene admission"),
    t_true(state.fault.subject.contains("__missing_asset_status_gate__.usda"), "fault names the requested asset"),
    t_true(state.fault.detail.len() > 0, "fault retains the loader error")
], "USD ASSET FAILURE", "USD_ASSET_FAILURE");
''')
        print(output, flush=True)
        if 'TESTS_OK 6' not in output or 'TESTS_FAIL' in output:
            raise RuntimeError('terminal failure verdict failed')
        execute(session, 'LoadScene', path=valid, root_prim='')
        wait_for_scene(session, str(scene), 60)
        wait_for_ready(args.port, 60)
        output = run(session, '''
let state = query("GetReadiness");
report_verdict([
    t_true(state.ready == true && state.faulted == false && state.world_hold == false,
        "valid replacement clears only the retired scene fault")
], "USD ASSET RECOVERY", "USD_ASSET_RECOVERY");
''')
        print(output, flush=True)
        if 'TESTS_OK 1' not in output or 'TESTS_FAIL' in output:
            raise RuntimeError('recovery verdict failed')
    print('PASS — seven authored checks; owned headful session closed')


if __name__ == '__main__':
    main()
