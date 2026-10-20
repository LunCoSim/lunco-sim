//! Read requirement coverage, evidence, and mapped test results in the Editor.
//! Uses the active Twin and the shared guided-tour command lifecycle.

fn steps() {
    [
        #{ anchor: "", focus: (), title: "SysML requirements", body: "Start this tour with a Twin scene and its indexed SysML sources open. The Requirements workspace starts with applied usages. Enable Definitions to include reusable declarations and All columns for source, kind and overall status. This tour explains the screen without running tests or editing your sources." },
        #{ anchor: "panel.sysml_requirements", focus: "sysml_requirements", title: "Find a requirement", body: "Search by ID, name, documentation, or test. A definition describes a reusable requirement; a usage applies it in the model. The first column includes the requirement statement. Click a row to inspect its complete text and evidence. Passed first puts current passing evidence at the top. Choose Column order for an ungrouped list. Click column headings to sort within the selected ordering, drag a dotted handle to reorder, and drag a divider to resize." },
        #{ anchor: "panel.sysml_requirements", focus: "sysml_requirements", title: "Read model coverage", body: "Coverage describes formal require criteria and resolved verify links. A missing formal criterion is model information, not a failed check. Definitions can be verified through their usages. A verify link also needs a Twin scene-test mapping before it can run." },
        #{ anchor: "panel.sysml_requirements", focus: "sysml_requirements", title: "Read requirement evidence", body: "Evidence is the structured result of checks for a requirement: PASS, FAIL, INCONCLUSIVE, UNVERIFIED, or ERROR. NO EVIDENCE means no result is available. STALE means the result belongs to an older source revision. Hover a status for its meaning." },
        #{ anchor: "panel.sysml_requirements", focus: "sysml_requirements", title: "Separate tests from requirements", body: "One test case can verify several requirements. Summary cards count entries in the selected usage/definition scope; run buttons count unique mapped tests. NOT RUN, RUNNING, QUEUED, and CANCELLED describe execution. NO VERIFY means no resolved verify link; NO RUNNER means a link lacks a Twin mapping. NO VERDICT and RUN ERROR describe runner outcomes." },
        #{ anchor: "panel.sysml_requirements", focus: "sysml_requirements", title: "Understand overall status", body: "VERIFIED requires valid source, complete coverage, current passing requirement evidence, and passing mapped tests. INCOMPLETE means some proof is missing; it is not a failure. FAILED means a current check or test failed. INCONCLUSIVE, ERROR, and STALE preserve uncertainty, evaluation errors, and outdated results. This summary does not imply full KerML execution." },
        #{ anchor: "panel.sysml_requirements", focus: "sysml_requirements", title: "Focus the list", body: "Click a summary card to filter matching requirements. The selected card stays highlighted; a zero-count card is disabled. Use the evidence, test, coverage, and source filters to narrow the list. Clear filters restores the list within the selected usage/definition scope. Status details opens the complete counts without expanding the main workspace." },
        #{ anchor: "panel.sysml_requirements", focus: "sysml_requirements", title: "Explore structure and traceability", body: "Traceability follows the selected requirement through its declared subjects, explicit satisfy links, mapped verification and measured evidence. Scroll vertically to inspect every stage. Structure navigates packages, parts, ports and connections. Select an element and filter its Parameters by name, type or value; hover a value for its source expression. Parent and contained-declaration actions follow the actual source hierarchy." },
        #{ anchor: "panel.sysml_requirement_details", focus: "sysml_requirement_details", title: "Inspect and verify", body: "Read the selected requirement statement, its subject, and verification mapping. Source actions open the usage or definition. Expand Engineering trace for relationships. Save or discard source edits before using Run mapped test. Running a test executes its authored scene procedure; a completed run must supply a verdict and current evidence. Read the resulting status and evidence before treating the requirement as verified." },
    ]
}

fn on_start(me, ctx) {
    this.i = 0;
    coach_step(steps(), this.i);
}

fn on_event(me, evt, ctx) {
    let s = steps();
    if evt.name == "cmd:GuidedNext" { this.i += 1; }
    else if evt.name == "cmd:GuidedBack" { if this.i > 0 { this.i -= 1; } }
    else if evt.name == "cmd:GuidedSkip" { this.i = s.len(); }
    else if evt.name == "cmd:GuidedGoto" { this.i = evt.value; }
    else { return; }

    if this.i >= s.len() {
        end_tour();
        notify_kind("SysML requirements tour complete", "success");
        emit("MISSION_COMPLETE", 0);
    } else { coach_step(s, this.i); }
}
