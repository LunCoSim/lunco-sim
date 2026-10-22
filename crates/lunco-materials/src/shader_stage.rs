//! Positive stage contracts using the native WESL frontend.
//!
//! A shader asset can be a fragment-only material, a vertex-only companion, or
//! a combined module.  The owner of a stage declares which entry point it needs;
//! it must not reject a valid module because it also contains another stage.
//! The shared WESL parser owns source syntax. Pipeline specialization owns
//! conditional compilation, linking and GPU validation. This render-free contract
//! prevents a stage-less library from being submitted as a material.

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
    pub reason: String,
}

/// Parse authored WESL without a renderer, GPU, or pipeline specialization.
pub fn validate_shader_syntax(source: &str) -> Result<(), String> {
    source
        .parse::<wesl::syntax::TranslationUnit>()
        .map(|_| ())
        .map_err(|error| error.to_string())
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
    use wesl::syntax::{Attribute, GlobalDeclaration, TranslationUnit};

    let module = source
        .parse::<TranslationUnit>()
        .map_err(|error| ShaderStageError {
            stage,
            reason: error.to_string(),
        })?;
    let found = module.global_declarations.iter().any(|declaration| {
        let GlobalDeclaration::Function(function) = &**declaration else {
            return false;
        };
        function.attributes.iter().any(|attribute| {
            matches!(
                (stage, &**attribute),
                (ShaderStage::Fragment, Attribute::Fragment)
                    | (ShaderStage::Vertex, Attribute::Vertex)
            )
        })
    });
    if found {
        Ok(())
    } else {
        Err(ShaderStageError {
            stage,
            reason: "required entry-point function is absent".to_owned(),
        })
    }
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
