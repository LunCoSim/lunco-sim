// Production grouping policy: exact interfaces, repeated branches and negative facts.
fn candidate(path, source, port_type) {
    #{path:path, role:"prim", type_name:"Xform", programs:["Modelica"],
      program_facts:[#{backend:"Modelica",source:source}],
      ports:[#{name:"inputs:u",kind:"input",type_name:port_type},#{name:"outputs:y",kind:"output",type_name:port_type}]}
}
let nodes = [candidate("/A1","model:A","double"), candidate("/B1","model:B","double"),
             candidate("/A2","model:A","double"), candidate("/B2","model:B","double"),
             candidate("/Shared","model:shared","double")];
let facts = #{nodes:nodes, edges:[#{source:"/A1",target:"/B1",kind:"Dataflow"},#{source:"/A2",target:"/B2",kind:"Dataflow"},
                                #{source:"/A1",target:"/Shared",kind:"Dataflow"},#{source:"/A2",target:"/Shared",kind:"Dataflow"}],scope:"/"};
let checks = []; let plan = invoke_hook("diagram.group.plan", [facts]);
checks.push(t_true(plan.ok,"production grouping policy is installed"));
if plan.ok {
    checks.push(t_true(plan.value.len() == 2 && plan.value[0].members.len() == 2 && plan.value[1].members.len() == 2,"repeated connected branches stay together"));
    let seen = #{}; for group in plan.value { for member in group.members { seen[member] = true; } }
    checks.push(t_true(!seen.contains("/Shared"),"shared singleton stays outside repeated branches"));
    checks.push(t_true(invoke_hook("diagram.group.plan", [facts]).value == plan.value,"group identity and membership are deterministic"));
    let family = invoke_hook("diagram.group.plan", [#{nodes:[nodes[0],nodes[2]],edges:[],scope:"/"}]);
    checks.push(t_true(family.ok && family.value.len() == 1,"identical isolated components form a family"));
    let different = invoke_hook("diagram.group.plan", [#{nodes:[nodes[0],candidate("/Motor2","model:A","float")],edges:[],scope:"/"}]);
    checks.push(t_true(different.ok && different.value.len() == 0,"different port contracts are not grouped by name"));
    let boundary = nodes[2]; boundary.role = "inputs";
    checks.push(t_true(invoke_hook("diagram.group.plan", [#{nodes:[nodes[0],boundary],edges:[],scope:"/"}]).value.len() == 0,"system terminals remain explicit"));
}
let collection = invoke_hook("diagram.group.plan", [#{nodes:nodes,edges:[],collections:[#{id:"collection:/System:actuators",label:"Actuators",members:["/A1","/B1"]}],scope:"/"}]);
checks.push(t_true(collection.ok && collection.value[0].id == "collection:/System:actuators", "standard USD collections take precedence"));
let layout_nodes = []; for node in nodes {node.width=250.0;node.height=96.0;node.rank=0;layout_nodes.push(node);}
let layout_edges=[];for edge in facts.edges {edge.source_port="outputs:y";edge.target_port="inputs:u";layout_edges.push(edge);}
if plan.ok {let groups=[];for group in plan.value {group.collapsed=false;groups.push(group);}
    let grouped=invoke_hook("diagram.layout",[#{nodes:layout_nodes,edges:layout_edges,groups:groups,scope:"/"}]);
    if !grouped.ok {print(grouped);}
    checks.push(t_true(grouped.ok && grouped.value.len()==nodes.len(),"grouped layout retains every source member"));
    let collapsed_groups=[];for group in groups {group.collapsed=true;collapsed_groups.push(group);}
    let collapsed=invoke_hook("diagram.layout",[#{nodes:layout_nodes,edges:layout_edges,groups:collapsed_groups,scope:"/"}]);
    if !collapsed.ok {print(collapsed);}
    checks.push(t_true(collapsed.ok && collapsed.value.len()==nodes.len(),"collapsed layout retains full member placement"));
}
let invalid = invoke_hook("diagram.group.plan", [#{nodes:[#{path:"/Bad"}],edges:[],scope:"/"}]);
checks.push(t_true(!invalid.ok,"malformed grouping facts report a policy error"));
report_verdict(checks,"DIAGRAM GROUPS","DIAGRAM_GROUPS")
