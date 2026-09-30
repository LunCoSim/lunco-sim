// Production browser checks. The API runner supplies a measured baseline and
// explicit expectations after channel admission, metadata edits, or selection.
// Only the runner performs the commands; verdicts stay in authored Rhai.

fn catalog_count(value) { parse_int(value.to_string()) }

let current = query("InspectTelemetryCatalog", #{signal: CATALOG_SIGNAL});
let missing = query("InspectTelemetryCatalog", #{signal: CATALOG_MISSING_SIGNAL});
let rejected = query("InspectTelemetryCatalog", #{signal: 42});
let fails = [];
fails.push(t_eq(catalog_count(current.initial_scans), CATALOG_BASELINE.initial_scans, "no full catalog rescan"));
fails.push(t_eq(catalog_count(current.prepared_channels), CATALOG_BASELINE.prepared_channels + CATALOG_EXPECT_PREPARED, "only changed descriptors prepared"));
fails.push(t_eq(catalog_count(current.indexed_channels), CATALOG_BASELINE.indexed_channels + CATALOG_EXPECT_ADDED, "channel index cardinality"));
fails.push(t_eq(catalog_count(current.pending_channels), 0, "descriptor queue settled"));
fails.push(t_true(!current.worker_active, "worker settled"));
fails.push(t_true(!current.preparing, "existing tree remains available"));
fails.push(t_true(catalog_count(current.last_batch_channels) <= 64, "bounded worker batch"));
fails.push(t_eq(missing.descriptors.len(), 0, "missing channel is not fabricated"));
fails.push(t_true(rejected.ok == false, "invalid signal parameter is rejected"));
fails.push(t_true(rejected.error.contains("InspectTelemetryCatalog: `signal` must be a string"), "rejection identifies the owning query"));
let continuously_available = true;
for preparing in CATALOG_PREPARING_OBSERVATIONS {
    if preparing { continuously_available = false; }
}
fails.push(t_true(continuously_available, "tree stays available during observed updates"));
if CATALOG_SIGNAL != "" {
    fails.push(t_eq(current.descriptors.len(), 1, "probe descriptor has one explicit owner"));
    if current.descriptors.len() == 1 {
        fails.push(t_eq(current.descriptors[0].unit, CATALOG_EXPECT_UNIT, "latest descriptor metadata"));
    }
}
if CATALOG_CHECK_HISTORY {
    let samples_present = !CATALOG_HISTORY_BEFORE.is_empty() && !CATALOG_HISTORY_AFTER.is_empty();
    fails.push(t_true(samples_present, "live history contains samples"));
    if samples_present {
        fails.push(t_true(CATALOG_HISTORY_AFTER[-1] > CATALOG_HISTORY_BEFORE[-1], "live history continues advancing"));
    }
}
t_report(fails);
