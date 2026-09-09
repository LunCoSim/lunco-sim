// Graphics acceptance for the maintained non-flat DEM and full-footprint rocks
// fixture. USD owns the terrain recipe; this observer checks the composed
// authored contract while the production render gate checks real pixels and the
// runtime log checks that the scatter owner instantiated the population.
const TEST_KIND = "graphics";
const ROOT = "/TerrainRocksTest/Terrain";
const WAIT_S = 5.0;

fn query_ok(record) {
    record != () && record.ok != false && record.attrs != ()
}

fn terrain_ready(record) {
    query_ok(record)
        && record.children != ()
        && record.children.contains(ROOT + "/Ground")
        && record.children.contains(ROOT + "/Rocks")
}

fn on_start(me) {
    this.t = 0.0;
    this.done = false;
}

fn on_tick(me) {
    if this.done { return; }
    this.t += dt();
    if this.t < 0.5 { return; }

    // The scene script can start before the composed projection has published
    // its child paths. Retry the public query until the authored contract is
    // observable, then fail visibly if it never becomes available.
    let terrain = query("QueryUsdPrim", #{
        path: ROOT,
        attrs: ["lunco:assetMode"],
        children: true,
    });
    let ground = query("QueryUsdPrim", #{
        path: ROOT + "/Ground",
        attrs: [
            "lunco:layer",
            "lunco:layer:demSource",
            "lunco:layer:windowM",
            "lunco:layer:targetRes",
            "lunco:layer:lodViz",
            "lunco:layer:colliderRing",
        ],
    });
    let rocks = query("QueryUsdPrim", #{
        path: ROOT + "/Rocks",
        attrs: [
            "lunco:layer",
            "lunco:layer:density",
            "lunco:layer:regionM",
            "lunco:layer:seed",
        ],
    });
    if this.t < WAIT_S && (!terrain_ready(terrain) || !query_ok(ground) || !query_ok(rocks)) {
        return;
    }
    this.done = true;

    let failures = [];
    failures.push(t_true(query_ok(terrain)
        && terrain.attrs["lunco:assetMode"] == "layered"
        && terrain.children.contains(ROOT + "/Ground")
        && terrain.children.contains(ROOT + "/Rocks"),
        "composed terrain does not expose the DEM and rocks layers"));
    failures.push(t_true(query_ok(ground)
        && ground.attrs["lunco:layer"] == "dem"
        && ground.attrs["lunco:layer:demSource"] == "terrain/rocks_fixture"
        && ground.attrs["lunco:layer:windowM"] == 1000.0
        && ground.attrs["lunco:layer:targetRes"] == 257
        && ground.attrs["lunco:layer:lodViz"] == true
        && ground.attrs["lunco:layer:colliderRing"] == true,
        "DEM layer does not carry the maintained fixture and explicit streaming contract"));
    failures.push(t_true(query_ok(rocks)
        && rocks.attrs["lunco:layer"] == "rocks"
        && rocks.attrs["lunco:layer:density"] == 12.0
        && rocks.attrs["lunco:layer:regionM"] == 0.0
        && rocks.attrs["lunco:layer:seed"] == 27092026,
        "rocks layer does not carry the deterministic full-footprint recipe"));
    report_verdict(failures, "TERRAIN ROCKS", "TERRAIN_ROCKS");
}
