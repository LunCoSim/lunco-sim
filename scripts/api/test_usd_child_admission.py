#!/usr/bin/env python3
"""Exercise initial hierarchy admission through authored production verdicts."""
from __future__ import annotations

import argparse
import json
import sys
import time
from pathlib import Path

from runtime import BINARY, ROOT, ProductionSession, get_json, wait_for_ready
from run_editor_scene_test import wait_for_scene, wait_for_scene_time_selection
from test_native_trim_precision import execute, run


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--port', type=int, required=True)
    parser.add_argument('--log', type=Path, required=True)
    args = parser.parse_args()
    bootstrap = ROOT / 'assets/scenes/tests/twin_session_retirement/replacement/replacement.usda'
    scene = ROOT / 'assets/scenes/tests/usd_child_admission.usda'
    source = (ROOT / 'assets/scenarios/tests/usd_child_admission.rhai').read_text()
    try:
        with ProductionSession(args.port, extra_args=('--no-throttle', '--scene', str(bootstrap)),
                               log_path=args.log, windowed=True) as session:
            pid = session.process.pid
            if sys.platform.startswith('linux') and (
                Path(f'/proc/{pid}/cwd').resolve() != ROOT
                or Path(f'/proc/{pid}/exe').resolve() != BINARY.resolve()
            ):
                raise RuntimeError('session does not match this checkout and binary')
            print(f'OWNED pid={pid} port={args.port} cwd={ROOT}', flush=True)
            wait_for_scene(session, str(bootstrap), 60)
            wait_for_scene_time_selection(session, args.log, 60)
            wait_for_ready(args.port, 60)
            started = execute(session, 'EnginePerformance')['frame_sample_count']
            execute(session, 'OpenFile', path=str(scene))
            doc = None
            deadline = time.monotonic() + 60
            while time.monotonic() < deadline:
                if doc is None:
                    documents = execute(session, 'ListOpenDocuments')['open_documents']
                    doc = next((int(d['doc_id']) for d in documents
                                if d.get('active') and d['kind'] == 'usd'
                                and d.get('origin', {}).get('path') == str(scene)), None)
                if doc is not None:
                    output = run(session, source + f'\nadmission_verify({doc}, {started});')
                    if 'ADMISSION_PENDING' not in output:
                        break
                time.sleep(0.01)
            else:
                raise RuntimeError('initial child admission did not settle')
            print(output, flush=True)
            positive = 'TESTS_OK 3' in output and 'TESTS_FAIL' not in output
            output = run(session, source + f'\nadmission_invalid({doc});')
            print(output, flush=True)
            if not positive or 'TESTS_OK 3' not in output or 'TESTS_FAIL' in output:
                raise RuntimeError('authored child admission verdict failed')
            execute(session, 'OpenTwin', path=str(bootstrap.parent))
            wait_for_scene(session, str(bootstrap), 60)
            wait_for_ready(args.port, 60)
            execute(session, 'OpenFile', path=str(scene))
            deadline = time.monotonic() + 60
            while time.monotonic() < deadline:
                if run(session, source + '\nadmission_pending();').strip() == 'true':
                    break
                time.sleep(0.01)
            else:
                raise RuntimeError('no observed in-progress child admission to cancel')
            print('Observed parent projection with its leaf still pending', flush=True)
            execute(session, 'OpenTwin', path=str(bootstrap.parent))
            wait_for_scene(session, str(bootstrap), 60)
            wait_for_ready(args.port, 60)
            retired = execute(session, 'EnginePerformance')['frame_sample_count']
            deadline = time.monotonic() + 60
            while execute(session, 'EnginePerformance')['frame_sample_count'] - retired < 128:
                if time.monotonic() >= deadline:
                    raise RuntimeError('replacement scene did not advance')
                time.sleep(0.05)
            output = run(session, source + '\nadmission_retired();')
            print(output, flush=True)
            if 'TESTS_OK 3' not in output or 'TESTS_FAIL' in output:
                raise RuntimeError('retired child admission verdict failed')
            args.log.with_suffix('.schema.json').write_text(
                json.dumps(get_json(args.port, '/api/commands/schema')), encoding='utf-8')
    except Exception as error:
        print(f'FAIL — {error}', file=sys.stderr)
        return 1
    print('PASS — nine authored checks; API Exit and port release verified')
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
