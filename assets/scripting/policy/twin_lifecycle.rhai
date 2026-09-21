// Application-wide lifecycle policy. A Twin can replace this hook from its
// own policy manifest without changing the Rust runtime.
fn twin_path(path) {
    path.replace("\\", "/")
}

fn twin_under_any_root(path, roots) {
    path = twin_path(path);
    for root in roots {
        let normalized_root = twin_path(root);
        if normalized_root == "." || path == normalized_root || path.starts_with(normalized_root + "/") { return true; }
    }
    false
}

fn twin_parent_path(path) {
    path = twin_path(path);
    let parts = path.split("/");
    if parts.len() < 2 { return "."; }
    let parent = "";
    for index in 0..(parts.len() - 1) {
        if index > 0 { parent += "/"; }
        parent += parts[index];
    }
    parent
}

fn twin_lifecycle(event, ctx) {
    let actions = [];
    let info = "";

    if event == "assets_mounted" && ctx.active {
        let manifest = ctx.manifest;
        let usd = if manifest != () && manifest.contains("usd") { manifest["usd"] } else { () };
        let scene = if usd != () && usd.contains("default_scene") { twin_path(usd["default_scene"]) } else { () };

        if scene != () {
            actions.push(#{
                command: "OpenTwinScene",
                params: #{ twin_id: ctx.twin_id, name: ctx.name, relative_path: scene },
            });
        } else {
            let reason = if manifest == () {
                "`" + ctx.root + "` has no twin.toml, so there is no scene to load. You may have opened the wrong folder; open the Twin root containing twin.toml."
            } else {
                "`" + ctx.root + "` has a twin.toml but declares no default scene — nothing to load."
            };
            actions.push(#{ command: "SetEmptyViewportReason", params: #{ reason: reason } });
            actions.push(#{ command: "ClearScene", params: #{} });
        }

        actions.push(#{
            command: "ActivateTwinToolScope",
            params: #{ twin_id: ctx.twin_id },
        });
        for path in ctx.files {
            if twin_parent_path(path) == "tools" && path.ends_with(".rhai") {
                let parts = path.split("/");
                let filename = parts[parts.len() - 1];
                let names = filename.split(".rhai");
                let library_name = names[0];
                actions.push(#{
                    command: "LoadTwinToolLibrary",
                    params: #{
                        twin_id: ctx.twin_id,
                        name: ctx.name,
                        library_name: library_name,
                        relative_path: path,
                    },
                });
            }
        }

        actions.push(#{
            command: "ActivateTwinTimelineScope",
            params: #{ twin_id: ctx.twin_id },
        });
        for path in ctx.files {
            if twin_parent_path(path) == "timelines" && path.ends_with(".json") {
                let parts = path.split("/");
                let filename = parts[parts.len() - 1];
                let names = filename.split(".json");
                let timeline_name = names[0];
                actions.push(#{
                    command: "LoadTwinTimelineFile",
                    params: #{
                        twin_id: ctx.twin_id,
                        name: ctx.name,
                        timeline_name: timeline_name,
                        relative_path: path,
                    },
                });
            }
        }

        let sysml = if manifest != () && manifest.contains("sysml") { manifest["sysml"] } else { () };
        let roots = if sysml != () && sysml.contains("paths") { sysml["paths"] } else { ["."] };
        if roots.len() == 0 { roots = ["."]; }
        let root = if sysml != () && sysml.contains("root") { twin_path(sysml["root"]) } else { () };
        let sources = [];
        for path in ctx.files {
            if (path.ends_with(".sysml") || path.ends_with(".kerml"))
                && twin_under_any_root(path, roots) {
                sources.push(path);
            }
        }

        let root_in_scope = root == () || twin_under_any_root(root, roots);
        let source_count = sources.len();
        if root != () && root_in_scope && sources.contains(root) {
            actions.push(#{
                command: "LoadTwinSysmlSource",
                params: #{ twin_id: ctx.twin_id, name: ctx.name, relative_path: root },
            });
        }
        for path in sources {
            if path != root {
                actions.push(#{
                    command: "LoadTwinSysmlSource",
                    params: #{ twin_id: ctx.twin_id, name: ctx.name, relative_path: path },
                });
            }
        }
        if source_count == 0 {
            info = "Twin `" + ctx.name + "` declares no indexed .sysml or .kerml sources";
        }

        let modelica = if manifest != () && manifest.contains("modelica") { manifest["modelica"] } else { () };
        let local_roots = [];
        if modelica != () && modelica.contains("paths") {
            for path in modelica["paths"] { local_roots.push(twin_path(path)); }
        } else {
            for path in ctx.files {
                if path.ends_with(".mo") {
                    let candidate = twin_parent_path(path);
                    if !local_roots.contains(candidate) { local_roots.push(candidate); }
                }
            }
            let minimized_roots = [];
            for candidate in local_roots {
                let nested = false;
                for root in minimized_roots {
                    if root == "." || candidate.starts_with(root + "/") { nested = true; break; }
                }
                if !nested {
                    if candidate == "." {
                        minimized_roots = ["."];
                    } else {
                        minimized_roots = minimized_roots.filter(|root| !root.starts_with(candidate + "/"));
                        minimized_roots.push(candidate);
                    }
                }
            }
            local_roots = minimized_roots;
        }
        let source_index = 0;
        for path in local_roots {
            actions.push(#{
                command: "LoadTwinModelicaSourceRoot",
                params: #{
                    twin_id: ctx.twin_id,
                    name: ctx.name,
                    id: "twin:" + ctx.name + ":modelica:" + source_index.to_string(),
                    path: path,
                },
            });
            source_index += 1;
        }
        if modelica != () && modelica.contains("externals") {
            for external in modelica["externals"] {
                let path = twin_path(external.path);
                if !path.starts_with("@bundled:") {
                    actions.push(#{
                        command: "LoadTwinModelicaSourceRoot",
                        params: #{
                            twin_id: ctx.twin_id,
                            name: ctx.name,
                            id: "twin:" + ctx.name + ":modelica:" + source_index.to_string(),
                            path: path,
                        },
                    });
                    source_index += 1;
                }
            }
        }
    }

    #{
        event: event,
        twin_id: ctx.twin_id,
        root: ctx.root,
        scope: ctx.scope,
        installed: ctx.installed,
        failed: ctx.failed,
        actions: actions,
        info: info,
    }
}
