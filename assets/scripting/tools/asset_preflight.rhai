// One-shot preflight protocol. Call advance at a discrete caller-owned boundary;
// scene tests may use on_tick to await their verdict. An error is terminal.

fn create(paths) {
    #{ paths: paths, index: 0, operation_id: (), reports: #{}, error: (), done: false }
}

fn advance(state) {
    if state.done || state.error != () { return state; }
    if state.index >= state.paths.len() { state.done = true; return state; }
    let path = state.paths[state.index];
    let response = if state.operation_id == () {
        query("ValidateAsset", #{path: path})
    } else {
        query("ValidateAsset", #{operation_id: state.operation_id})
    };
    if response == () || response.state == () {
        state.error = path + ": validation did not return an operation envelope";
        return state;
    }
    if response.state == "pending" {
        state.operation_id = response.operation_id;
    } else if response.state == "ready" {
        if response.report == () || response.source_revisions == () {
            state.error = path + ": completed validation omitted its report or read identity";
            return state;
        }
        state.reports[path] = response.report;
        state.operation_id = ();
        state.index += 1;
        state.done = state.index == state.paths.len();
    } else if response.state == "failed" {
        state.error = path + ": " + response.diagnostic;
    } else {
        state.error = path + ": unknown preparation state " + response.state;
    }
    state
}

// A scenario-local sequence of typed query requests. Completed replies remain
// only in the caller's state so assertions can resume after the next boundary.
fn requests() { #{ entries: [], error: () } }

fn read(query_name, params, state) {
    if state.error != () { return #{state: state, ready: false}; }
    let index = -1;
    for i in 0..state.entries.len() {
        let entry = state.entries[i];
        if entry.query_name == query_name && entry.params == params { index = i; break; }
    }
    if index < 0 {
        let response = query(query_name, params);
        if response == () || response.state == () {
            state.error = query_name + ": preparation did not return an operation envelope";
            return #{state: state, ready: false};
        }
        state.entries.push(#{query_name: query_name, params: params,
            operation_id: response.operation_id, response: response});
        index = state.entries.len() - 1;
    }
    let response = state.entries[index].response;
    if response.state == "pending" {
        response = query(query_name, #{operation_id: state.entries[index].operation_id});
        state.entries[index].response = response;
    }
    if response == () || response.state == () {
        state.error = query_name + ": operation was rejected or retired";
    } else if response.state == "ready" {
        if response.report == () || response.source_revisions == () {
            state.error = query_name + ": completed preparation omitted its report or read identity";
        } else {
            return #{state: state, ready: true, report: response.report};
        }
    } else if response.state == "failed" {
        state.error = query_name + ": " + response.diagnostic;
    } else if response.state != "pending" {
        state.error = query_name + ": unknown preparation state " + response.state;
    }
    #{state: state, ready: false}
}
