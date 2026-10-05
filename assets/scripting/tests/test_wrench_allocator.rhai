// Run WrenchAllocatorContract.mo through RunModelicaSolve; after RunStatus is
// done, provide its experiment_id in the Command/REPL scope for this probe.
// Signed saturation, coupled reconstruction and zero authority are analytical.
let variables=[];
for i in 1..7 {variables.push("orthogonal.command["+i+"]");}
for i in 1..4 {variables.push("coupled.command["+i+"]");}
variables.push("powerless.command[1]");
let result=query("GetExperimentResult",#{experiment_id:experiment_id,variables:variables});
let failures=[];
let expected=[0.25,-0.4,1.0,-1.0,0.0,0.7];
for i in 0..expected.len() {
    let values=result.series[variables[i]];
    let ok=values!=() && values.len()>0;
    if ok {for value in values {ok=ok && value.is_finite() && (value-expected[i]).abs()<1.0e-10;}}
    failures.push(t_true(ok,"orthogonal signed demand / saturation "+(i+1)));
}
let coupled=[];
for i in 6..9 {
    let values=result.series[variables[i]];
    let ok=values!=() && values.len()>0;
    if ok {for value in values {ok=ok && value.is_finite() && value>=0.0 && value<=1.0;}}
    failures.push(t_true(ok,"coupled actuator command bounds "+(i-5)));
    coupled.push(if ok {values[0]} else {()});
}
let reconstruct=coupled[0]!=() && coupled[1]!=() && coupled[2]!=()
    && (coupled[0]+coupled[1]-1.0).abs()<1.0e-10
    && (coupled[1]+coupled[2]-1.0).abs()<1.0e-10;
failures.push(t_true(reconstruct,"coupled demand reconstruction"));
let values=result.series["powerless.command[1]"];
let finite=values!=() && values.len()>0;
if finite {for value in values {finite=finite && value.is_finite() && (value-0.25).abs()<1.0e-10;}}
failures.push(t_true(finite,"zero-authority column preserves its lower bound"));
report_verdict(failures,"WRENCH ALLOCATOR","WRENCH_ALLOCATOR");
