// Run in a windowed production host with the shared guided HUD installed.
fn simulation_dependencies(me,ctx){#{modelica_entities:[],entity_reads:[],entity_writes:[],query_reads:[],required_inputs:[]}}
fn layout(){for card in cmd_data("InspectGuidedHud",#{}).cards{if card.hud=="layout-probe"{return card.layout;}}()}
fn on_start(me,ctx){
    hint("layout-probe","HUD layout acceptance");
    let checks=[];
    checks.push(t_true(cmd("SetGuidedHudLayout",#{hud:"layout-probe",width:480.0,text_scale:1.25,draggable:true}).ok,"configure named HUD layout"));
    let selected=layout();
    checks.push(t_true(selected.width==480.0 && selected.text_scale==1.25 && selected.draggable,"inspection publishes applied presentation settings"));
    checks.push(t_true(!cmd("SetGuidedHudLayout",#{hud:"layout-probe",width:0.0}).ok,"invalid width is rejected"));
    checks.push(t_true(layout().width==480.0,"rejection retains selected layout"));
    checks.push(t_true(!cmd("SetGuidedHudLayout",#{hud:"",text_scale:1.0}).ok,"layout requires explicit named ownership"));
    clear_hint("layout-probe");
    report_verdict(checks,"GUIDED HUD LAYOUT","GUIDED_HUD_LAYOUT");
}
