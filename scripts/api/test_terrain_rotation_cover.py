#!/usr/bin/env python3
"""Verify stable drawn terrain during two fixed-position yaw sweeps in a real scene."""
from __future__ import annotations

import argparse
import json
import math
import os
import statistics
import time
from pathlib import Path

from PIL import Image, ImageChops, ImageStat
from runtime import BINARY, ROOT, ProductionSession, assert_render_log_clean, run_authored_scenario


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--scene', required=True)
    parser.add_argument('--camera-path', required=True)
    eye = parser.add_mutually_exclusive_group(required=True)
    eye.add_argument('--eye', nargs=3, type=float, help='fixed eye in active physics frame')
    eye.add_argument('--eye-from-path', help='place the fixed eye relative to a live scene entity')
    parser.add_argument('--eye-offset', nargs=3, type=float, default=[4.0, 5.0, 4.0])
    parser.add_argument('--port', type=int, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--cache', type=Path, help='task-owned asset and terrain cache')
    parser.add_argument('--quality', choices=['low', 'balanced', 'high'], default='high')
    parser.add_argument('--windowed', action='store_true', help='use a caller-owned isolated graphical display')
    parser.add_argument('--record-size', default='1600x900', help='physical pixel size in GPU offscreen mode')
    parser.add_argument('--radius-m', type=float, default=20.0)
    parser.add_argument('--max-spacing-m', type=float, required=True)
    parser.add_argument('--radial-radii', nargs='+', type=float, default=[10.0, 20.0, 30.0, 40.0, 60.0])
    parser.add_argument('--look-distance-m', type=float, default=40.0)
    parser.add_argument('--look-drop-m', type=float, default=5.0)
    parser.add_argument('--max-relief-texel-m', type=float, help='require a current local normal page no coarser than this spacing')
    parser.add_argument('--long-range-height-m', type=float, help='raise the camera and verify relief retirement when the site raster suffices')
    parser.add_argument('--mid-range-height-m', type=float, help='raise the camera and verify a coarser but still useful relief level')
    parser.add_argument('--relief-translation-m', type=float, help='move horizontally across the relief-window retention area')
    parser.add_argument('--traverse-distance-m', type=float, help='continuous multi-chunk horizontal camera traversal')
    parser.add_argument('--traverse-seconds', type=float, default=10.0)
    parser.add_argument('--measure-native', action='store_true', help='measure steady and moving native application frame times (requires private windowed output)')
    args = parser.parse_args()
    if not all(math.isfinite(value) and value > 0 for value in
               (args.radius_m, args.max_spacing_m, args.look_distance_m, args.look_drop_m)):
        parser.error('quality and aim distances must be finite and positive')
    if any(not math.isfinite(r) or r <= 0 for r in args.radial_radii) or args.radial_radii != sorted(set(args.radial_radii)):
        parser.error("radial probe radii must be finite, positive and strictly increasing")
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    env = {**os.environ, 'LUNCOSIM_CONFIG': str(output / 'config'),
           'LUNCOSIM_EPHEMERAL_SETTINGS': '1', 'LUNCO_SCREENSHOT_ROOT': str(output)}
    if args.cache is not None:
        env['LUNCOSIM_CACHE'] = str(args.cache.resolve())
    log = output / 'runtime.log'

    def target(angle):
        return [position[0] + args.look_distance_m * math.cos(angle),
                position[1] - args.look_drop_m,
                position[2] + args.look_distance_m * math.sin(angle)]

    presentation_args = [] if args.windowed else ['--offscreen', '--record-size', args.record_size]
    with ProductionSession(args.port, windowed=True, env=env, log_path=log,
                           extra_args=['--scene', args.scene, '--render-quality', args.quality,
                                       '--no-throttle', '--no-vsync', *presentation_args]) as session:
        pid = session.process.pid
        if Path(f'/proc/{pid}/cwd').resolve() != ROOT or Path(f'/proc/{pid}/exe').resolve() != BINARY.resolve():
            raise RuntimeError('owned runtime differs from checkout and production binary')

        def execute(command, **params):
            response = session.post({'type': 'ExecuteCommand', 'command': command, 'params': params})
            if response.get('error'):
                raise RuntimeError(f'{command}: {response}')
            return response['data']

        def api_id(path):
            entity = int(execute('RunRhai', code=f'print(find_path({json.dumps(path)}));')['stdout'].strip())
            if entity <= 0:
                raise RuntimeError(f'fixture entity not found: {path}')
            return entity

        camera = api_id(args.camera_path)
        execute('SetTimeTransport', playing=False)
        position = args.eye
        if args.eye_from_path:
            anchor = execute('QueryEntity', id=api_id(args.eye_from_path))['position']
            position = [a + b for a, b in zip(anchor, args.eye_offset)]
        if len(position) != 3 or not all(math.isfinite(v) for v in position):
            raise RuntimeError(f'invalid fixed eye: {position}')
        execute('SetUserCamera', name=args.camera_path)
        execute('SetCameraLookAt', camera=camera, eye=position, target=target(0))
        if args.windowed:
            for feed in execute('InspectCameraFeeds')['views']:
                execute('RemoveCameraFeed', feed=feed['feed'])
        def settled(require_relief=args.max_relief_texel_m is not None, require_no_relief=False):
            started = time.monotonic()
            geometry_seconds = None
            deadline = started + 180
            while time.monotonic() < deadline:
                status = execute('TerrainLodStatus', include_draw_cover=True)
                # Offscreen owns a separate render mirror of the canonical
                # viewport camera; the single consumed output owns terrain LOD.
                covers = [v for v in status['camera_cover'] if not args.windowed
                          or v['camera'] == status['viewport_camera']]
                candidates = execute('SceneCameraAudit')['candidates']
                active = any(row['usd_path'] == args.camera_path
                             and row['entity'] == status['viewport_camera']
                             and (row['camera_active'] or not args.windowed) for row in candidates)
                geometry_ready = active and len(covers) == 1 and covers[0]['ready'] and status['stream']['pending'] == 0
                if geometry_ready and geometry_seconds is None:
                    geometry_seconds = time.monotonic() - started
                if (geometry_ready and all(r['current'] for r in status['derived']['rasters'])
                        and (not require_relief or (covers[0].get('relief') or {}).get('current'))
                        and (not require_no_relief or covers[0].get('relief') is None)):
                    status['test_settle_seconds'] = {'geometry': geometry_seconds, 'all_visuals': time.monotonic() - started}
                    return status, covers[0]
                time.sleep(.2)
            raise RuntimeError('fixed-eye cover did not settle')

        status, view = settled()
        if not 0 < view['clearance_m'] < args.radius_m:
            raise RuntimeError(f'fixture requires a low clear camera: {view["clearance_m"]}')
        (output / 'before.json').write_text(json.dumps(status, indent=2))
        session.capture_screenshot(output / 'before.png')
        run_authored_scenario(session, 'lunco://scenarios/tests/terrain_rotation_cover.rhai',
                              title='TERRAIN ROTATION COVER', check_count=8,
                              params={'camera_id': camera, 'source': view['source'], 'eye': position,
                                      'viewport_camera': status['viewport_camera'],
                                      'output_camera': view['camera'],
                                      'targets': [target(i * math.pi / 4) for i in range(1, 17)],
                                      'radius_m': args.radius_m, 'max_spacing_m': args.max_spacing_m,
                                      'require_relief': args.max_relief_texel_m is not None,
                                      'max_relief_texel_m': args.max_relief_texel_m or 1.0,
                                      'radial_radii': args.radial_radii,
                                      'position_tolerance_m': .005, 'minimum_heading_s': .25,
                                      'heading_timeout_s': 30.0}, timeout_s=510)
        (output / 'after.json').write_text(json.dumps(execute('TerrainLodStatus', include_draw_cover=True), indent=2))
        session.capture_screenshot(output / 'after.png')
        # Render each cardinal view after the continuous authored gate.
        for degrees in (90, 180, 225, 270):
            execute('SetCameraLookAt', camera=camera, eye=position, target=target(math.radians(degrees)))
            time.sleep(.25)
            status, _ = settled()
            (output / f'yaw-{degrees}.json').write_text(json.dumps(status, indent=2))
            session.capture_screenshot(output / f'yaw-{degrees}.png')
        if args.relief_translation_m is not None:
            # Move away from the actual retained centre, not necessarily +X;
            # a startup window can legitimately cover both ends of a +45 m move.
            origin_status, origin_view = settled()
            direction = -1.0 if origin_view['position_local_m'][0] < origin_view['relief']['center_m'][0] else 1.0
            translated_x = position[0] + direction * args.relief_translation_m
            height = execute('TerrainHeights', points=[[translated_x,position[2]]])[0]
            if not height['found']: raise RuntimeError('translation misses admitted terrain')
            translated_eye = [translated_x, height['height']+origin_view['clearance_m'], position[2]]
            run_authored_scenario(session, 'lunco://scenarios/tests/terrain_relief_translation.rhai',
                                  title='TERRAIN RELIEF TRANSLATION', check_count=8,
                                  params={'camera_id': camera, 'camera': view['camera'], 'source': view['source'],
                                          'eye': translated_eye, 'target': [translated_eye[0], translated_eye[1]-5.0, translated_eye[2]+40.0],
                                          'minimum_distance_m': args.relief_translation_m*.8, 'timeout_s': 90.0}, timeout_s=110)
            translated_status, _ = settled()
            (output / 'translated.json').write_text(json.dumps(translated_status, indent=2))
            session.capture_screenshot(output / 'translated.png')
        if args.traverse_distance_m is not None:
            execute('SetCameraLookAt', camera=camera, eye=position, target=target(0))
            start_status, start_view = settled()
            count = max(16, round(args.traverse_seconds * 10))
            points = [[position[0]+args.traverse_distance_m*(i+1)/count, position[2]] for i in range(count)]
            elevations = execute('TerrainHeights', points=points)
            if len(elevations) != count or not all(h['found'] for h in elevations):
                raise RuntimeError('traversal path requires admitted ground heights')
            eyes = [[x, h['height']+start_view['clearance_m'], z] for (x,z),h in zip(points,elevations)]
            targets = [[x+40.0,y-5.0,z] for x,y,z in eyes]
            run_authored_scenario(session, 'lunco://scenarios/tests/terrain_relief_traverse.rhai',
                                  title='TERRAIN RELIEF TRAVERSE', check_count=5,
                                  params={'camera_id': camera, 'camera': start_view['camera'], 'source': start_view['source'],
                                          'eyes': eyes, 'targets': targets, 'step_seconds': args.traverse_seconds/count,
                                          'minimum_distance_m': args.traverse_distance_m*.8,
                                          'timeout_s': args.traverse_seconds + 90}, timeout_s=args.traverse_seconds+110)
            traversal_status, _ = settled()
            (output / 'traverse.json').write_text(json.dumps(traversal_status, indent=2))
            session.capture_screenshot(output / 'traverse.png')
            execute('SetCameraLookAt', camera=camera, eye=position, target=target(0))
            return_status, _ = settled()
            (output / 'return.json').write_text(json.dumps(return_status, indent=2))
            session.capture_screenshot(output / 'return.png')
            if not args.windowed:
                # The resolved foreground should be independent of atlas origin
                # after returning to the exact same view. Exclude the horizon,
                # where discretionary mesh selection can legitimately differ.
                with Image.open(output / 'before.png') as before, Image.open(output / 'return.png') as returned:
                    width, height = before.size
                    box = (0, int(height * .64), width, height)
                    diff = ImageChops.difference(before.convert('RGB').crop(box), returned.convert('RGB').crop(box))
                    mean = ImageStat.Stat(diff).mean
                    (output / 'return-pixels.json').write_text(json.dumps({'foreground_rgb_mean_absolute_difference': mean, 'crop': box},indent=2))
                    if max(mean) > 0.5:
                        raise RuntimeError(f'foreground changed after returning to the same pose: {mean}')
            if args.measure_native:
                if not args.windowed:
                    raise RuntimeError('native performance measurement requires --windowed')
                (output / 'native-camera-audit.json').write_text(json.dumps(execute('SceneCameraAudit'),indent=2))
                def sample_native(duration, poses=None):
                    first = execute('EnginePerformance')
                    if not first['available']:
                        raise RuntimeError('native frame diagnostics unavailable')
                    start_count=first['frame_sample_count']
                    rows={}
                    started=time.monotonic()
                    cursor=0
                    def capture_frames():
                        engine=execute('EnginePerformance')
                        last=engine['frame_sample_count']; history=engine['frame_time_samples_ms']
                        for n,ms in enumerate(history,last-len(history)+1):
                            if n>start_count: rows[n]=ms
                    next_sample=started
                    while time.monotonic()-started<duration or (poses is not None and cursor<len(poses)):
                        now=time.monotonic()
                        if poses is not None and cursor<len(poses) and now-started>=cursor*duration/len(poses):
                            execute('SetCameraLookAt',camera=camera,eye=poses[cursor],target=targets[cursor])
                            cursor+=1
                        if now>=next_sample:
                            capture_frames();next_sample=now+.5
                        time.sleep(.01)
                    capture_frames();elapsed=time.monotonic()-started
                    ordered=[rows[k] for k in sorted(rows)]
                    if not ordered: raise RuntimeError('no new native frame samples')
                    sorted_ms=sorted(ordered)
                    return {'wall_seconds':elapsed,'samples':len(ordered),'commands':cursor,
                            'mean_fps':1000/statistics.mean(ordered),'p50_ms':statistics.median(ordered),
                            'p95_ms':sorted_ms[math.ceil(.95*len(sorted_ms))-1], 'max_ms':max(ordered),
                            'frame_times_ms':ordered}
                native={'stationary':sample_native(5.0),'motion':sample_native(args.traverse_seconds,eyes)}
                (output / 'native-performance.json').write_text(json.dumps(native,indent=2))
                settled()
        for label, height, expect_relief in [('mid-range',args.mid_range_height_m,True),('long-range',args.long_range_height_m,False)]:
            if height is None: continue
            low_status = json.loads((output / 'before.json').read_text())
            low_view = next(v for v in low_status['camera_cover'] if v['camera'] == view['camera'])
            low_contacts = [[r[k] for k in ('source', 'depth', 'x', 'z', 'resolution', 'min_wavelength_m')]
                            for r in low_status['contacts']]
            high_eye = [position[0], position[1] + height, position[2]]
            execute('SetCameraLookAt', camera=camera, eye=high_eye,
                    target=[position[0]+400.0, position[1]-5.0, position[2]])
            high_status, high_view = settled(require_relief=expect_relief, require_no_relief=not expect_relief)
            (output / f'{label}.json').write_text(json.dumps(high_status, indent=2))
            session.capture_screenshot(output / f'{label}.png')
            run_authored_scenario(session, 'lunco://scenarios/tests/terrain_relief_range.rhai',
                                  title='TERRAIN RELIEF RANGE', check_count=5,
                                  params={'camera': high_view['camera'], 'source': high_view['source'],
                                          'low_spacing_m': low_view['spacing_m'],
                                          'min_clearance_m': height * .9, 'expect_relief': expect_relief,
                                          'low_texel_m': (low_view.get('relief') or {}).get('texel_m', 1.0),
                                          'low_contacts': low_contacts}, timeout_s=30)
        (output / 'performance.json').write_text(json.dumps(execute('EnginePerformance'), indent=2))
        diagnostics = execute('RuntimeDiagnostics')
        (output / 'diagnostics.json').write_text(json.dumps(diagnostics, indent=2))
        if diagnostics['errors'] or diagnostics['warnings']:
            raise RuntimeError(f'runtime diagnostics: {diagnostics}')
        assert_render_log_clean(log)
        pixels = {}
        for name in ('before', 'after', 'yaw-90', 'yaw-180', 'yaw-225', 'yaw-270'):
            with Image.open(output / f'{name}.png') as image:
                if not args.windowed and image.size != tuple(map(int, args.record_size.split('x'))):
                    raise RuntimeError(f'{name} pixels have unexpected size: {image.size}')
                luminance_stddev = ImageStat.Stat(image.convert('L')).stddev[0]
                if luminance_stddev < 2.0:
                    raise RuntimeError(f'{name} may be a clear frame')
                pixels[name] = {'size': image.size, 'luminance_stddev': luminance_stddev}
        (output / 'pixels.json').write_text(json.dumps(pixels, indent=2))


if __name__ == '__main__':
    main()
