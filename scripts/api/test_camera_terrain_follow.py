#!/usr/bin/env python3
"""Check terrain navigation, independent physical support, capacity and rebasing."""
from __future__ import annotations

import argparse
import json
import os
import math
import time
from pathlib import Path

from runtime import BINARY, ROOT, ProductionSession, assert_render_log_clean, port_is_open, run_authored_scenario


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--scene', required=True)
    parser.add_argument('--body-path', required=True)
    parser.add_argument('--camera-path', required=True)
    parser.add_argument('--freeflight-scene', default='lunco://scenes/tests/camera_flat_terrain.usda')
    parser.add_argument('--freeflight-camera-path', default='/CameraFlatTerrain/Avatar')
    parser.add_argument('--freeflight-support-path', default='/CameraFlatTerrain/SupportBody')
    parser.add_argument('--freeflight-half-extent-m', type=float, default=500.0)
    parser.add_argument('--port', type=int, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--clearance-tolerance-m', type=float, default=0.005)
    parser.add_argument('--maximum-lateral-altitude-drift-m', type=float, default=0.02,
                        help='declared bound for the fixture lateral corridor')
    parser.add_argument('--quality', choices=['low', 'balanced', 'high'], default='balanced')
    parser.add_argument('--skip-follow', action='store_true', help='run only the standalone terrain-navigation fixtures')
    args = parser.parse_args()
    if not (math.isfinite(args.clearance_tolerance_m) and args.clearance_tolerance_m > 0
            and math.isfinite(args.maximum_lateral_altitude_drift_m)
            and args.maximum_lateral_altitude_drift_m >= 0
            and math.isfinite(args.freeflight_half_extent_m) and args.freeflight_half_extent_m > 0):
        parser.error('clearance tolerance and terrain extent must be positive; lateral drift bound must be non-negative')
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    os.environ['LUNCOSIM_CONFIG'] = str(output / 'config')
    os.environ['LUNCOSIM_EPHEMERAL_SETTINGS'] = '1'
    log = output / 'runtime.log'
    with ProductionSession(args.port, windowed=True, log_path=log,
                           extra_args=['--scene', args.scene, '--no-throttle', '--no-vsync',
                                       '--render-quality', args.quality]) as session:
        pid = session.process.pid
        if Path(f'/proc/{pid}/cwd').resolve() != ROOT or Path(f'/proc/{pid}/exe').resolve() != BINARY.resolve():
            raise RuntimeError('owned runtime does not match this checkout and executable')

        def execute(command, **params):
            response = session.post({'type': 'ExecuteCommand', 'command': command, 'params': params})
            if response.get('error'):
                raise RuntimeError(response)
            return response.get('data', response)

        def run_gate(source, **kwargs):
            try:
                run_authored_scenario(session, source, **kwargs)
            except Exception:
                for command in ('TerrainLodStatus', 'RuntimeDiagnostics', 'GetReadiness'):
                    (output / ('failure-' + command + '.json')).write_text(
                        json.dumps(execute(command), indent=2))
                raise

        def api_id(path):
            return int(execute('RunRhai', code=f'print(find_path({json.dumps(path)}));')['stdout'].strip())

        def settle_camera(path, *, select=True):
            if select:
                execute('SetUserCamera', name=path)
            execute('SetTimeTransport', playing=False)
            candidates = [row for row in execute('SceneCameraAudit')['candidates']
                          if row['usd_path'] == path]
            if len(candidates) != 1:
                raise RuntimeError(f'fixture camera does not have one exact render identity: {path}')
            camera_raw = candidates[0]['entity']
            deadline = time.monotonic() + 120
            while time.monotonic() < deadline:
                status = execute('TerrainLodStatus')
                if (status['viewport_camera'] == camera_raw and status['stream']['pending'] == 0
                        and any(row['camera'] == camera_raw and row['ready']
                                for row in status['camera_cover'])):
                    return
                time.sleep(.15)
            raise RuntimeError(f'camera cover did not settle: {path}')

        def load_camera(scene, path):
            previous = {row['entity'] for row in execute('SceneCameraAudit')['candidates']
                        if row['usd_path'] == path}
            execute('LoadScene', path=scene)
            deadline = time.monotonic() + 120
            while time.monotonic() < deadline:
                readiness = execute('GetReadiness')
                avatar = api_id(path)
                fresh = [row for row in execute('SceneCameraAudit')['candidates']
                         if row['usd_path'] == path and row['entity'] not in previous]
                if readiness['ready'] and not readiness['world_hold'] and avatar > 0 and len(fresh) == 1:
                    settle_camera(path)
                    return avatar
                time.sleep(.15)
            raise RuntimeError(f'fixture did not become ready: {scene}')

        deadline = time.monotonic() + 90
        while time.monotonic() < deadline:
            if 'name=SIMULATION_STARTED' in log.read_text():
                break
            time.sleep(.25)
        else:
            raise RuntimeError('scene scenario did not start')
        if not args.skip_follow:
            target = api_id(args.body_path)
            if target <= 0:
                raise RuntimeError('follow target is unavailable')
            execute('SetUserCamera', name=args.camera_path)
            execute('FollowTarget', target=target)
            execute('SetCameraFollow', distance_m=80.0, height_m=32.0, yaw_rad=0.0, pitch_rad=-0.05)
            settle_camera(args.camera_path, select=False)
            (output / 'runtime-schema.json').write_text(json.dumps(
                session.post({'type': 'DiscoverSchema'}), indent=2))
            run_gate('lunco://scenarios/tests/camera_terrain_follow.rhai',
                     title='CAMERA TERRAIN FOLLOW', check_count=8,
                     params={'clearance_tolerance_m': args.clearance_tolerance_m,
                             'maximum_preparation_wait_s': 15.0}, timeout_s=150)
            (output / 'follow-status.json').write_text(json.dumps(execute('TerrainLodStatus'), indent=2))
            session.capture_screenshot(output / 'follow.png')
            run_gate('lunco://scenarios/tests/camera_rapid_terrain_motion.rhai',
                     title='CAMERA RAPID TERRAIN MOTION', check_count=5,
                     params={'clearance_tolerance_m': args.clearance_tolerance_m}, timeout_s=90)
            (output / 'rapid-status.json').write_text(json.dumps(execute('TerrainLodStatus'), indent=2))
        avatar = load_camera(args.freeflight_scene, args.freeflight_camera_path)
        support = api_id(args.freeflight_support_path)
        deadline = time.monotonic() + 30
        while time.monotonic() < deadline:
            result = execute('RunRhai', code='\n'.join(
                f'print(get({avatar}, "CameraCollisionEnvelope.{field}"));'
                for field in ('radius_m', 'skin_m', 'sweep_tolerance_m')))['stdout']
            values = result.strip().splitlines()
            if len(values) == 3 and all(value != '()' for value in values):
                radius, skin, sweep_tolerance = map(float, values)
                if not all(math.isfinite(value) and value > 0 for value in (radius, skin, sweep_tolerance)):
                    raise RuntimeError(f'invalid camera collision envelope: {values}')
                break
            time.sleep(.1)
        else:
            raise RuntimeError('camera collision envelope did not become available')
        envelope = {'radius_m': radius, 'skin_m': skin, 'sweep_tolerance_m': sweep_tolerance}
        (output / 'camera-envelope.json').write_text(json.dumps(envelope, indent=2))
        run_gate('lunco://scenarios/tests/camera_freeflight_terrain.rhai',
                              title='CAMERA FREEFLIGHT TERRAIN', check_count=11,
                              params={'avatar_id': avatar, 'clearance_m': radius + skin,
                                      'support_id': support, 'source_half_extent_m': args.freeflight_half_extent_m,
                                      'minimum_support_distance_m': 100.0,
                                      'clearance_tolerance_m': max(args.clearance_tolerance_m, 4 * sweep_tolerance),
                                      'maximum_lateral_altitude_drift_m': args.maximum_lateral_altitude_drift_m},
                              timeout_s=90)
        (output / 'freeflight-status.json').write_text(json.dumps(execute('TerrainLodStatus'), indent=2))
        session.capture_screenshot(output / 'freeflight.png')
        avatar = load_camera('lunco://scenes/tests/camera_capacity_terrain.usda', '/CameraCapacityTerrain/Avatar')
        run_gate('lunco://scenarios/tests/camera_terrain_capacity.rhai',
                 title='CAMERA TERRAIN CAPACITY', check_count=12,
                 params={'avatar_id': avatar, 'clearance_m': radius + skin,
                         'pose_tolerance_m': sweep_tolerance,
                         'clearance_tolerance_m': max(args.clearance_tolerance_m, 4 * sweep_tolerance)}, timeout_s=90)
        (output / 'capacity-status.json').write_text(json.dumps(execute('TerrainLodStatus'), indent=2))
        avatar = load_camera('lunco://scenes/tests/camera_rebase_terrain.usda', '/CameraRebaseTerrain/Avatar')
        run_gate('lunco://scenarios/tests/camera_terrain_rebase.rhai',
                 title='CAMERA TERRAIN REBASE', check_count=8,
                 params={'avatar_id': avatar, 'clearance_m': radius + skin,
                         'clearance_tolerance_m': max(args.clearance_tolerance_m, 4 * sweep_tolerance),
                         'maximum_lateral_altitude_drift_m': args.maximum_lateral_altitude_drift_m}, timeout_s=90)
        (output / 'rebase-status.json').write_text(json.dumps(execute('TerrainLodStatus'), indent=2))
        session.capture_screenshot(output / 'rebase.png')
        policy_root = output / 'policy-twin'
        policy_name = 'CameraTerrainPolicyProbe'
        if (policy_root / 'twin.toml').exists():
            execute('OpenTwin', path=str(policy_root))
        else:
            execute('CreateTwin', path=str(policy_root), name=policy_name, default_scene='')
        deadline = time.monotonic() + 90
        while time.monotonic() < deadline:
            if execute('RunRhai', code='print(twin_root());')['stdout'].strip() == str(policy_root):
                break
            time.sleep(.15)
        else:
            raise RuntimeError('isolated camera policy Twin did not open')
        avatar = load_camera(args.freeflight_scene, args.freeflight_camera_path)
        run_gate('lunco://scenarios/tests/camera_terrain_policy.rhai',
                 title='CAMERA TERRAIN POLICY', check_count=6,
                 params={'avatar_id': avatar, 'twin_source_identity':
                         execute('RunRhai', code='print(twin_source_identity());')['stdout'].strip()}, timeout_s=90)
        (output / 'policy-status.json').write_text(json.dumps(execute('TerrainLodStatus'), indent=2))
        zoom_path = '/CameraZoomTerrain/Avatar'
        load_camera('lunco://scenes/tests/camera_zoom_terrain.usda', zoom_path)
        wide = execute('TerrainLodStatus')
        wide_view = next(row for row in wide['camera_cover'] if row['camera'] == wide['viewport_camera'])
        (output / 'zoom-wide-status.json').write_text(json.dumps(wide, indent=2))
        session.capture_screenshot(output / 'zoom-wide.png')
        load_camera('lunco://scenes/tests/camera_zoom_terrain_narrow.usda', zoom_path)
        run_gate('lunco://scenarios/tests/camera_terrain_zoom.rhai',
                 title='CAMERA TERRAIN ZOOM', check_count=8,
                 params={'wide_spacing_m': wide_view['spacing_m'], 'wide_depth': wide_view['depth'],
                         'wide_position_m': wide_view['position_local_m'],
                         'wide_contacts': [[row[key] for key in ('depth', 'x', 'z', 'resolution', 'min_wavelength_m')]
                                           for row in wide['contacts']]}, timeout_s=60)
        (output / 'zoom-narrow-status.json').write_text(json.dumps(execute('TerrainLodStatus'), indent=2))
        session.capture_screenshot(output / 'zoom-narrow.png')
        diagnostics = execute('RuntimeDiagnostics')
        if diagnostics['errors'] or diagnostics['warnings']:
            raise RuntimeError(f'runtime diagnostics: {diagnostics}')
        assert_render_log_clean(log)
    if port_is_open(args.port):
        raise RuntimeError('owned API port was not released')
    print('PASS camera terrain follow, free flight, capacity, rebasing, policy and zoom with owned Exit', flush=True)


if __name__ == '__main__':
    main()
