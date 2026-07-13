//! Parameter-name normalisation — **render-free**.
//!
//! USD authors material params in camelCase by convention (`colorA`, `baseColor`,
//! `morphStart`); WGSL `struct Material` fields are snake_case (`color_a`,
//! `base_color`, `morph_start`). Both authoring paths (USD → `lunco-usd-sim`, the
//! live `SetObjectProperty` command → `lunco-sandbox-edit`) bridge the two through
//! here, so an authored name actually resolves to a schema field instead of
//! silently packing to nothing.

/// Converts a camelCase / PascalCase identifier to snake_case. Idempotent for
/// names that are already snake_case (no uppercase → returned unchanged), so it
/// is safe to apply on every authored param.
///
/// An underscore is inserted before an uppercase letter that either follows a
/// lowercase letter/digit (`baseColor` → `base_color`) or begins a new word at
/// the end of an acronym (`AOStrength` → `ao_strength`).
pub fn to_snake_case(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len() + 4);
    for (i, &c) in chars.iter().enumerate() {
        if c.is_ascii_uppercase() {
            let prev_lower_or_digit =
                i > 0 && (chars[i - 1].is_ascii_lowercase() || chars[i - 1].is_ascii_digit());
            let acronym_boundary = i > 0
                && chars[i - 1].is_ascii_uppercase()
                && i + 1 < chars.len()
                && chars[i + 1].is_ascii_lowercase();
            if prev_lower_or_digit || acronym_boundary {
                out.push('_');
            }
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

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
