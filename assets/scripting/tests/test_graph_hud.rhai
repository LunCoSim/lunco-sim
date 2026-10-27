fn hud_report(id) {
    for hud in query("InspectGraphHud", #{}).huds {
        if hud.hud.to_string() == id.to_string() { return hud; }
    }
    throw "Required HUD is unavailable: " + id;
}

// Production assertions over the same native grid, command and history owners.
fn hud_channel(name) {
    for channel in query("ListTelemetryChannels", #{name: name}).channels {
        if channel.active && channel.source != () && channel.samples.to_float() > 2.0 { return channel; }
    }
    throw "HUD gate requires an active sampled channel: " + name;
}
fn graph_hud_empty_prepare() {
    t_report([t_true(cmd("SetGraphHud", #{visible: true}).ok, "open an empty graph HUD")]);
}
fn graph_hud_empty_rendered() {
    let hud = hud_report(1);
    t_report([t_true(hud.rendered && hud.signals.len() == 0 &&
                  hud.width >= 320.0 && hud.height >= 240.0,
                  "empty HUD retains its native minimum dimensions"),
              t_true(cmd("SetGraphHud", #{visible: false}).ok &&
                  cmd("SetGraphHud", #{visible: true}).ok,
                  "empty HUD can hide and show after rendering"),
              t_true(cmd("RemoveGraphHud", #{hud: 1}).ok, "close the empty HUD")]);
}
fn graph_hud_prepare(signal, secondary) {
    let checks = [];
    let initial = query("InspectGraphHud", #{});
    checks.push(t_true(initial.huds.len() == 0, "scene starts without graph HUDs"));
    let documents = query("ListOpenDocuments", #{}).count;
    let tabs = query("InspectWorkbench", #{}).docked_instance_tabs.len();
    let first = hud_channel(signal);
    let second = hud_channel(secondary);
    let a = #{target_gid: first.source, signal: first.name, caption: "Battery charge"};
    let b = #{target_gid: second.source, signal: second.name, caption: "Battery power"};
    let history = query("QueryTelemetryHistory", #{key: first.key, limit: 10});
    checks.push(t_true(cmd("SetGraphHud", #{signals: [a], visible: true}).ok,
        "Twin can choose a measured parameter and activate the HUD"));
    let one = hud_report(1);
    checks.push(t_true(one.visible && one.signals.len() == 1 && one.signals[0].signal == signal,
        "HUD uses the explicit parameter"));
    checks.push(t_true(!one.signals[0].show_controls && one.signals[0].compact_layout &&
        one.signals[0].line_width >= 3.0, "presentation uses compact native plots and clear strokes"));
    checks.push(t_true(cmd("ConfigureLinePlot", #{plot: one.plots[0], line_width: 4.0,
        time_window_s: 30.0, text_size: 18.0}).ok, "ordinary native appearance controls remain available"));
    checks.push(t_true(cmd("SetGraphHud", #{signals: [a, b], columns: 2}).ok,
        "choose several parameters in one atomic request"));
    let shown = hud_report(1);
    checks.push(t_true(shown.signals.len() == 2 && shown.plots[0] != shown.plots[1] &&
        shown.columns.to_float() == 2.0, "parameters have separate native plots"));
    checks.push(t_true(shown.signals[0].unit == first.unit && shown.signals[1].unit == second.unit &&
        first.unit != second.unit, "different quantities retain their measured units"));
    checks.push(t_true(shown.plots[0] == one.plots[0] && shown.signals[0].line_width == 4.0 &&
        shown.signals[0].time_window_s == 30.0, "changing selection preserves retained plot identity and style"));
    checks.push(t_true(shown.signals[0].series_color != shown.signals[1].series_color,
        "different parameters have distinct theme colours"));
    checks.push(t_true(shown.signals[0].time_window_s == shown.signals[1].time_window_s,
        "added parameters inherit the selected history window"));
    let missing = #{target_gid: first.source, signal: "__missing_hud_parameter__", caption: "Missing"};
    checks.push(t_true(!cmd("SetGraphHud", #{signals: [a, missing], visible: false}).ok,
        "missing parameters reject at the shared grid owner"));
    let after_rejection = hud_report(1);
    checks.push(t_true(after_rejection.visible && after_rejection.plots == shown.plots,
        "failed selection preserves visibility and every previous plot"));
    checks.push(t_true(!cmd("SetGraphHud", #{signals: [a, b, a, b, a]}).ok,
        "over-budget parameter selection rejects"));
    checks.push(t_true(!cmd("SetGraphHud", #{signals: [a, a]}).ok,
        "duplicate parameters reject"));
    let bad_size = cmd("SetGraphHud", #{width: 0.0, visible: false});
    let after_size = hud_report(1);
    checks.push(t_true(!bad_size.ok && after_size.visible && after_size.width == shown.width,
        "invalid dimensions reject without changing the HUD"));
    checks.push(t_true(cmd("SetGraphHud", #{visible: false}).ok, "hide through shared command"));
    let hidden = hud_report(1);
    checks.push(t_true(!hidden.visible && hidden.plots == shown.plots, "hide retains parameter selection"));
    checks.push(t_true(cmd("SetGraphHud", #{visible: true}).ok, "show retained parameters"));
    let created = cmd("CreateGraphHud", #{});
    checks.push(t_true(created.ok, "create an independent graph HUD"));
    if !created.ok { throw "second HUD could not be created"; }
    let second_hud = created.data.hud;
    checks.push(t_true(cmd("SetGraphHud", #{hud: second_hud, signals: [b], width: 420.0, height: 280.0, x: 840.0, y: 70.0}).ok,
        "configure a second HUD with its own size, position and parameters"));
    let other = hud_report(second_hud);
    checks.push(t_true(hud_report(1).plots == shown.plots && other.plots[0] != shown.plots[1],
        "HUDs retain independent native plot identities"));
    checks.push(t_true(other.signals[0].series_color == hud_report(1).signals[1].series_color,
        "the same parameter keeps its colour across HUDs"));
    checks.push(t_true(cmd("SetGraphHud", #{hud: second_hud, visible: false}).ok &&
        !hud_report(second_hud).visible && hud_report(1).visible, "each HUD has independent visibility"));
    checks.push(t_true(cmd("SetGraphHud", #{hud: second_hud, visible: true}).ok,
        "show the second HUD again"));
    checks.push(t_true(!cmd("SetGraphHud", #{hud: 999999, visible: true}).ok &&
        query("InspectGraphHud", #{}).huds.len() == 2, "unknown HUD identity rejects without creating an instance"));
    checks.push(t_true(!cmd("SetGraphHud", #{hud: second_hud, x: 30.0}).ok,
        "incomplete position rejects without guessing its other coordinate"));
    checks.push(t_true(hud_report(second_hud).width == 420.0 && hud_report(1).width == shown.width,
        "each HUD retains its own dimensions"));
    checks.push(t_true(cmd("SetGraphHud", #{x: 16.0, y: 330.0}).ok, "position the primary HUD independently"));
    let after_history = query("QueryTelemetryHistory", #{key: first.key, limit: 10});
    checks.push(t_true(history.samples == after_history.samples, "presentation preserves measured history"));
    checks.push(t_true(query("ListOpenDocuments", #{}).count == documents, "HUD opens no document"));
    checks.push(t_true(query("InspectWorkbench", #{}).docked_instance_tabs.len() == tabs,
        "HUD opens no dock tab"));
    t_report(checks);
}

fn graph_hud_retired(retired_grid) {
    let huds = query("InspectGraphHud", #{}).huds;
    let old_grid_found = false;
    for grid in query("InspectSignalGrid", #{}).grids {
        if grid.grid.to_string() == retired_grid.to_string() { old_grid_found = true; }
    }
    t_report([t_true(huds.len() == 0, "scene teardown retires all HUD instances"),
              t_true(!old_grid_found, "scene teardown releases HUD-owned grids and plots")]);
}

fn graph_hud_rendered() {
    let huds = query("InspectGraphHud", #{}).huds;
    let checks = [t_true(huds.len() == 2, "both HUD instances are present")];
    for hud in huds {
        checks.push(t_true(hud.visible && hud.rendered, "HUD has a rendered native window"));
        if hud.rect == () || hud.viewport == () {
            checks.push(t_true(false, "native window and viewport have measured bounds"));
        } else {
            let r = hud.rect; let v = hud.viewport;
            checks.push(t_true(r[2] >= 320.0 && r[3] >= 240.0, "rendered graph is readable"));
            checks.push(t_true(r[0] >= v[0] && r[1] >= v[1] &&
                r[0] + r[2] <= v[0] + v[2] && r[1] + r[3] <= v[1] + v[3],
                "native window stays inside the measured viewport"));
        }
    }
    t_report(checks);
}

fn graph_hud_resized(primary_width, primary_height, second_width, second_height) {
    let first = hud_report(1); let second = hud_report(2);
    t_report([t_true(first.width == primary_width && first.height == primary_height,
                "resizing one HUD leaves the other dimensions unchanged"),
              t_true(second.width < second_width - 20.0 && second.height < second_height - 10.0,
                "native corner drag changes the second HUD size"),
              t_true(second.signals.len() == 1 && first.signals.len() == 2,
                "resizing preserves independent parameter selections")]);
}

fn graph_hud_removed_secondary(second_grid) {
    let grid_found = false;
    for grid in query("InspectSignalGrid", #{}).grids {
        if grid.grid.to_string() == second_grid.to_string() { grid_found = true; }
    }
    t_report([t_true(query("InspectGraphHud", #{}).huds.len() == 1, "title-bar close removes one HUD"),
              t_true(!grid_found, "closing a HUD releases its native grid"),
              t_true(hud_report(1).signals.len() == 2, "closing one HUD preserves the other")]);
}
