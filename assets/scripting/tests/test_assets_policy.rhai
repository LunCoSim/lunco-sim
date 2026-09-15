// TEST-ONLY. The high-level dataset policy is authored in Rhai; this file
// proves its selection and sequencing rules without a network, filesystem, or
// fake Rust registry. The production command/query bridge supplies the same
// map and command result shapes at runtime.

fn query(name, params) {
    #{
        datasets: [
            #{ id: "engine/moon", recommended: true, state: #{ kind: "missing" } },
            #{ id: "engine/earth", recommended: true, state: #{ kind: "installed" } },
        ]
    }
}

fn cmd(name, params) {
    #{ ok: true, command: name, id: params.id }
}

let checks = [];
let listed = list("");
checks.push(t_eq(listed.datasets.len(), 2, "assets.list returns all declared datasets"));
checks.push(t_eq(find("engine/moon").dataset.id, "engine/moon",
    "assets.find returns the requested declaration"));
checks.push(t_true(find("engine/mars").ok == false,
    "assets.find rejects an undeclared dataset"));
checks.push(t_eq(request("engine/earth").status, "already_installed",
    "assets.request does not enqueue an installed dataset"));
checks.push(t_eq(request("engine/moon").command, "RequestDataset",
    "assets.request emits the typed request command"));
checks.push(t_eq(cancel("engine/moon").command, "CancelDataset",
    "assets.cancel emits the typed cancel command"));
checks.push(t_eq(request_recommended("").requested.len(), 1,
    "assets.request_recommended skips installed recommendations"));
t_report(checks);
