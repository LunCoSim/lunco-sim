"""Verify Twin binding admission, retirement and invalid input/UI through production."""
import argparse
import json
import shutil
import time
from pathlib import Path
from runtime import ROOT, BINARY, ProductionSession, assert_render_log_clean


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--port', type=int, required=True)
    parser.add_argument('--log', type=Path, default=Path('/tmp/twin-presentation-profiles.log'))
    args = parser.parse_args()
    sources = ROOT / 'assets/scenes/tests/twin_presentation_profiles'
    fixture = ROOT / 'target' / f'twin-presentation-profiles-{args.port}'
    for source in sources.rglob('*'):
        if not source.is_file(): continue
        relative = source.relative_to(sources)
        target = fixture / relative
        if target.suffix == '.fixture': target = target.with_suffix('')
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(source, target)
    with ProductionSession(args.port, windowed=True, extra_args=('--no-throttle', '--no-vsync'), log_path=args.log) as session:
        pid = session.process.pid
        assert Path(f'/proc/{pid}/cwd').resolve() == ROOT
        assert Path(f'/proc/{pid}/exe').resolve() == BINARY.resolve()
        def q(command, **params):
            result = session.post(dict(type='ExecuteCommand', command=command, params=params))
            if result.get('error'): raise RuntimeError(result)
            return result['data']
        def rhai(code):
            result = q('RunRhai', code=code)
            if result.get('error'): raise RuntimeError(result)
            return result['stdout']
        def verdict(call, title, count, fault=False):
            source = (ROOT / 'assets/scenarios/tests/twin_presentation_profiles.rhai').read_text()
            output = rhai(source + '\n' + call + ';')
            if f'TESTS_OK {count}' not in output or f'{title}: PASS' not in output:
                raise RuntimeError(f'Authored verdict failed: {output}')
            print(output.strip(), flush=True)
        def open_fixture(name, fault=None):
            previous = rhai('twin_name()')
            q('OpenTwin', path=str(fixture / name))
            deadline = time.monotonic() + 45
            while time.monotonic() < deadline:
                state = q('GetReadiness')
                if rhai('twin_name()') != previous:
                    if fault and state['faulted'] and state['fault']['kind'] == fault: return
                    if not fault and state['ready']: return
                time.sleep(.1)
            raise RuntimeError(f'{name}: {state}')
        # Wait for the asynchronous shared defaults publication before capturing preferences.
        deadline = time.monotonic() + 30
        while rhai('input_binding("forward")') == '()':
            if time.monotonic() > deadline: raise RuntimeError('application bindings did not publish')
            time.sleep(.1)
        before = rhai('get_setting("InputBindingsSettings.bindings")')
        expected = {name: rhai(f'input_binding({json.dumps(name)})') for name in ('speed_boost', 'pause', 'forward')}
        def map_literal(values):
            return '#{' + ','.join(json.dumps(k) + ':' + json.dumps(v) for k,v in values.items()) + '}'
        for _ in range(2):
            open_fixture('valid')
            time.sleep(.15)
            verdict('restored(#{speed_boost:"B",pause:"ControlLeft + P",forward:' + json.dumps(expected['forward']) + '})', 'TWIN INPUT RESTORATION', 3)
            verdict('preferences(' + before + ')', 'TWIN INPUT PREFERENCES', 1)
            q('SetTimeTransport', playing=False)
            open_fixture('plain')
            time.sleep(.15)
            verdict('restored(' + map_literal(expected) + ')', 'TWIN INPUT RESTORATION', 3)
        for name, kind in [('invalid_input', 'input-profile-invalid'), ('duplicate_input', 'input-profile-invalid'), ('invalid_ui', 'runtime-ui-manifest-invalid')]:
            open_fixture(name, kind)
            tick = rhai('clock_snapshot().sim_tick')
            time.sleep(.2)
            verdict('rejected(' + json.dumps(kind) + ',' + tick + ')', 'TWIN PRESENTATION REJECTION', 2, True)
            open_fixture('plain')
        verdict('preferences(' + before + ')', 'TWIN INPUT PREFERENCES', 1)
        assert_render_log_clean(args.log)
    print('PASS: Twin reopen, paused replacement, preference preservation, three negative profiles, Exit and port release')


if __name__ == '__main__':
    main()
