// Run through the production UI host. These operations select historical
// samples only and must leave the physical clock unchanged.
let before=clock_snapshot().sim_tick;
let seed=cmd("SetPlotSignals",#{plot:1,signals:[]});
if !seed.ok {throw "Could not prepare the canonical historical plot";}
let good=cmd("SetPlotScrubTime",#{plot:1,time_s:0.5});
if !good.ok {throw "Existing plot rejected a finite historical cursor";}
let bad=cmd("SetPlotScrubTime",#{plot:999999,time_s:0.5});
if bad.ok || bad.status!="rejected" {throw "Unknown plot must reject its cursor command";}
let clear=cmd("SetPlotScrubTime",#{plot:1});
if !clear.ok {throw "Omitted time must clear the historical cursor";}
if clock_snapshot().sim_tick!=before {throw "Historical cursor advanced the physical clock";}
print("PLOT_SCRUB_TIME_PASS");
