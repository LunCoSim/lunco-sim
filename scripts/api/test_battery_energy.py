#!/usr/bin/env python3
"""Qualify the idealized battery using parameters from an explicit composed USD part."""
import argparse
import hashlib
import json
import os
import time
from pathlib import Path

from runtime import ROOT, ProductionSession, port_is_open, run_authored_scenario

CHECKS = 38
DISCHARGE_VARIABLES = [
    'state_of_charge', 'stored_energy_j', 'absorbed_energy_j', 'heat_energy_j',
    'delivered_energy_j', 'series_loss_j', 'transferred_charge_ah',
    'nameplate_energy_j', 'charge_balance_ah',
]
CYCLE_VARIABLES = [
    'state_of_charge', 'stored_energy_j', 'absorbed_energy_j', 'heat_energy_j',
    'terminal_power_w', 'heat_w',
]
INVALID = [
    'ZeroCapacity', 'ZeroVoltage', 'NegativeResistance', 'NegativeInitialCharge',
    'ExcessInitialCharge', 'ZeroReserve', 'FullReserve',
]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--port', type=int, default=4193)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--scene', type=Path, required=True)
    parser.add_argument('--battery-path', required=True)
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    os.environ['LUNCOSIM_CONFIG'] = str(output / 'config')
    os.environ['LUNCOSIM_EPHEMERAL_SETTINGS'] = '1'
    runs = []
    with ProductionSession(args.port, windowed=True,
                           extra_args=('--scene', str(args.scene.resolve())),
                           log_path=output / 'runtime.log') as session:
        def execute(command, **params):
            response = session.post({'type': 'ExecuteCommand', 'command': command, 'params': params})
            if response.get('error'):
                raise RuntimeError(response)
            return response.get('data', response)

        execute('Open', uri=str(args.scene.resolve()))
        deadline = time.monotonic() + 120
        while True:
            documents = execute('ListOpenDocuments')['open_documents']
            document = next((doc for doc in documents
                             if doc['kind'] == 'usd'
                             and doc.get('origin', {}).get('path') == str(args.scene.resolve())), None)
            readiness = execute('GetReadiness')
            if readiness.get('faulted'):
                raise RuntimeError(readiness)
            viewport = execute('InspectUsdViewport')
            preview = next((p for p in viewport['previews']
                            if document and p['doc_id'] == document['doc_id']), None)
            if (document and preview and preview['projection_ready']
                    and preview['text_ready'] and readiness.get('ready')):
                (output / 'source-viewport.json').write_text(json.dumps(viewport, indent=2))
                break
            if time.monotonic() > deadline:
                raise RuntimeError('The explicitly launched scene did not finish document admission')
            time.sleep(.1)
        snapshot = execute('QueryUsdPrim', doc_id=document['doc_id'], path=args.battery_path,
                           attrs=['inputs:capacity', 'inputs:voltage_nom', 'inputs:R_internal'])
        if (snapshot.get('doc_id') != document['doc_id']
                or snapshot.get('position_frame') != 'canonical_stage'
                or snapshot.get('document_generation') != snapshot.get('generation')):
            raise RuntimeError(f'Battery snapshot has the wrong owner/projection: {snapshot}')
        (output / 'battery-source.json').write_text(json.dumps(snapshot, indent=2))
        attrs = snapshot['attrs']
        fixture = ROOT / 'assets/models/tests/BatteryEnergyContracts.mo'
        # These are structural parameters in the energy witness. Compile exact
        # source-bound specializations; runtime overrides must not silently
        # retain a previous constant-folded energy budget.
        capacity = format(float(attrs['inputs:capacity']), '.17g')
        voltage = format(float(attrs['inputs:voltage_nom']), '.17g')
        resistance = format(float(attrs['inputs:R_internal']), '.17g')
        configured = fixture.read_text()
        for label, value in [('Ideal', '0.0'), ('Lossy', resistance)]:
            configured += (f'\nmodel BatteryEnergy{label}\n'
                           f'  extends BatteryEnergyContracts.Discharge(capacity_ah={capacity}, '
                           f'voltage_v={voltage}, resistance_ohm={value});\n'
                           f'end BatteryEnergy{label};\n')
        (output / 'configured-bench.mo').write_text(configured)
        doc = execute('CreateScratchModelicaDocument', name=fixture.stem,
                      source=configured)['doc_id']
        try:
            deadline = time.monotonic() + 60
            while not execute('CompileStatus', doc_id=doc)['ast_parsed']:
                if time.monotonic() > deadline:
                    raise RuntimeError('Battery contract source did not parse')
                time.sleep(.1)
            cases = [
                ('ideal', 'BatteryEnergyIdeal', 36000., 60., DISCHARGE_VARIABLES),
                ('lossy', 'BatteryEnergyLossy', 36000., 60., DISCHARGE_VARIABLES),
                ('cycle', 'Cycle', 4., .01, CYCLE_VARIABLES),
                ('empty', 'EmptyLoad', 1., .01,
                 ['voltage_v', 'current_a', 'discharge_w', 'stored_j']),
                ('regeneration', 'Regeneration', 1., .01, CYCLE_VARIABLES),
            ]
            cases += [(name, name, 1., .01, []) for name in INVALID]
            for label, target, stop, step, variables in cases:
                run = execute('RunExperiment', doc_id=doc, **{
                    'class': target if target.startswith('BatteryEnergy') else 'BatteryEnergyContracts.' + target,
                    't_end': stop, 'dt': step, 'tolerance': 1e-9,
                    'label': 'Battery energy ' + label,
                })
                runs.append(dict(run, label=label, stop_s=stop, variables=variables,
                                 invalid=label in INVALID))
            (output / 'runs.json').write_text(json.dumps(runs, indent=2))
            execute('SetTimeTransport', playing=True)
            run_authored_scenario(session, 'lunco://scenarios/tests/battery_energy_contracts.rhai',
                                  title='BATTERY ENERGY', check_count=CHECKS, params={'runs': runs})
        finally:
            for run in runs:
                status = execute('RunStatus', experiment_id=run['experiment_id'])
                (output / (run['label'] + '-status.json')).write_text(json.dumps(status, indent=2))
                if status['state'] == 'done':
                    result = execute('GetExperimentResult', experiment_id=run['experiment_id'],
                                     variables=run['variables'], max_points=4000)
                    (output / (run['label'] + '-trajectory.json')).write_text(json.dumps(result, indent=2))
            execute('CloseDocument', doc_id=doc)
    evidence = {
        'checks': CHECKS, 'runs': runs, 'source_snapshot': snapshot,
        'battery_model_sha256': hashlib.sha256(
            (ROOT / 'assets/models/LunCo/Electrical/Battery.mo').read_bytes()).hexdigest(),
        'fixture_sha256': hashlib.sha256(fixture.read_bytes()).hexdigest(),
        'port_released': not port_is_open(args.port),
        'scope': 'synthetic idealized storage qualification; no flight OCV/BMS calibration',
    }
    (output / 'validation.json').write_text(json.dumps(evidence, indent=2))
    assert evidence['port_released']
    print(f'PASS {CHECKS} authored battery checks; owned API Exit and port release')


if __name__ == '__main__':
    main()
