fn main() {}
#[test]
fn test_index() {
    let text = std::fs::read_to_string("/home/rod/Documents/luncosim-workspace/.cache/msl/msl_index.json").unwrap();
    let r = serde_json::from_str::<lunco_modelica::index::MslIndex>(&text);
    println!("Parsed MslIndex: {}", r.is_ok());
    if let Err(e) = r {
        println!("Error: {:?}", e);
        // let's try the relaxed
        #[derive(serde::Deserialize)]
        struct Relaxed {
            components: Vec<lunco_modelica::index::ClassEntry>,
        }
        let r2 = serde_json::from_str::<Relaxed>(&text);
        println!("Parsed Relaxed: {}", r2.is_ok());
        if let Err(e2) = r2 {
            println!("Relaxed Error: {:?}", e2);
        } else if let Ok(res) = r2 {
            let c = res.components.iter().find(|c| c.name == "Modelica.Thermal.HeatTransfer.Sources.FixedTemperature").unwrap();
            println!("Icon is some: {}", c.icon.is_some());
        }
    }
}
