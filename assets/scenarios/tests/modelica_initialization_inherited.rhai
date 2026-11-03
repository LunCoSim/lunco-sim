// @timing presentation
import "lunco://scenarios/tests/modelica_initialization_review" as review;
fn on_start(me, ctx) { this = review::start("inherited", "modelica_initialization_inherited"); }
fn on_tick(me, ctx) { this = review::observe(this); }
