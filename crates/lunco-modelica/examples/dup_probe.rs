use rumoca_phase_parse::{parse_to_ast, parse_to_syntax};

fn main() {
    let src = r#"model Untitled1

    Modelica.Blocks.Noise.BandLimitedWhiteNoise NewBandLimitedWhiteNoise
        annotation(Placement(transformation(extent={{602.391968,-295.217072},{622.391968,-275.217072}})));
    Modelica.Blocks.Sources.IntegerStep NewIntegerStep
        annotation(Placement(transformation(extent={{1000.103516,-431.484467},{1020.103516,-411.484467}})));

    Modelica.Blocks.Sources.IntegerStep NewIntegerStep
        annotation(Placement(transformation(extent={{1000.103516,-431.484467},{1020.103516,-411.484467}})));

equation
    connect(NewBandLimitedWhiteNoise.y, NewIntegerStep.y);
end Untitled1;
"#;
    println!("=== parse_to_ast (strict) ===");
    match parse_to_ast(src, "test.mo") {
        Ok(ast) => {
            println!("OK. Classes: {:?}", ast.classes.keys().collect::<Vec<_>>());
            if let Some(c) = ast.classes.get("Untitled1") {
                println!("  Untitled1 components ({}): {:?}", c.components.len(), c.components.keys().collect::<Vec<_>>());
            }
        }
        Err(e) => println!("ERR: {}", e),
    }
    println!();
    println!("=== parse_to_syntax (lenient) ===");
    let syntax = parse_to_syntax(src, "test.mo");
    let ast = syntax.best_effort();
    println!("Classes: {:?}", ast.classes.keys().collect::<Vec<_>>());
    if let Some(c) = ast.classes.get("Untitled1") {
        println!("  Untitled1 components ({}): {:?}", c.components.len(), c.components.keys().collect::<Vec<_>>());
    }
    println!("Parse errors: {}", syntax.parse_errors().len());
    println!();
    println!("=== regex scan ===");
    let re = regex::Regex::new(
        r"(?m)^\s*(?:(?:flow|stream|input|output|parameter|constant|discrete|inner|outer|replaceable|final)\s+)*((?:[A-Za-z_]\w*\.)*[A-Za-z_]\w*)\s+([A-Za-z_]\w*)\b"
    ).unwrap();
    const KEYWORDS: &[&str] = &[
        "model", "block", "connector", "package", "function", "record", "class", "type",
        "extends", "import", "equation", "algorithm", "initial", "protected", "public",
        "annotation", "connect", "if", "for", "when", "end", "within", "and", "or", "not",
        "true", "false", "else", "elseif", "elsewhen", "while", "loop", "break", "return",
        "then", "external", "encapsulated", "partial", "expandable", "operator", "pure",
        "impure", "redeclare",
    ];
    for cap in re.captures_iter(src) {
        let ty = &cap[1];
        let inst = &cap[2];
        let first = ty.split('.').next().unwrap_or(ty);
        let filtered = KEYWORDS.contains(&first);
        println!("  type='{}' inst='{}' filtered={}", ty, inst, filtered);
    }
}
