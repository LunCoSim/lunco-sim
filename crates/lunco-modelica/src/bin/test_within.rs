fn main() {
    let source = "within Modelica; package Thermal end Thermal;";
    let ast = lunco_modelica_ast::parse_to_recovered_ast(source, "test.mo");
    if let Some(within) = ast.within {
        println!("to_string: {:?}", within.to_string());
    } else {
        println!("No within found");
    }
    for (short, _) in ast.classes {
        println!("Class: {}", short);
    }
}
