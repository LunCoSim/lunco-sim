use serde::Deserialize;

#[derive(Deserialize)]
struct Relaxed {
    components: Vec<lunco_modelica::index::ClassEntry>,
}

fn main() {
    let text = std::fs::read_to_string("/home/rod/Documents/luncosim-workspace/.cache/msl/msl_index.json").unwrap();
    let r = serde_json::from_str::<Relaxed>(&text);
    println!("Parsed Relaxed: {}", r.is_ok());
    if let Err(e) = r {
        println!("Relaxed Error: {:?}", e);
    } else if let Ok(res) = r {
        let c = res.components.iter().find(|c| c.name == "Modelica.Thermal.HeatTransfer.Sources.FixedTemperature").unwrap();
        println!("Icon is some: {}", c.icon.is_some());
        if let Some(i) = &c.icon {
            println!("Graphics len: {}", i.graphics.len());
        }
    }
}
