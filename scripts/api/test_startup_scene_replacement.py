#!/usr/bin/env python3
"""Replace an explicitly requested startup Twin during native scene preparation."""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import subprocess
import time

from runtime import BINARY, ROOT, ProductionSession, wait_for_ready


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--port', type=int, required=True)
    parser.add_argument('--log-dir', type=Path, required=True)
    args = parser.parse_args()
    evidence = args.log_dir.resolve()
    evidence.mkdir(parents=True, exist_ok=True)
    os.environ.update(LUNCOSIM_EPHEMERAL_SETTINGS='1', LUNCOSIM_ISOLATED_RUN='1',
                      LUNCOSIM_CONFIG=str(evidence / 'config'))
    replacement = ROOT / 'assets/scenes/tests/twin_session_retirement/replacement'
    startup = ROOT / 'assets/scenes/luncosim'
    receipt = {'port': args.port, 'startup': str(startup), 'replacement': str(replacement)}
    log = evidence / 'runtime.log'
    try:
        with ProductionSession(args.port, windowed=True, require_readiness=False,
                               extra_args=['--scene', str(startup / 'sandbox_scene.usda')],
                               log_path=log) as session:
            pid = session.process.pid
            receipt['pid'] = pid
            assert Path(f'/proc/{pid}/cwd').resolve() == ROOT
            assert Path(f'/proc/{pid}/exe').resolve() == BINARY.resolve()
            print(f'OWNED pid={pid} port={args.port}', flush=True)

            def execute(name, **params):
                reply = session.post({'type': 'ExecuteCommand', 'command': name, 'params': params})
                if reply.get('error'):
                    raise RuntimeError((name, reply))
                return reply['data']

            deadline = time.monotonic() + 120
            while time.monotonic() < deadline:
                if session.process.poll() is not None:
                    raise RuntimeError('startup process exited before replacement')
                try:
                    twin = execute('ListTwin', limit=0)
                    ready = execute('GetReadiness')
                except RuntimeError as error:
                    if 'API request failed on port' not in str(error):
                        raise
                    time.sleep(.01)
                    continue
                if twin.get('root') == str(startup) and any(
                        item['kind'] == 'scene_load' for item in ready['pending']):
                    receipt['startup_in_flight'] = ready
                    break
                time.sleep(.01)
            else:
                raise RuntimeError('no in-flight startup scene observed; cancellation not exercised')
            receipt['open_response'] = execute('OpenTwin', path=str(replacement))
            deadline = time.monotonic() + 60
            while time.monotonic() < deadline:
                twin = execute('ListTwin', limit=0)
                if twin.get('root') == str(replacement):
                    ready = execute('GetReadiness')
                    if ready['ready'] and not ready['world_hold']:
                        break
                time.sleep(.025)
            else:
                raise RuntimeError('replacement did not become ready')
            wait_for_ready(args.port)
            # Recheck the replacement after a bounded interval for late results.
            time.sleep(1)
            source = (ROOT / 'assets/scenarios/tests/twin_loading_replacement.rhai').read_text()
            verdict = subprocess.run(
                [str(BINARY), 'rhai', '--api', str(args.port), '--stdout', '-e', source
                 + '\nif !verify_startup_replacement(' + json.dumps(str(replacement))
                 + ') { throw "startup replacement failed"; }'],
                cwd=ROOT, capture_output=True, text=True, timeout=30)
            receipt['verdict'] = verdict.stdout + verdict.stderr
            if (verdict.returncode or 'TESTS_OK 4' not in verdict.stdout
                    or 'Startup Twin replacement: PASS' not in verdict.stdout
                    or 'TESTS_FAIL' in verdict.stdout):
                raise RuntimeError(receipt['verdict'])
            session.capture_screenshot(evidence / 'final.png')
        receipt['app_closed'] = True
        receipt['late_authority_rejection'] = 'document stage input authority changed during preparation' in log.read_text()
        print(receipt['verdict'].strip(), flush=True)
        print('PASS — startup replacement stays usable and closes through its owned API', flush=True)
    except Exception as error:
        receipt['error'] = str(error)
        raise
    finally:
        (evidence / 'receipt.json').write_text(json.dumps(receipt, indent=2) + '\n')


if __name__ == '__main__':
    main()
