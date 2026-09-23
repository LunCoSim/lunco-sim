// The application startup policy installs this function once. The USD scene
// lifecycle invokes it once after the composed stage and scene projections
// settle, then releases time-dependent consumers with the selected epoch.
fn select_scene_time(facts) {
    if facts.epoch_api == true && facts.epoch_status == "valid" {
        return #{
            source: "authored",
            epoch_jd: facts.authored_epoch_jd,
            warning: (),
        };
    }

    let warning = ();
    if facts.epoch_api == true {
        warning = "Scene root LunCoEpochAPI has a " + facts.epoch_status
            + " lunco:time:epochJd; using current computer UTC converted to TDB."
            + " Author a non-zero epoch for repeatable celestial time.";
    } else if facts.has_celestial_source == true {
        warning = "Scene has celestial sources but no root LunCoEpochAPI; using"
            + " current computer UTC converted to TDB. Author an epoch for"
            + " repeatable celestial time.";
    }

    #{
        source: "computer_time",
        epoch_jd: facts.computer_time_tdb_jd,
        warning: warning,
    }
}
