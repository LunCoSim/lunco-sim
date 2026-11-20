// @timing presentation
const COMET = "/World/Comet";

fn simulation_dependencies(me, ctx) {
    #{modelica_entities: [], entity_reads: [], entity_writes: [],
      query_reads: ["GetReadiness"], required_inputs: []}
}

fn set_drive(comet, value) {
    let result = drive(comet, value, 0.0);
    if !result.ok { throw result.to_string(); }
}

fn speed(comet) { -get(comet, "velocity_z") }

fn reverse(me) {
    this.forward = speed(this.comet);
    set_drive(this.comet, -1.0);
}

fn forward_again(me) {
    this.reverse = speed(this.comet);
    set_drive(this.comet, 1.0);
}

fn finish(me) {
    let last_speed = speed(this.comet);
    set_drive(this.comet, 0.0);
    report_verdict([
        t_true(this.forward > 2.0, "Comet reaches forward speed before reverse is commanded"),
        t_true(this.reverse < -1.0, "Full reverse brings Comet through zero speed without a gearbox fault"),
        t_true(last_speed > 1.0, "Comet accelerates forward again after full reverse"),
        t_true(query("GetReadiness").ready && clock_snapshot().clock_contract_ok,
            "The shared simulation stays live through both direction changes")
    ], "CRATER CUP COMET REVERSE", "CRATER_CUP_COMET_REVERSE");
}

fn task(me, ctx) {
    seq([wait_until(|m| query("GetReadiness").ready),
        once(|m| { this.comet = find_path(COMET); set_drive(this.comet, 1.0); }),
        wait_until(|m| speed(this.comet) > 2.1), once(Fn("reverse")),
        wait_until(|m| speed(this.comet) < -1.1), once(Fn("forward_again")),
        wait_until(|m| speed(this.comet) > 1.1), once(Fn("finish"))])
}
