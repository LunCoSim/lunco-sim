#!/usr/bin/env python3
"""Qualify simulation-core contracts through the production API and authored verdicts."""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import time

from runtime import BINARY, ROOT, ProductionSession, run_authored_scenario


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--port', type=int, default=4883)
    parser.add_argument('--case', action='append', choices=[
        'physics-y', 'physics-z', 'commands', 'inputs', 'parameters', 'invalid-mass', 'mass-units', 'joint-order'])
    args = parser.parse_args()
    selected = args.case or ['physics-y', 'physics-z', 'commands', 'inputs', 'parameters',
                             'invalid-mass', 'mass-units', 'joint-order']
    evidence = ROOT / '.cache/codex-evidence/simulation-core-fixes-20261103/production'
    evidence.mkdir(parents=True, exist_ok=True)
    os.environ.update(LUNCOSIM_ISOLATED_RUN='1', LUNCOSIM_EPHEMERAL_SETTINGS='1')
    results = []
    for case in selected:
        name = {'physics-z': 'simulation_core_contracts_z_up',
                'invalid-mass': 'simulation_core_mass_invalid',
                'mass-units': 'simulation_core_mass_units_invalid',
                'joint-order': 'paused_wheel_effort'}.get(case, 'simulation_core_contracts')
        scene = ROOT / 'assets/scenes/tests' / (name + '.usda')
        source_hash = hashlib.sha256(scene.read_bytes()).hexdigest()
        os.environ['LUNCOSIM_CONFIG'] = str(evidence / (case + '-config'))
        negative = case in ['invalid-mass', 'mass-units', 'joint-order']
        log = evidence / (case + '.log')
        startup_scene = ROOT / 'assets/scenes/tests/simulation_core_contracts.usda' if case == 'mass-units' else scene
        # The existing live-document CompileModel command belongs to the
        # workbench; parameter admission and replacement use the shared core.
        with ProductionSession(args.port, windowed=case == 'parameters',
                               extra_args=['--scene', str(startup_scene)],
                               require_readiness=case != 'invalid-mass', log_path=log) as session:
            pid = session.process.pid
            assert Path(f'/proc/{pid}/cwd').resolve() == ROOT
            assert Path(f'/proc/{pid}/exe').resolve() == BINARY.resolve()

            def command(name, **params):
                response = session.post({'type': 'ExecuteCommand', 'command': name, 'params': params})
                if response.get('error'):
                    raise RuntimeError(response)
                return response.get('data', {})

            def rhai(code):
                return command('RunRhai', code=code)['stdout']

            def wait(predicate, message, seconds=60):
                deadline = time.monotonic() + seconds
                while time.monotonic() < deadline:
                    value = predicate()
                    if value:
                        return value
                    if session.process.poll() is not None:
                        raise RuntimeError(f'{message}: process exited; {log}')
                    time.sleep(.1)
                raise RuntimeError(f'{message}; {log}')

            if case != 'invalid-mass':
                probe = '/World/Physical/Wheel_FL_Hinge' if case == 'joint-order' else '/World/Target'
                wait(lambda: command('GetReadiness').get('ready')
                     and rhai(f'find_path("{probe}")>=0 && clock_snapshot().sim_tick>=2') == 'true',
                     'the requested startup scene did not finish native admission')

            if negative:
                twin = None
                if case == 'mass-units':
                    twin = int(rhai('query("ReadActiveTwinContract").twin_id'))
                    command('LoadScene', path='twin://tests/simulation_core_mass_units_invalid.usda')
                elif case == 'joint-order':
                    twin = int(rhai('query("ReadActiveTwinContract").twin_id'))
                    removed = rhai('remove(find_path("/World/Physical/Wheel_FL_Hinge"),"PhysicsOrderKey")')
                    assert removed == 'true', removed
                def fault():
                    try:
                        ready = command('GetReadiness')
                        return ready if ready.get('faulted') else None
                    except RuntimeError:
                        return None
                ready = wait(fault, 'invalid source did not publish a terminal fault')
                expected = {'invalid-mass': 'usd-reduced-mass-invalid',
                            'mass-units': 'scene-load-failed',
                            'joint-order': 'cosim-joint-effort-order-invalid'}[case]
                detail = {'invalid-mass': 'centerOfMass', 'mass-units': 'kilogramsPerUnit',
                          'joint-order': 'stable order key'}[case]
                run_authored_scenario(session, 'lunco://scenarios/tests/simulation_core_rejection.rhai',
                    title='SIMULATION CORE REJECTION', check_count=5, allow_runtime_fault=True,
                    owner_twin_id=twin,
                    params={'expected_kind': expected, 'detail': detail})
            else:
                command('OpenFile', path=str(scene))
                wait(lambda: any(d.get('origin', {}).get('path') == str(scene)
                     for d in command('ListOpenDocuments')['open_documents']), 'scene document did not open')
                twin = int(rhai('query("ReadActiveTwinContract").twin_id'))
                if case.startswith('physics-'):
                    run_authored_scenario(session, 'lunco://scenarios/tests/simulation_core_physics.rhai',
                        title='SIMULATION CORE PHYSICS', check_count=16, owner_twin_id=twin,
                        params={'up_axis': 'Z' if case == 'physics-z' else 'Y'})
                elif case == 'commands':
                    run_authored_scenario(session, 'lunco://scenarios/tests/command_admission_contracts.rhai',
                        title='COMMAND ADMISSION CONTRACTS', check_count=18, owner_twin_id=twin)
                    for kind in ['mission-epoch-rejected', 'animation-command-rejected']:
                        wait(lambda: any(f'name={kind}, severity=Error' in line
                             for line in log.read_text().splitlines()),
                             f'{kind} did not reach the shared runtime event/status bridge')
                elif case == 'inputs':
                    target = int(rhai('find_path("/World/Target")'))
                    command('SetPorts', target=target, writes=[['mass', 7.0]], producer_id=args.port)
                    wait(lambda: abs(float(rhai('query("GetPort",#{api_id:find_path("/World/Target"),'
                         'name:"mass"}).value')) - 7.0) < 1e-5, 'controller input did not commit')
                    run_authored_scenario(session, 'lunco://scenarios/tests/cosim_input_contracts.rhai',
                        title='COSIM INPUT CONTRACTS', check_count=16, owner_twin_id=twin)
                else:
                    docs = command('ListOpenDocuments')['open_documents']
                    doc = next(d['doc_id'] for d in docs if d.get('origin', {}).get('path') == str(scene))
                    path = '/World/ModelicaProbe'
                    ops = [{'AddPrim': {'edit_target': '@runtime@', 'parent_path': '/World',
                            'name': 'ModelicaProbe', 'type_name': 'Scope', 'reference': None}},
                           {'SetApiSchemas': {'edit_target': '@runtime@', 'path': path,
                            'schemas': ['LunCoProgramAPI']}}]
                    for attr, kind, value in [('info:implementationSource', 'token', '"sourceAsset"'),
                        ('info:sourceAsset', 'asset', '@lunco://models/tests/InitialFeedback.mo@'),
                        ('lunco:program:realtimeSafe', 'bool', 'false'),
                        ('inputs:feedback', 'double', '0'), ('inputs:clockFeedback', 'double', '0'),
                        ('outputs:sample', 'double', '0'), ('outputs:solverTime', 'double', '0'),
                        ('outputs:clockSeen', 'double', '0')]:
                        ops.append({'SetAttribute': {'edit_target': '@runtime@', 'path': path,
                                   'name': attr, 'type_name': kind, 'value': value}})
                    receipt = command('ApplyUsdOps', doc_id=doc, ops=ops)
                    assert receipt['paths'], receipt
                    wait(lambda: any(row['name'] == path and row['status'] == 'Running'
                         for row in command('CosimStatus')['entities']), 'USD Modelica probe did not run')
                    target = int(rhai('find_path("/World/ModelicaProbe")'))
                    run_authored_scenario(session, 'lunco://scenarios/tests/modelica_parameter_contracts.rhai',
                        title='MODELICA PARAMETERS', check_count=12, owner_twin_id=twin,
                        params={'no_document_entity': target})
            diagnostics = command('RuntimeDiagnostics')
            results.append({'case': case, 'log': str(log), 'diagnostics': diagnostics})
        assert hashlib.sha256(scene.read_bytes()).hexdigest() == source_hash
        print(f'PASS {case}; authored verdict, source preservation and API Exit verified', flush=True)
    (evidence / ('results-' + '-'.join(selected) + '.json')).write_text(json.dumps(results, indent=2) + '\n')


if __name__ == '__main__':
    main()
