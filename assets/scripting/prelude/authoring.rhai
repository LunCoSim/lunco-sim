// Script-first robot and mission authoring.
//
// This file is deliberately a thin policy layer over the existing typed command
// and query surfaces. USD remains the authority for scene topology, placement,
// joints and metadata. Modelica remains the authority for equations and
// connections. Every document edit takes an explicit document id so a script
// cannot silently write to whichever editor tab happens to be active.

fn __missing_document(kind) {
    #{ ok: false, error: "missing explicit " + kind + " document id" }
}

/// Spawn a catalog entry. Catalog spawning is the authoritative replicated
/// path; callers do not fabricate ECS entities or USD runtime identities.
fn spawn_robot(entry_id, position) {
    cmd("SpawnEntity", #{ entry_id: entry_id, position: position })
}

// ── Modelica graph authoring ───────────────────────────────────────────────

fn modelica_apply(doc, ops) {
    if doc == () { return __missing_document("Modelica"); }
    cmd("ApplyModelicaOps", #{ doc_id: doc, ops: ops })
}

/// Apply one Modelica batch against an inspected generation. This is the
/// preferred editor path; the two-argument form above remains for authored
/// scripts that intentionally do not use optimistic concurrency.
fn modelica_apply_against(doc, ops, parent_generation) {
    if doc == () || doc.to_string() == "0" { return __missing_document("Modelica"); }
    cmd("ApplyModelicaOps", #{
        doc_id: doc,
        ops: ops,
        parent_generation: parent_generation,
    })
}

// Each constructor below returns one `ApiOp` payload. Keep these constructors
// in Rhai so authored tools and tutorials can describe Modelica edits without
// depending on Rust enum constructors. `modelica_apply` remains the only
// submission path, preserving one journal/undo transaction for a batch.

fn modelica_noop() { "Noop" }

fn modelica_replace_source(source) {
    #{ ReplaceSource: #{ source: source } }
}

fn modelica_edit_text(range_start, range_end, replacement) {
    #{ EditText: #{
        range_start: range_start,
        range_end: range_end,
        replacement: replacement,
    }}
}

fn modelica_placement(x, y, width, height) {
    #{ x: x, y: y, width: width, height: height }
}

fn modelica_modification(name, value) { #{ name: name, value: value } }

fn modelica_add_component(class, type_name, name, placement, modifications) {
    #{ AddComponent: #{
        class: class,
        type_name: type_name,
        name: name,
        modifications: modifications,
        placement: placement,
    }}
}

fn modelica_remove_component(class, name) {
    #{ RemoveComponent: #{ class: class, name: name } }
}

fn modelica_connect(class, from_component, from_port, to_component, to_port) {
    #{ AddConnection: #{
        class: class,
        from_component: from_component,
        from_port: from_port,
        to_component: to_component,
        to_port: to_port,
        line_points: [],
    }}
}

fn modelica_disconnect(class, from_component, from_port, to_component, to_port) {
    #{ RemoveConnection: #{
        class: class,
        from_component: from_component,
        from_port: from_port,
        to_component: to_component,
        to_port: to_port,
    }}
}

fn modelica_set_connection_line(class, from_component, from_port, to_component, to_port, line_points) {
    #{ SetConnectionLine: #{
        class: class,
        from_component: from_component,
        from_port: from_port,
        to_component: to_component,
        to_port: to_port,
        line_points: line_points,
    }}
}

fn modelica_set_connection_line_style(class, from_component, from_port, to_component, to_port, color, thickness, smooth_bezier) {
    #{ SetConnectionLineStyle: #{
        class: class,
        from_component: from_component,
        from_port: from_port,
        to_component: to_component,
        to_port: to_port,
        color: color,
        thickness: thickness,
        smooth_bezier: smooth_bezier,
    }}
}

fn modelica_reverse_connection(class, from_component, from_port, to_component, to_port) {
    #{ ReverseConnection: #{
        class: class,
        from_component: from_component,
        from_port: from_port,
        to_component: to_component,
        to_port: to_port,
    }}
}

fn modelica_set_placement(class, name, placement) {
    #{ SetPlacement: #{ class: class, name: name, placement: placement } }
}

fn modelica_set_parameter(class, component, param, value) {
    #{ SetParameter: #{
        class: class, component: component, param: param, value: value
    }}
}

fn modelica_add_plot_node(class, signal, title, x1, y1, x2, y2) {
    #{ AddPlotNode: #{
        class: class,
        signal: signal,
        title: title,
        x1: x1,
        y1: y1,
        x2: x2,
        y2: y2,
    }}
}

fn modelica_remove_plot_node(class, signal) {
    #{ RemovePlotNode: #{ class: class, signal: signal } }
}

