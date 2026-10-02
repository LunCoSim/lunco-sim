fn group_entry(id,label,members) {#{id:id,label:label,members:members}}
// View-only groups from standard collections and exact component/connection contracts.
fn component_signature(node) {
    if !node.contains("role") {throw "Grouping node lacks its interface role";}
    if node.role != "prim" { return ""; }
    let programs=[];for p in node.program_facts {programs.push(p.backend+":"+p.source);}programs.sort();
    let ports=[];for p in node.ports {ports.push(p.kind+":"+p.name+":"+p.type_name);}ports.sort();
    if programs.len()==0 && (node.type_name=="Xform" || node.type_name=="Scope" || node.type_name=="") {return "";}
    node.type_name+"|"+programs.to_string()+"|"+ports.to_string()
}
fn unique_neighbors(path,neighbors,signatures) {
    let result=[];let contracts=neighbors[path];
    for signature in contracts.keys() {
        let candidates=contracts[signature].keys();if candidates.len()!=1 {continue;}
        let other=candidates[0];let reverse=neighbors[other];let matches=reverse[signatures[path]];
        if matches.len()==1 {result.push(other);}
    }
    result
}
fn branch_shape(members,signatures,edges) {
    let shape=[];let member_set=#{};
    for path in members {member_set[path]=true;shape.push(signatures[path]);}
    for edge in edges {
        if !member_set.contains(edge.source) || !member_set.contains(edge.target) {continue;}
        let source_port=if edge.contains("source_port") {edge.source_port} else {""};
        let target_port=if edge.contains("target_port") {edge.target_port} else {""};
        let source=signatures[edge.source]+"."+source_port;let target=signatures[edge.target]+"."+target_port;
        shape.push(source+"|"+edge.kind+"|"+target);
    }
    shape.sort();shape.to_string()
}
fn neighbor_index(edges,signatures,families) {
    let neighbors=#{};for path in signatures.keys() {neighbors[path]=#{};}
    for edge in edges {
        if edge.source==edge.target || !signatures.contains(edge.source) || !signatures.contains(edge.target) {continue;}
        let a=signatures[edge.source];let b=signatures[edge.target];
        if a==b || families[a].len()<2 || families[b].len()<2 {continue;}
        let from=neighbors[edge.source];let to=neighbors[edge.target];
        if !from.contains(b) {from[b]=#{};}if !to.contains(a) {to[a]=#{};}
        from[b][edge.target]=true;to[a][edge.source]=true;
        neighbors[edge.source]=from;neighbors[edge.target]=to;
    }
    neighbors
}
fn visit_branch(seed,neighbors,signatures,used,visited) {
    let queue=[seed];let members=[];visited[seed]=true;let cursor=0;
    while cursor<queue.len() {
        let path=queue[cursor];cursor+=1;members.push(path);
        for other in unique_neighbors(path,neighbors,signatures) {
            if visited.contains(other) {continue;}if used.contains(other) {continue;}
            visited[other]=true;queue.push(other);
        }
    }
    #{members:members,visited:visited}
}
fn branch_candidates(paths,neighbors,signatures,edges,used) {
    let visited=#{};let branches=#{};
    for seed in paths {
        if used.contains(seed) || visited.contains(seed) {continue;}
        let branch=visit_branch(seed,neighbors,signatures,used,visited);visited=branch.visited;let members=branch.members;
        if members.len()<2 {continue;}
        members.sort();let key=branch_shape(members,signatures,edges);
        if !branches.contains(key) {branches[key]=[];}branches[key].push(members);
    }
    branches
}
fn collection_members(collection,nodes,used) {
    let members=[];
    for path in collection.members {
        if !nodes.contains(path) || used.contains(path) {continue;}
        let node=nodes[path];if node.role=="prim" && !members.contains(path) {members.push(path);}
    }
    members.sort();members
}
fn plan_diagram_groups(facts) {
    let signatures=#{};let families=#{};let nodes=#{};let used=#{};let result=[];
    for node in facts.nodes {
        nodes[node.path]=node;let signature=component_signature(node);
        if signature=="" {continue;}
        signatures[node.path]=signature;if !families.contains(signature) {families[signature]=[];}
        families[signature].push(node.path);
    }
    if facts.contains("collections") {
        for collection in facts.collections {
            let members=collection_members(collection,nodes,used);if members.len()<2 {continue;}
            result.push(group_entry(collection.id,collection.label,members));for path in members {used[path]=true;}
        }
    }
    let neighbors=neighbor_index(facts.edges,signatures,families);let paths=signatures.keys();paths.sort();
    let branches=branch_candidates(paths,neighbors,signatures,facts.edges,used);let shapes=branches.keys();shapes.sort();
    for shape in shapes {
        let candidates=branches[shape];if candidates.len()<2 {continue;}
        for members in candidates {
            let seed=members[0];let name=seed.split('/');let label=name[name.len()-1]+" branch";
            let id="branch:"+seed;result.push(group_entry(id,label,members));for path in members {used[path]=true;}
        }
    }
    let keys=families.keys();keys.sort();
    for signature in keys {
        let members=[];for path in families[signature] {if !used.contains(path) {members.push(path);}}
        if members.len()<2 {continue;}members.sort();let node=nodes[members[0]];let label=node.type_name;
        if node.program_facts.len()>0 {let program=node.program_facts[0];let source=program.source.split('/');label=source[source.len()-1];}
        let id="family:"+members[0];result.push(group_entry(id,label,members));
    }
    result
}
