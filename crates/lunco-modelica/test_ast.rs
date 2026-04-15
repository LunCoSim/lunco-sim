use rumoca_phase_parse::parse_to_ast;
use rumoca_session::parsing::ast::{ClassDef, Element};
fn main() {
    let source = "model Resistor extends OnePort; end Resistor;";
    let ast = parse_to_ast(source, "model.mo").unwrap();
    println!("{:#?}", ast);
}
