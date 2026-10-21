#!/usr/bin/env python3
"""Check saved USD component publication using exact documents in an owned session."""
import argparse
import hashlib
import json
import sys
import time
from pathlib import Path

from runtime import BINARY, ROOT, ProductionSession, assert_render_log_clean, port_is_open, run_authored_scenario


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--port', type=int, required=True)
    parser.add_argument('--twin', type=Path, required=True)
    parser.add_argument('--component', required=True)
    parser.add_argument('--assembly', required=True)
    parser.add_argument('--component-root', required=True)
    parser.add_argument('--assembly-root', required=True)
    parser.add_argument('--probe-child', required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=False)
    twin = args.twin.resolve()
    component = (twin / args.component).resolve()
    assembly = (twin / args.assembly).resolve()
    report = {'binary_sha256': hashlib.file_digest(BINARY.open('rb'), 'sha256').hexdigest()}
    log = args.output / 'runtime.log'
    with ProductionSession(args.port, windowed=True, extra_args=['--no-throttle', '--no-vsync'], log_path=log) as session:
        pid = session.process.pid
        if sys.platform.startswith('linux') and (Path(f'/proc/{pid}/cwd').resolve() != ROOT or Path(f'/proc/{pid}/exe').resolve() != BINARY.resolve()):
            raise RuntimeError('Runtime does not belong to this checkout')
        print(f'OWNED pid={pid} port={args.port}', flush=True)
        report.update(pid=pid, port=args.port)

        def execute(name, params=None):
            result = session.post({'type': 'ExecuteCommand', 'command': name, 'params': params or {}})
            if result.get('error'):
                raise RuntimeError(f'{name}: {result}')
            return result.get('data')

        def wait_ready():
            end = time.monotonic() + 120
            while True:
                ready = execute('GetReadiness')
                cosim = execute('CosimStatus', {'include_values': False})
                if ready.get('faulted'):
                    raise RuntimeError(ready)
                if ready['ready'] and not ready['world_hold'] and cosim['entity_count'] > 0 and cosim['synchronization']['topology_ready']:
                    return
                if time.monotonic() > end:
                    raise RuntimeError(f'Twin did not become ready: {ready}')
                time.sleep(.2)

        def open_preview(path):
            execute('OpenFile', {'path': str(path)})
            end = time.monotonic() + 90
            while True:
                docs = execute('ListOpenDocuments')['open_documents']
                found = [d for d in docs if d['kind'] == 'usd' and d['origin'].get('path') == str(path)]
                if len(found) == 1:
                    doc = found[0]['doc_id']
                    previews = execute('InspectUsdViewport')['previews']
                    match = [p for p in previews if p['doc_id'] == doc]
                    if len(match) == 1 and match[0]['projection_ready'] and match[0]['text_ready']:
                        return doc
                if time.monotonic() > end:
                    raise RuntimeError(f'Preview did not admit {path}')
                time.sleep(.15)

        def settled(doc):
            end = time.monotonic() + 60
            while True:
                state = execute('InspectUsdDocument', {'doc_id': doc})
                previews = execute('InspectUsdViewport')['previews']
                match = [p for p in previews if p['doc_id'] == doc]
                if len(match) == 1 and match[0]['projection_ready'] and match[0]['text_ready'] and match[0]['projected_generation'] == state['generation']:
                    return state
                if time.monotonic() > end:
                    raise RuntimeError(f'Projection did not settle for {doc}')
                time.sleep(.15)

        execute('OpenTwin', {'path': str(twin)})
        wait_ready()
        execute('SetTimeTransport', {'playing': False})
        execute('ActivatePerspective', {'id': 'editor'})
        doc = open_preview(component)
        original = execute('QueryUsdPrim', {'doc_id': doc, 'path': args.component_root, 'children': True})
        probe = execute('QueryUsdPrim', {'doc_id': doc, 'path': args.component_root + '/' + args.probe_child, 'attrs': ['xformOp:translate']})
        report.update(component_doc=doc, original=original, probe=probe)
        edited = False
        try:
            # The pure Rhai planner supplies standard typed document operations.
            code = '''fn fixture_plan(doc,root,probe) {
                let current=query("QueryUsdPrim",#{doc_id:doc,path:root+"/"+probe,attrs:["xformOp:translate"]});
                let value=current.attrs["xformOp:translate"];
                if value==() || value.len()!=3 {throw("fixture requires a translated source child");}
                value[1]+=0.01;
                [#{AddPrim:#{edit_target:"@root@",parent_path:root,name:"SavedSourceFixture",type_name:"Cube",reference:(),reference_prim_path:()}},
                 #{SetTranslate:#{edit_target:"@root@",path:root+"/"+probe,value:value}}]
            }
            let ops=fixture_plan(DOC,ROOT,PROBE);
            let before=assembly_edit::describe(DOC);
            let result=assembly_edit::batch(DOC,"Exercise saved component publication",ops,before.generation);
            if result.ok!=true {throw(result);} print(result);'''
            code = code.replace('DOC', str(doc)).replace('ROOT', json.dumps(args.component_root)).replace('PROBE', json.dumps(args.probe_child))
            report['apply'] = execute('RunRhai', {'code': code})
            edited = True
            settled(doc)
            execute('SaveDocument', {'doc_id': doc})
            saved = settled(doc)
            if saved['dirty']:
                raise RuntimeError('Component save did not finish')
            target_doc = open_preview(assembly)
            report['assembly_doc'] = target_doc
            report['saved_component'] = execute('QueryUsdPrim', {'doc_id': doc, 'path': args.component_root, 'children': True})
            report['new_assembly'] = execute('QueryUsdPrim', {'doc_id': target_doc, 'path': args.assembly_root, 'children': True})
            execute('SetTimeTransport', {'playing': True})
            run_authored_scenario(session, 'lunco://scenarios/tests/saved_component_reference.rhai',
                                  title='SAVED COMPONENT REFERENCE', check_count=5, timeout_s=30,
                                  params={'component_doc': doc, 'assembly_doc': target_doc,
                                          'component_root': args.component_root, 'assembly_root': args.assembly_root,
                                          'probe_child': args.probe_child})
            report['verdict'] = 'PASS'
        finally:
            execute('SetTimeTransport', {'playing': False})
            if edited:
                execute('UndoDocument', {'doc_id': doc})
                settled(doc)
                execute('SaveDocument', {'doc_id': doc})
                state = settled(doc)
                restored = execute('QueryUsdPrim', {'doc_id': doc, 'path': args.component_root, 'children': True})
                restored_probe = execute('QueryUsdPrim', {'doc_id': doc, 'path': args.component_root + '/' + args.probe_child, 'attrs': ['xformOp:translate']})
                if state['dirty'] or restored['children'] != original['children'] or restored_probe['attrs']['xformOp:translate'] != probe['attrs']['xformOp:translate']:
                    raise RuntimeError('Saved fixture was not restored through the document journal')
                report['restored'] = True
            (args.output / 'report.json').write_text(json.dumps(report, indent=2) + '\n')
        assert_render_log_clean(log)
    if port_is_open(args.port):
        raise RuntimeError('Owned API port remained open after Exit')


if __name__ == '__main__':
    main()
