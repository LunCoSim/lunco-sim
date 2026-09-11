use lunco_modelica_ast::parse_to_ast;
use rumoca_compile::parsing::ast::{ClassDef, Element};
fn main() {
    let source = "model Resistor extends OnePort; end Resistor;";
    let ast = parse_to_ast(source, "model.mo").unwrap();
    println!("{:#?}", ast);
}
