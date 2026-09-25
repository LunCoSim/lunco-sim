//! Rust-backed string operations shared by workspace Rhai engines.

use rhai::{Engine, ImmutableString};

/// Register Unicode-aware Rust string operations missing from Rhai's default
/// string package. Case conversion returns a new string; Rhai's ordinary `==`
/// keeps its exact, case-sensitive semantics for names and identity keys.
pub fn register_string_functions(engine: &mut Engine) {
    engine
        .register_fn(
            "to_lowercase",
            |value: ImmutableString| -> ImmutableString { value.to_lowercase().into() },
        )
        .register_fn(
            "to_uppercase",
            |value: ImmutableString| -> ImmutableString { value.to_uppercase().into() },
        )
        .register_fn("trimmed", |value: ImmutableString| -> ImmutableString {
            value.trim().into()
        })
        .register_fn("contains_alphabetic", |value: ImmutableString| {
            value.chars().any(char::is_alphabetic)
        });
}

#[cfg(test)]
mod tests {
    use super::register_string_functions;
    use rhai::{Engine, ImmutableString};

    #[test]
    fn string_helpers_are_unicode_aware_and_do_not_change_identity_equality() {
        let mut engine = Engine::new();
        register_string_functions(&mut engine);

        let lower = engine
            .eval::<ImmutableString>(r#""ÄBC Port".to_lowercase()"#)
            .expect("to_lowercase is registered");
        assert_eq!(lower.as_str(), "äbc port");

        let upper = engine
            .eval::<ImmutableString>(r#""straße".to_uppercase()"#)
            .expect("to_uppercase is registered");
        assert_eq!(upper.as_str(), "STRASSE");

        let trimmed = engine
            .eval::<ImmutableString>(r#""  Port α  ".trimmed()"#)
            .expect("trimmed returns a string value");
        assert_eq!(trimmed.as_str(), "Port α");

        assert!(
            engine
                .eval::<bool>(r#""123α".contains_alphabetic()"#)
                .expect("contains_alphabetic is registered")
        );
        assert!(
            !engine
                .eval::<bool>(r#""Port" == "port""#)
                .expect("ordinary equality remains available")
        );
    }
}
