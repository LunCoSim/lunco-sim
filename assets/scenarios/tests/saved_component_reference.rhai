// Observe a saved component and a subsequently opened assembly through their
// exact document owners. Fixture creation and undo use the document journal.
fn simulation_dependencies(me,ctx) {
    #{modelica_entities:[],entity_reads:[],entity_writes:[],
      query_reads:["InspectUsdDocument","QueryUsdPrim"],required_inputs:[]}
}
fn child_names(record) {
    let names=[];
    for path in record.children {names.push(path.split('/').pop());}
    names.sort();names
}
fn on_start(me,ctx) {this.done=false;}
fn on_tick(me,ctx) {
    if this.done {return;}
    let source=query("InspectUsdDocument",#{doc_id:ctx.component_doc});
    let component=query("QueryUsdPrim",#{doc_id:ctx.component_doc,path:ctx.component_root,children:true});
    let assembly=query("QueryUsdPrim",#{doc_id:ctx.assembly_doc,path:ctx.assembly_root,children:true});
    let a=query("QueryUsdPrim",#{doc_id:ctx.component_doc,path:ctx.component_root+"/"+ctx.probe_child,
        attrs:["xformOp:translate"]});
    let b=query("QueryUsdPrim",#{doc_id:ctx.assembly_doc,path:ctx.assembly_root+"/"+ctx.probe_child,
        attrs:["xformOp:translate"]});
    let composed=child_names(assembly);
    let includes_source=component.children.len()>0;
    for name in child_names(component) {if !composed.contains(name) {includes_source=false;}}
    let fails=[t_true(source.dirty==false,"source document has not completed its save"),
        t_true(includes_source,
            "new assembly does not compose the saved component inventory")];
    for axis in 0..3 {
        fails.push(t_true(a.attrs["xformOp:translate"][axis]==b.attrs["xformOp:translate"][axis],
            "new assembly has a stale component translation on axis "+axis));
    }
    this.done=true;
    report_verdict(fails,"SAVED COMPONENT REFERENCE","SAVED_COMPONENT_REFERENCE");
}
