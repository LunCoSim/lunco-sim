// Run through the host Rhai test runner with a uniquely projected PROBE path.
let failures = [];
let id = find_path(PROBE);
let shifted = PROBE + "_ProjectedPathProbe";
let document = query("QueryUsdPrim", #{path: PROBE, attrs: []});
if id < 0 || document == () || find_path(shifted) != -1 {
    failures.push("expected a unique mounted probe and an unused destination");
} else {
    let doc = document.doc_id;
    failures.push(t_true(set(id, "UsdPrimPath.path", shifted), "replace projected identity"));
    failures.push(t_eq(find_path(shifted), id, "global lookup follows replacement"));
    failures.push(t_eq(find_path_in_stage(id, shifted), id, "mount lookup follows replacement"));
    failures.push(t_eq(find_path_in_document(doc, shifted), id, "document lookup follows replacement"));
    failures.push(t_eq(find_path(PROBE), -1, "old identity released"));
    failures.push(t_true(!set(id, "UsdPrimPath.absentField", shifted), "invalid edit rejected"));
    failures.push(t_eq(find_path(shifted), id, "rejected edit preserves identity"));
    failures.push(t_true(set(id, "UsdPrimPath.path", PROBE), "restore projected identity"));
    failures.push(t_eq(find_path_in_document(doc, PROBE), id, "restored identity resolves"));
    failures.push(t_eq(find_path(shifted), -1, "temporary identity released"));
}
t_report(failures);
