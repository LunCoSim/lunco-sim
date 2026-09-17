// Application-wide lifecycle policy. A Twin can replace this hook from its
// own policy manifest without changing the Rust runtime.
fn twin_lifecycle(event, ctx) {
    #{
        event: event,
        twin_id: ctx.twin_id,
        root: ctx.root,
        scope: ctx.scope,
        installed: ctx.installed,
        failed: ctx.failed,
    }
}
