//! Positive stage contracts for authored WGSL programs.
//!
//! A shader asset can be a fragment-only material, a vertex-only companion, or
//! a combined module.  The owner of a stage declares which entry point it needs;
//! it must not reject a valid module because it also contains another stage.
//! Full WGSL compilation remains the renderer/asset-loader responsibility. This
//! small render-free contract prevents a stage-less library from being submitted
//! as a material and is shared by USD projection and the render binder.

use std::fmt;

/// The stage an authored shader asset is being used for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ShaderStage {
    Fragment,
    Vertex,
}

impl ShaderStage {
    const fn attribute(self) -> &'static str {
        match self {
            Self::Fragment => "@fragment",
            Self::Vertex => "@vertex",
        }
    }

    const fn label(self) -> &'static str {
        match self {
            Self::Fragment => "fragment",
            Self::Vertex => "vertex",
        }
    }
}

/// A source-level stage contract failure.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShaderStageError {
    pub stage: ShaderStage,
    pub reason: &'static str,
}

impl fmt::Display for ShaderStageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} stage does not declare a callable {} entry point ({})",
            self.stage.label(),
            self.stage.attribute(),
            self.reason,
        )
    }
}

/// Validate the positive entry-point contract for one authored stage.
///
/// This deliberately checks only the stage being requested. A combined module
/// is valid for either use, and a module may contain helper text mentioning an
/// entry-point attribute in comments without satisfying the contract.
pub fn validate_shader_stage(source: &str, stage: ShaderStage) -> Result<(), ShaderStageError> {
    let code = strip_wgsl_comments(source);
    let attribute = stage.attribute();
    let Some(attribute_start) = code.match_indices(attribute).find_map(|(start, _)| {
        let after = &code[start + attribute.len()..];
        after
            .as_bytes()
            .first()
            .is_none_or(|byte| !byte.is_ascii_alphanumeric() && *byte != b'_')
            .then_some(start)
    }) else {
        return Err(ShaderStageError {
            stage,
            reason: "required entry-point attribute is absent",
        });
    };

    // WGSL attributes apply to the declaration immediately following them.
    // Requiring a function token avoids accepting an orphan annotation while
    // leaving the actual signature/type checking to the renderer's WGSL loader.
    let after_attribute = &code[attribute_start + attribute.len()..];
    if !function_follows_attributes(after_attribute) {
        return Err(ShaderStageError {
            stage,
            reason: "entry-point attribute is not followed by a function",
        });
    }

    Ok(())
}

fn function_follows_attributes(mut source: &str) -> bool {
    loop {
        source = source.trim_start();
        if let Some(after_fn) = source.strip_prefix("fn") {
            if after_fn
                .chars()
                .next()
                .is_none_or(|character| !(character.is_ascii_alphanumeric() || character == '_'))
            {
                return true;
            }
        }

        let Some(after_at) = source.strip_prefix('@') else {
            return false;
        };
        let name_len = after_at
            .find(|character: char| !(character.is_ascii_alphanumeric() || character == '_'))
            .unwrap_or(after_at.len());
        if name_len == 0 {
            return false;
        }
        source = &after_at[name_len..];
        source = source.trim_start();
        if !source.starts_with('(') {
            continue;
        }

        let mut depth = 0usize;
        let mut end = None;
        for (index, character) in source.char_indices() {
            match character {
                '(' => depth += 1,
                ')' => {
                    if depth == 0 {
                        return false;
                    }
                    depth -= 1;
                    if depth == 0 {
                        end = Some(index + character.len_utf8());
                        break;
                    }
                }
                _ => {}
            }
        }
        let Some(end) = end else {
            return false;
        };
        source = &source[end..];
    }
}

/// Remove line and block comments before looking for stage attributes.
///
/// WGSL has no string literals that can contain source comments in a valid
/// shader, so a small lexical pass is sufficient and keeps this crate free of
/// a renderer/compiler dependency.
fn strip_wgsl_comments(source: &str) -> String {
    let mut out = String::with_capacity(source.len());
    let bytes = source.as_bytes();
    let mut i = 0;
    let mut block = false;
    while i < bytes.len() {
        if block {
            if bytes[i..].starts_with(b"*/") {
                block = false;
                out.push(' ');
                i += 2;
            } else {
                if bytes[i] == b'\n' {
                    out.push('\n');
                }
                i += 1;
            }
        } else if bytes[i..].starts_with(b"//") {
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
        } else if bytes[i..].starts_with(b"/*") {
            block = true;
            i += 2;
        } else {
            out.push(bytes[i] as char);
            i += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_the_requested_stage_in_a_combined_module() {
        let source = "@vertex fn vertex() {}\n@fragment fn fragment() {}";
        assert!(validate_shader_stage(source, ShaderStage::Vertex).is_ok());
        assert!(validate_shader_stage(source, ShaderStage::Fragment).is_ok());
    }

    #[test]
    fn ignores_stage_text_in_comments() {
        let source = "// @fragment fn example() {}\nfn helper() {}";
        assert!(validate_shader_stage(source, ShaderStage::Fragment).is_err());
    }

    #[test]
    fn rejects_an_attribute_without_a_function() {
        assert!(
            validate_shader_stage("@fragment struct Material {}", ShaderStage::Fragment).is_err()
        );
    }

    #[test]
    fn requires_a_complete_attribute_token() {
        assert!(
            validate_shader_stage("@fragmented fn fragment() {}", ShaderStage::Fragment).is_err()
        );
    }
}