fn modelica_set_plot_node_extent(class, signal, x1, y1, x2, y2) {
    #{ SetPlotNodeExtent: #{
        class: class,
        signal: signal,
        x1: x1,
        y1: y1,
        x2: x2,
        y2: y2,
    }}
}

fn modelica_set_plot_node_title(class, signal, title) {
    #{ SetPlotNodeTitle: #{ class: class, signal: signal, title: title } }
}

fn modelica_set_diagram_text_extent(class, index, x1, y1, x2, y2) {
    #{ SetDiagramTextExtent: #{
        class: class,
        index: index,
        x1: x1,
        y1: y1,
        x2: x2,
        y2: y2,
    }}
}

fn modelica_set_diagram_text_string(class, index, text) {
    #{ SetDiagramTextString: #{ class: class, index: index, text: text } }
}

fn modelica_remove_diagram_text(class, index) {
    #{ RemoveDiagramText: #{ class: class, index: index } }
}

fn modelica_add_class(parent, name, kind, description, partial) {
    #{ AddClass: #{
        parent: parent,
        name: name,
        kind: kind,
        description: description,
        partial: partial,
    }}
}

fn modelica_remove_class(qualified) {
    #{ RemoveClass: #{ qualified: qualified } }
}

fn modelica_add_short_class(parent, name, kind, base, prefixes, modifications) {
    #{ AddShortClass: #{
        parent: parent,
        name: name,
        kind: kind,
        base: base,
        prefixes: prefixes,
        modifications: modifications,
    }}
}

fn modelica_add_variable(class, name, type_name, causality, variability, flow, modifications, value, description) {
    #{ AddVariable: #{
        class: class,
        name: name,
        type_name: type_name,
        causality: causality,
        variability: variability,
        flow: flow,
        modifications: modifications,
        value: value,
        description: description,
    }}
}

fn modelica_remove_variable(class, name) {
    #{ RemoveVariable: #{ class: class, name: name } }
}

fn modelica_add_equation(class, lhs, rhs) {
    #{ AddEquation: #{ class: class, lhs: lhs, rhs: rhs } }
}

fn modelica_rectangle(x1, y1, x2, y2, line_color, fill_color, fill_pattern) {
    #{ kind: "Rectangle", x1: x1, y1: y1, x2: x2, y2: y2,
       line_color: line_color, fill_color: fill_color, fill_pattern: fill_pattern }
}

fn modelica_polygon(points, line_color, fill_color, fill_pattern) {
    #{ kind: "Polygon", points: points,
       line_color: line_color, fill_color: fill_color, fill_pattern: fill_pattern }
}

fn modelica_line(points, color, thickness, pattern) {
    #{ kind: "Line", points: points, color: color, thickness: thickness, pattern: pattern }
}

fn modelica_text(x1, y1, x2, y2, text, color, font_size) {
    #{ kind: "Text", x1: x1, y1: y1, x2: x2, y2: y2,
       text: text, color: color, font_size: font_size }
}

fn modelica_add_icon_graphic(class, graphic) {
    #{ AddIconGraphic: #{ class: class, graphic: graphic } }
}

fn modelica_add_diagram_graphic(class, graphic) {
    #{ AddDiagramGraphic: #{ class: class, graphic: graphic } }
}

fn modelica_set_experiment(class, start_time, stop_time, tolerance, interval) {
    #{ SetExperimentAnnotation: #{
        class: class,
        start_time: start_time,
        stop_time: stop_time,
        tolerance: tolerance,
        interval: interval,
    }}
}

// ── Durable mission checkpoints ────────────────────────────────────────────

// Scenario scripts are attached to host entities, so checkpoint state is
// authored on that host prim. The mission supplies its USD document explicitly
// through `fn mission_document(me)`. This keeps writes correct for referenced
// prims and works with the existing typed command surface without a Rust rebuild.
// Empty string is the logical tombstone used by mission_checkpoint_clear.
fn mission_checkpoint(me, doc, edit_target, key, value) {
    let path = usd_path(me);
    if path == () || doc == () { return false; }
    let result = cmd("ApplyUsdOp", #{
        doc_id: doc,
        op: #{ SetAttribute: #{
            edit_target: edit_target,
            path: path,
            name: "lunco:mission:checkpoint:" + key,
            type_name: "string",
            value: value,
        }},
    });
    result.ok == true
}

fn mission_checkpoint_read(me, key) {
    let path = usd_path(me);
    if path == () { return (); }
    let attr = "lunco:mission:checkpoint:" + key;
    let result = query("QueryUsdPrim", #{ path: path, attrs: [attr] });
    if result == () || result.attrs == () || result.attrs[attr] == () { return (); }
    if result.attrs[attr] == "" { return (); }
    result.attrs[attr]
}

fn mission_checkpoint_clear(me, doc, edit_target, key) {
    mission_checkpoint(me, doc, edit_target, key, "")
}
