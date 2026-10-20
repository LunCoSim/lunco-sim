// Run with the current runtime_ui_properties.rhai prepended by the API driver.
let lunar=coordinate_properties(#{system:"IAU/WGCCRE",body:301,
    latitude_deg:-84.72672255,longitude_deg:29.14428685,height_m:60.125},[1.0,2.0,3.0]);
let western=coordinate_properties(#{system:"IAU/WGCCRE",body:301,
    latitude_deg:10.0,longitude_deg:-179.5,height_m:-2169.25},[]);
let world=coordinate_properties(#{system:"world"},[1.0,2.0,3.0]);
let unavailable=coordinate_properties((),[1.0e11,2.0,3.0]);
report_verdict([
    t_true(lunar.position=="84.72672° S  ·  29.14429° E","lunar position has angular units and hemispheres"),
    t_true(lunar.height=="60.1 m · mean sphere","height names its datum"),
    t_true(lunar.coordinate_frame=="MOON · IAU/WGCCRE · east positive","frame names the implemented standard"),
    t_true(western.position=="10.0° N  ·  179.5° W","north and west hemispheres"),
    t_true(western.height=="-2169.3 m · mean sphere","negative reference-sphere height is retained"),
    t_true(world.position=="1.0, 2.0, 3.0 m" && world.coordinate_frame=="WORLD · metres","noncelestial sandbox is explicit"),
    t_true(unavailable.position=="COORDINATES UNAVAILABLE" && unavailable.height=="—","invalid site never displays Solar System metres")
],"RUNTIME UI COORDINATES","RUNTIME_UI_COORDINATES");
