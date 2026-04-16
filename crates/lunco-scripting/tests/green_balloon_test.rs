use bevy::prelude::*;
use lunco_scripting::{LunCoScriptingPlugin, ScriptRegistry, doc::{ScriptDocument, ScriptedModel, ScriptLanguage}};
use lunco_doc::{DocumentId, DocumentHost};

#[test]
fn test_green_balloon_python_physics() {
    let mut app = App::new();
    // Use regular Time to ensure FixedUpdate runs
    app.add_plugins((MinimalPlugins, LunCoScriptingPlugin));

    // 1. Setup the Green Balloon script
    let doc_id = DocumentId::new(100);
    let source = r#"
g = 9.81
maxVolume = 6.0
dragCoeff = 0.47

height = inputs.get("height", 0.0)
velocity = inputs.get("velocity", 0.0)

temperature = 288.15 - 0.0065 * height
airDensity = (101325.0 / (287.058 * temperature)) * (1.0 - 0.0065 * height / 288.15) ** 5.255
volume = maxVolume * (temperature / 288.15)

buoyancy = airDensity * volume * g
drag = 0.5 * airDensity * dragCoeff * (3.14159 * volume ** (2.0 / 3.0)) * velocity * abs(velocity)

outputs["netForce"] = buoyancy - drag
"#;

    let doc = ScriptDocument {
        id: 100,
        generation: 0,
        language: ScriptLanguage::Python,
        source: source.to_string(),
        inputs: vec!["height".to_string(), "velocity".to_string()],
        outputs: vec!["netForce".to_string()],
    };

    app.world_mut().resource_mut::<ScriptRegistry>().documents.insert(doc_id, DocumentHost::new(doc));

    // 2. Spawn a balloon entity
    let balloon = app.world_mut().spawn(ScriptedModel {
        document_id: Some(100),
        inputs: [("height".to_string(), 1000.0), ("velocity".to_string(), 0.0)].into(),
        ..default()
    }).id();

    // 3. Manually trigger FixedUpdate since MinimalPlugins might not do it automatically in one tick
    app.world_mut().run_schedule(FixedUpdate);

    // 4. Verify results
    let model = app.world().get::<ScriptedModel>(balloon).unwrap();
    let net_force = model.outputs.get("netForce").expect("netForce should be calculated");
    
    println!("Green Balloon Net Force at 1000m: {} N", net_force);
    assert!(*net_force > 0.0, "Balloon should have positive buoyancy at 1000m");
}
