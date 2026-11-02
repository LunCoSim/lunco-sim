// Generic runtime-UI action policy.
//
// Rust owns only the HUI action transport and dropdown state. Twin/scenario
// policy decides what an action means. Camera actions are shared engine
// capabilities, so they live here instead of inside one route program; other
// actions remain available on the typed runtime.ui.action event bus to the
// authored Twin policies that own them.

fn on_action(ctx) {
    if ctx == () || ctx.action == () || ctx.action == "" {
        return #{ ok: true, handled: false };
    }
    let action = ctx.action;
    let parameters = if ctx.parameters == () { #{} } else { ctx.parameters };
    if action == "request_required" {
        let result = assets::request_required(parameters.scope);
        if result.ok == false { throw result.error; }
        return result;
    }
    if action == "request" {
        return assets::request(parameters.id);
    }
    if action == "process" { return assets::bake(parameters.id); }
    if action == "reload_asset_scene" {return cmd("RestartScene",#{});}
    if action == "rebuild" { return assets::rebuild(parameters.id); }
    if action == "dataset_details" {return cmd("ToggleDatasetDetails",#{id:parameters.id});}
    if action == "open_web_url" { return cmd("OpenWebUrl",#{url:parameters.url}); }
    if action == "cancel" {
        return assets::cancel(parameters.id);
    }
    if action == "dismiss" {
        let scope = if parameters.scope == () { "" } else { parameters.scope };
        return cmd("DismissDatasetProvisioning", #{ scope: scope });
    }
    if handle_camera_ui_action(action) {
        return #{ ok: true, handled: true };
    }
    #{ ok: true, handled: false }
}
