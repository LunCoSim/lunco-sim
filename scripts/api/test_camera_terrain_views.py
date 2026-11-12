#!/usr/bin/env python3
"""Exercise independent main/rover-feed terrain covers and native physical support."""
from __future__ import annotations

import argparse
import json
import math
import os
import time
from pathlib import Path

from runtime import BINARY, ROOT, ProductionSession, assert_render_log_clean, port_is_open, run_authored_scenario


def await_value(read, predicate, label, timeout=120):
    deadline = time.monotonic() + timeout
    last = None
    while time.monotonic() < deadline:
        last = read()
        if predicate(last):
            return last
        time.sleep(.15)
    raise RuntimeError(f'{label} did not settle: {last}')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('scene', 'body-path', 'camera-path', 'mounted-camera-path'):
        parser.add_argument('--' + name, required=True)
    parser.add_argument('--main-eye', nargs=3, type=float, required=True,
                        help='fixed low camera position in the active physics frame')
    parser.add_argument('--main-look-at', nargs=3, type=float, required=True,
                        help='look-at point in the same active physics frame')
    for view in ('main', 'feed'):
        parser.add_argument(f'--{view}-min-spacing-m', type=float, required=True)
        parser.add_argument(f'--{view}-max-spacing-m', type=float, required=True)
    parser.add_argument('--require-distinct-depths', action='store_true')
    parser.add_argument('--maximum-budget-refused', type=int, default=0)
    parser.add_argument('--minimum-separation-m', type=float, default=60.0)
    parser.add_argument('--maximum-main-clearance-m', type=float, default=2.0)
    parser.add_argument('--drive-seconds', type=float, default=10.0)
    parser.add_argument('--minimum-drive-distance-m', type=float, default=2.0)
    parser.add_argument('--feed-width', type=int, default=640)
    parser.add_argument('--feed-height', type=int, default=360)
    parser.add_argument('--port', type=int, required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    os.environ['LUNCOSIM_CONFIG'] = str(output / 'config')
    os.environ['LUNCOSIM_EPHEMERAL_SETTINGS'] = '1'
    log = output / 'runtime.log'
    evidence = {'arguments': {**vars(args), 'output': str(output)}, 'snapshots': {}}
    def save_evidence():
        (output / 'validation.json').write_text(json.dumps(evidence, indent=2))
    with ProductionSession(args.port, windowed=True, log_path=log,
                           extra_args=['--scene', args.scene, '--no-throttle']) as session:
        pid = session.process.pid
        if Path(f'/proc/{pid}/cwd').resolve() != ROOT or Path(f'/proc/{pid}/exe').resolve() != BINARY.resolve():
            raise RuntimeError('owned runtime differs from the checkout and production binary')

        def execute(command, **params):
            response = session.post({'type': 'ExecuteCommand', 'command': command, 'params': params})
            if response.get('error'):
                raise RuntimeError(f'{command}: {response}')
            return response.get('data', response)

        def api_id(path):
            return int(execute('RunRhai', code=f'print(find_path({json.dumps(path)}));')['stdout'].strip())

        await_value(lambda: 'name=SIMULATION_STARTED' in log.read_text(), bool,
                    'authored scene startup')
        body, avatar, mounted = await_value(
            lambda: [api_id(path) for path in (args.body_path, args.camera_path, args.mounted_camera_path)],
            lambda ids: all(value > 0 for value in ids), 'scene body and camera identities')
        execute('SetTimeTransport', playing=False)
        execute('SetUserCamera', name=args.camera_path)
        execute('SetCameraLookAt', camera=avatar, eye=args.main_eye, target=args.main_look_at)
        for view in execute('InspectCameraFeeds')['views']:
            execute('RemoveCameraFeed', feed=view['feed'])
        main_raw = await_value(lambda: execute('SceneCameraAudit')['candidates'],
            lambda rows: any(row['usd_path'] == args.camera_path and row['camera_active'] for row in rows),
            'selected main camera')
        main_raw = next(row['entity'] for row in main_raw
                        if row['usd_path'] == args.camera_path and row['camera_active'])

        def settled(status):
            views = status['camera_cover']
            return (views and status['contacts'] and all(row['ready'] for row in status['contacts'])
                    and all(row['ready'] and row['wanted'] > 0 and row['resident'] == row['wanted']
                            and row['drawn'] > 0 for row in views) and status['stream']['pending'] == 0)

        baseline = await_value(lambda: execute('TerrainLodStatus'), settled, 'main terrain cover')
        run_authored_scenario(session, 'lunco://scenarios/tests/camera_terrain_quality_rejection.rhai',
            title='CAMERA TERRAIN QUALITY REJECTION', check_count=6, timeout_s=30)
        main_before = next(row for row in baseline['camera_cover'] if row['camera'] == main_raw)
        source = main_before['source']
        if not 0 < main_before['clearance_m'] <= args.maximum_main_clearance_m:
            raise RuntimeError(f'main fixture is not a low clear view: {main_before}')
        start = execute('QueryEntity', id=body)['position']
        camera_position = execute('QueryEntity', id=avatar)['position']
        if math.dist(start, camera_position) < args.minimum_separation_m:
            raise RuntimeError('main fixture is too close to the rover')
        evidence['snapshots']['baseline'] = baseline
        save_evidence()
        session.capture_screenshot(output / 'main-before-drive.png')

        execute('AcquireControl', target=body, bind_camera=False)
        execute('SetPorts', target=body, producer_id=4795001, seq=0, tick=0,
                writes=[['program_active', 1.0], ['autopilot_enable', 0.0],
                        ['throttle', .35], ['steer', 0.0], ['brake', 0.0]])
        execute('SetTimeTransport', playing=True)
        drive_end = time.monotonic() + args.drive_seconds
        while time.monotonic() < drive_end:
            readiness = execute('GetReadiness')
            if readiness.get('faulted'):
                raise RuntimeError(f'drive faulted: {readiness}')
            time.sleep(.25)
        execute('SetPorts', target=body, producer_id=4795001, seq=0, tick=0,
                writes=[['throttle', 0.0], ['steer', 0.0], ['brake', 1.0]])
        time.sleep(1)
        execute('SetTimeTransport', playing=False)
        end = execute('QueryEntity', id=body)['position']
        current_camera_position = execute('QueryEntity', id=avatar)['position']
        if math.dist(camera_position, current_camera_position) > 0.000001:
            raise RuntimeError('the fixed main camera moved during the physical drive')
        distance = math.dist(start, end)
        if distance < args.minimum_drive_distance_m:
            raise RuntimeError(f'physical drive travelled only {distance}m')
        trail = execute('InspectVehicleTrail', id=body)
        if not any(lane['sample_count'] > 1 and lane['projection'] == 'terrain'
                   and lane.get('error') is None for lane in trail['lanes']):
            raise RuntimeError(f'drive did not publish native terrain contacts: {trail}')
        evidence['drive'] = {'start': start, 'end': end, 'distance_m': distance, 'trail': trail}
        save_evidence()

        def add_feed(label):
            before = {row['camera'] for row in execute('TerrainLodStatus')['camera_cover']}
            feed = execute('CreateCameraFeed')['feed']
            execute('SetCameraFeed', feed=feed, source=mounted, label=label,
                    width=args.feed_width, height=args.feed_height, minimum_scene_items=1)
            status = await_value(lambda: execute('TerrainLodStatus'),
                lambda value: settled(value) and len({row['camera'] for row in value['camera_cover']} - before) == 1,
                'rover feed terrain cover')
            raw = next(row['camera'] for row in status['camera_cover'] if row['camera'] not in before)
            return feed, raw

        retired_feed, retired_raw = add_feed('Retirement probe')
        execute('RemoveCameraFeed', feed=retired_feed)
        await_value(lambda: execute('TerrainLodStatus'),
                    lambda value: all(row['camera'] != retired_raw for row in value['camera_cover']),
                    'retired feed demand removal')
        feed, feed_raw = add_feed('Rover terrain')
        status = execute('TerrainLodStatus')
        evidence['snapshots']['with-feed'] = status
        save_evidence()
        run_authored_scenario(session, 'lunco://scenarios/tests/camera_terrain_view_fidelity.rhai',
            title='CAMERA TERRAIN VIEW FIDELITY', check_count=14,
            params={'camera_raw': main_raw, 'other_camera_raw': feed_raw, 'source_raw': source,
                    'min_spacing_m': args.main_min_spacing_m, 'max_spacing_m': args.main_max_spacing_m,
                    'other_min_spacing_m': args.feed_min_spacing_m, 'other_max_spacing_m': args.feed_max_spacing_m,
                    'require_distinct_depths': args.require_distinct_depths,
                    'maximum_budget_refused': args.maximum_budget_refused,
                    'expected_main_wanted': main_before['wanted'], 'retired_camera_raw': retired_raw}, timeout_s=60)
        session.capture_screenshot(output / 'main-with-rover-feed.png')
        await_value(lambda: execute('InspectCameraFeeds')['views'],
                    lambda rows: any(row['feed'] == feed and row['ready'] for row in rows), 'rendered feed image')
        feed_path = output / 'rover-feed.png'
        started = time.time_ns()
        execute('CaptureCameraFeed', feed=feed, path=str(feed_path), request='terrain-fidelity')
        await_value(lambda: feed_path.exists() and feed_path.stat().st_mtime_ns >= started, bool, 'feed image saved')
        execute('RemoveCameraFeed', feed=feed)
        evidence['snapshots']['retired'] = await_value(lambda: execute('TerrainLodStatus'),
            lambda value: settled(value) and all(row['camera'] != feed_raw for row in value['camera_cover']),
            'final feed demand removal')
        diagnostics = execute('RuntimeDiagnostics')
        if diagnostics['errors'] or diagnostics['warnings']:
            raise RuntimeError(f'runtime diagnostics: {diagnostics}')
        evidence['diagnostics'] = diagnostics
        assert_render_log_clean(log)
    if port_is_open(args.port):
        raise RuntimeError('owned API port was not released')
    save_evidence()
    print('PASS 14 authored view/physics checks, 6 quality rejection checks, native-contact drive and feed retirement; images require visual review')


if __name__ == '__main__':
    main()
