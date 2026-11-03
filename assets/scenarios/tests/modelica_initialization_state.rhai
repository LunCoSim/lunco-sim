// @timing presentation
import "lunco://scenarios/tests/modelica_initialization_review" as review;
fn on_start(me, ctx) { this = review::start("state", "modelica_initialization_state"); }
fn on_tick(me, ctx) { this = review::observe(this); }
