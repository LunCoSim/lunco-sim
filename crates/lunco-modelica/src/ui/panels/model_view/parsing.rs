//! Text and annotation extraction helpers for Modelica documentation.

pub fn extract_documentation(
    annotations: &[rumoca_compile::parsing::ast::Expression],
) -> (Option<String>, Option<String>) {
    use rumoca_compile::parsing::ast::Expression;
    let call = annotations.iter().find(|e| match e {
        Expression::FunctionCall { comp, .. } | Expression::ClassModification { target: comp, .. } => {
            comp.parts
                .first()
                .map(|p| p.ident.text.as_ref() == "Documentation")
                .unwrap_or(false)
        }
        _ => false,
    });
    let Some(call) = call else { return (None, None) };
    let args: &[Expression] = match call {
        Expression::FunctionCall { args, .. } => args.as_slice(),
        Expression::ClassModification { modifications, .. } => modifications.as_slice(),
        _ => return (None, None),
    };
    let str_arg = |name: &str| -> Option<String> {
        for a in args {
            let (arg_name, value) = match a {
                Expression::NamedArgument { name, value, .. } => {
                    (name.text.as_ref(), value.as_ref())
                }
                Expression::Modification { target, value, .. } => (
                    target.parts.first().map(|p| p.ident.text.as_ref()).unwrap_or(""),
                    value.as_ref(),
                ),
                _ => continue,
            };
            if arg_name != name {
                continue;
            }
            if let Some(s) = crate::ast_extract::string_literal_value(value) {
                return Some(s);
            }
        }
        None
    };
    (str_arg("info"), str_arg("revisions"))
}
