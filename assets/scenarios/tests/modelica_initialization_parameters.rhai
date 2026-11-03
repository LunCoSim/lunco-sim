// @timing presentation
import "lunco://scenarios/tests/modelica_initialization_review" as review;
fn on_start(me, ctx) { this = review::start("parameters", "modelica_initialization_parameters"); }
fn on_tick(me, ctx) { this = review::observe(this); }
