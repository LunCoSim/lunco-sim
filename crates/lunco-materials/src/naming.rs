//! Parameter-name normalisation — **render-free**.
//!
//! USD authors material params in camelCase by convention (`colorA`, `baseColor`,
//! `morphStart`); WESL `struct Material` fields are snake_case (`color_a`,
//! `base_color`, `morph_start`). Both authoring paths (USD → `lunco-usd-sim-shader`, the
//! live `SetObjectProperty` command → `lunco-scene-authoring`) bridge the two through
//! here, so an authored name actually resolves to a schema field instead of
//! silently packing to nothing.

use std::borrow::Cow;

/// Converts a camelCase / PascalCase identifier to snake_case. Canonical names
/// borrow their input; only a changed spelling allocates owned storage.
///
/// An underscore is inserted before an uppercase letter that either follows a
/// lowercase letter/digit (`baseColor` → `base_color`) or begins a new word at
/// the end of an acronym (`AOStrength` → `ao_strength`).
pub fn to_snake_case(s: &str) -> Cow<'_, str> {
    if !s.bytes().any(|byte| byte.is_ascii_uppercase()) {
        return Cow::Borrowed(s);
    }
    let mut chars = s.chars().peekable();
    let mut previous: Option<char> = None;
    let mut out = String::with_capacity(s.len() + 4);
    while let Some(c) = chars.next() {
        if c.is_ascii_uppercase() {
            let prev_lower_or_digit = previous
                .is_some_and(|previous| previous.is_ascii_lowercase() || previous.is_ascii_digit());
            let acronym_boundary = previous.is_some_and(|previous| previous.is_ascii_uppercase())
                && chars.peek().is_some_and(|next| next.is_ascii_lowercase());
            if prev_lower_or_digit || acronym_boundary {
                out.push('_');
            }
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
        previous = Some(c);
    }
    Cow::Owned(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_parameter_names_keep_their_borrowed_storage() {
        for name in ["", "roughness", "color_a", "uv_scale2", "m²", "éclair"] {
            assert!(
                matches!(to_snake_case(name), std::borrow::Cow::Borrowed(value) if value == name)
            );
        }
        assert!(
            matches!(to_snake_case("AOStrength"), std::borrow::Cow::Owned(value) if value == "ao_strength")
        );
    }

    #[test]
    fn snake_case_conversion() {
        assert_eq!(to_snake_case("colorA"), "color_a");
        assert_eq!(to_snake_case("baseColor"), "base_color");
        assert_eq!(to_snake_case("morphStart"), "morph_start");
        // already snake_case → unchanged (idempotent)
        assert_eq!(to_snake_case("color_a"), "color_a");
        assert_eq!(to_snake_case("roughness"), "roughness");
        // acronym boundary
        assert_eq!(to_snake_case("AOStrength"), "ao_strength");
        // digits
        assert_eq!(to_snake_case("uvScale2"), "uv_scale2");
    }
}
