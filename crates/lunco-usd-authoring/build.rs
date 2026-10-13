use std::{env, fs, path::PathBuf};

fn main() {
    openusd_build::configure()
        .search_path("schema/bases")
        .schema_data("schema/schema.usda")
        .plug_info("schema/plugInfo.json")
        .generate()
        .expect("the project schema declarations must generate");

    // USD excludes generator customData from resolved definitions. Inspector
    // hints are application facts, decoded once through the shared native reader.
    let mut hints = Vec::new();
    for path in ["schema/schema.usda", "schema/core/physxSchema.usda"] {
        println!("cargo:rerun-if-changed={path}");
        let data = openusd::usda::read_file(path).expect("valid registered schema source");
        for (path, spec) in data.iter() {
            let Some((prim, name)) = path.split_property() else {
                continue;
            };
            let Some(openusd::sdf::Value::Dictionary(dict)) = spec.get("customData") else {
                continue;
            };
            let Some(hint) = lunco_usd_data::metadata::AttrUiHint::from_dict(dict) else {
                continue;
            };
            assert!(
                hint.min.is_none_or(f64::is_finite) && hint.max.is_none_or(f64::is_finite),
                "schema inspector bounds must be finite at {path}"
            );
            hints.push((
                prim.name().expect("schema class name").to_owned(),
                name.to_owned(),
                hint,
            ));
        }
    }
    hints.sort_by(|a, b| (&a.0, &a.1).cmp(&(&b.0, &b.1)));
    let mut source = String::from(
        "pub(super) const DECLARED_HINTS: &[(&str, &str, Option<f64>, Option<f64>, Option<&str>, Option<&str>)] = &[\n",
    );
    for (schema, name, hint) in hints {
        source.push_str(&format!(
            "({schema:?}, {name:?}, {:?}, {:?}, {:?}, {:?}),\n",
            hint.min,
            hint.max,
            hint.unit.as_deref(),
            hint.type_name.as_deref()
        ));
    }
    source.push_str("];\n");
    fs::write(
        PathBuf::from(env::var_os("OUT_DIR").expect("Cargo output directory"))
            .join("schema_hints.rs"),
        source,
    )
    .expect("write compiled inspector hints");
}
