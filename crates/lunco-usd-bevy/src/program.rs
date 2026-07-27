//! The USD side of a **Modelica program facet** — one reader, shared by every
//! consumer of that authoring contract.
//!
//! Three places used to answer "is this prim a Modelica component, and what
//! class does it instantiate?" independently: the runtime network projector
//! (`lunco-usd-sim`), the per-prim program binder (same crate), and the lint
//! fact producer (`lunco-usd-avian`). They disagreed — the lint accepted any
//! `.mo` asset while the projector additionally needed a `models/` root to
//! invent the class name — so an asset could lint clean and be rejected at
//! load with a different message. The contract lives here now, and the callers
//! read it.
//!
//! Modelica lexical rules (identifiers, keywords, the mangling used to spell a
//! USD path as an identifier) live here for the same reason: the authoring
//! check and the code emitter must use ONE definition of "valid member name",
//! and this is the crate both sides already depend on.

use std::collections::HashSet;

// `StageView` rather than `impl UsdRead`: the composed reads this needs
// (`value_str`, `collection_members`) are the view's own, and every caller —
// the runtime projector, the per-prim binder, the lint facts — already holds one.
use crate::read::UsdRead;
use crate::view::StageView;
use openusd::sdf::Path as SdfPath;

/// Why a prim that claims to be a Modelica program facet cannot be used as one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProgramSourceIssue {
    /// The property carrying the unusable opinion (`<prim>.info:sourceAsset`).
    pub property: String,
    /// Actionable explanation, suitable for a console line or a lint message.
    pub message: String,
}

/// The Modelica class a program facet instantiates, or why it has none.
///
/// `info:sourceAsset:subIdentifier` wins when authored — that is USD's own way
/// of naming one entity inside a source file, and it is the only correct answer
/// for a `package.mo`. Otherwise the class is derived from the asset path below
/// its `models/` root, which is the layout the shipped library uses (`within
/// LunCo.Electrical;` in `models/LunCo/Electrical/Battery.mo`).
///
/// The `.mo` itself is NOT parsed here: this runs inside stage reads on the web
/// too, where the file is an unfetched HTTP resource. A path whose package root
/// cannot be established is therefore a hard, named error asking for the
/// `subIdentifier` rather than a guess — a guessed class surfaces much later as
/// an unattributable "class not found" from the compiler.
pub fn modelica_member_class(
    view: &StageView<'_>,
    prim: &SdfPath,
) -> Result<String, ProgramSourceIssue> {
    let implementation = view
        .value_str(prim, "info:implementationSource")
        .unwrap_or_default();
    if implementation != "sourceAsset" {
        return Err(ProgramSourceIssue {
            property: format!("{prim}.info:implementationSource"),
            message: "a Modelica program facet must use info:implementationSource = sourceAsset"
                .into(),
        });
    }
    let Some(source) = view.asset(prim, "info:sourceAsset") else {
        return Err(ProgramSourceIssue {
            property: format!("{prim}.info:sourceAsset"),
            message: "a Modelica program facet must author a .mo info:sourceAsset".into(),
        });
    };
    let sub_identifier = view
        .value_str(prim, "info:sourceAsset:subIdentifier")
        .filter(|value| !value.is_empty());
    model_class_from_asset(&source, sub_identifier.as_deref()).ok_or(ProgramSourceIssue {
        property: format!("{prim}.info:sourceAsset"),
        message: format!(
            "`{source}` does not name a Modelica class: point it at a `.mo` under a `models/` \
             root, or author info:sourceAsset:subIdentifier with the fully-qualified class name"
        ),
    })
}

/// The class named by an asset path (+ optional `subIdentifier`), or `None`.
///
/// `rsplit_once` deliberately: the package root is the LAST `models/` segment on
/// the path, so a twin cached under `…/models/cache/models/LunCo/…` still
/// resolves to `LunCo.…` instead of `cache.models.LunCo.…`.
pub fn model_class_from_asset(asset: &str, sub_identifier: Option<&str>) -> Option<String> {
    if let Some(class) = sub_identifier {
        return is_modelica_class_name(class).then(|| class.to_string());
    }
    let path = asset
        .strip_prefix("lunco://")
        .or_else(|| asset.strip_prefix("twin://"))
        .unwrap_or(asset);
    let model_path = match path.rsplit_once("models/") {
        Some((_, tail)) => tail,
        None => return None,
    };
    let class = model_path.strip_suffix(".mo")?;
    let class = class.replace('/', ".");
    is_modelica_class_name(&class).then_some(class)
}

pub fn is_modelica_class_name(class: &str) -> bool {
    !class.is_empty() && class.split('.').all(is_modelica_identifier)
}

/// Is `prim` the root of a projected domain network — i.e. does it carry the
/// component collection the runtime compiles into one generated model?
///
/// Codeless multiple-apply schemas are not consistently surfaced by every
/// OpenUSD binding through `HasAPI`; their standard authored properties are
/// authoritative and round-trip in all runtimes.
pub fn is_domain_network_root(view: &StageView<'_>, prim: &SdfPath) -> bool {
    view.any_attr_with_prefix(prim, "collection:components:")
}

/// Every prim on the stage that belongs to SOME component collection.
///
/// A member is compiled as part of its network's generated model, so no other
/// pass may give it a solver of its own. Membership — not "does it declare an
/// acausal connector" — is what makes that true: a causal-only member (a
/// controller, a PDU) is just as much part of the generated DAE, and gating on
/// connectors handed it a second, independent solver whose outputs then fed the
/// wire fabric.
pub fn network_member_paths(view: &StageView<'_>) -> HashSet<String> {
    let mut members = HashSet::new();
    for prim in view.prim_paths() {
        if !is_domain_network_root(view, &prim) {
            continue;
        }
        let Ok(paths) = view.collection_members(&prim, "components") else {
            continue;
        };
        members.extend(paths.into_iter().map(|path| path.to_string()));
    }
    members
}

/// The Modelica keywords a generated or authored member name may not be.
const KEYWORDS: &[&str] = &[
    "algorithm",
    "and",
    "annotation",
    "block",
    "break",
    "class",
    "connect",
    "connector",
    "constant",
    "constrainedby",
    "der",
    "discrete",
    "each",
    "else",
    "elseif",
    "elsewhen",
    "encapsulated",
    "end",
    "enumeration",
    "equation",
    "expandable",
    "extends",
    "external",
    "false",
    "final",
    "flow",
    "for",
    "function",
    "if",
    "import",
    "impure",
    "in",
    "initial",
    "inner",
    "input",
    "loop",
    "model",
    "not",
    "operator",
    "or",
    "outer",
    "output",
    "package",
    "parameter",
    "partial",
    "protected",
    "public",
    "pure",
    "record",
    "redeclare",
    "replaceable",
    "return",
    "stream",
    "then",
    "true",
    "type",
    "when",
    "while",
    "within",
];

pub fn is_modelica_identifier(raw: &str) -> bool {
    let mut chars = raw.chars();
    chars
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
        && chars.all(|character| character.is_ascii_alphanumeric() || character == '_')
        && !KEYWORDS.contains(&raw)
}

/// Injective ASCII spelling for arbitrary USD path/name text.
///
/// `_` is escaped too, so punctuation replacement cannot collapse `Motor-A`
/// and `Motor_A` onto one Modelica instance.
pub fn modelica_identifier(raw: &str) -> String {
    if is_modelica_identifier(raw) {
        return raw.to_string();
    }
    let mut result = modelica_path_identifier(raw);
    if !result.starts_with("usd_") {
        result.insert_str(0, "usd_");
    }
    result
}

pub fn modelica_path_identifier(raw: &str) -> String {
    let mut result = String::with_capacity(raw.len() + 1);
    for character in raw.chars() {
        if character.is_ascii_alphanumeric() {
            result.push(character);
        } else if character == '_' {
            result.push_str("__");
        } else {
            result.push_str(&format!("_x{:x}_", character as u32));
        }
    }
    if result.is_empty() {
        result.push_str("ModelicaNetwork");
    }
    if result.as_bytes()[0].is_ascii_digit() || !is_modelica_identifier(&result) {
        result.insert_str(0, "usd_");
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derives_qualified_class_from_model_asset_path() {
        assert_eq!(
            model_class_from_asset("lunco://models/LunCo/Electrical/Battery.mo", None),
            Some("LunCo.Electrical.Battery".into())
        );
        assert_eq!(
            model_class_from_asset(
                "lunco://models/vendor/package.mo",
                Some("Vendor.Power.CustomBattery")
            ),
            Some("Vendor.Power.CustomBattery".into())
        );
        assert_eq!(
            model_class_from_asset("lunco://models/vendor/package.mo", Some("bad-class")),
            None
        );
    }

    #[test]
    fn package_root_is_the_last_models_segment() {
        assert_eq!(
            model_class_from_asset("twin://cache/models/twin/models/Site/Rover.mo", None),
            Some("Site.Rover".into())
        );
    }

    #[test]
    fn a_source_outside_a_models_root_needs_a_sub_identifier() {
        assert_eq!(model_class_from_asset("twin://parts/Battery.mo", None), None);
        assert_eq!(
            model_class_from_asset("twin://parts/Battery.mo", Some("Twin.Parts.Battery")),
            Some("Twin.Parts.Battery".into())
        );
    }

    #[test]
    fn generated_identifiers_are_injective_and_avoid_keywords() {
        assert_ne!(
            modelica_path_identifier("Motor-A"),
            modelica_path_identifier("Motor_A")
        );
        assert_eq!(modelica_identifier("model"), "usd_model");
        assert_eq!(modelica_identifier("3phase"), "usd_3phase");
        assert!(is_modelica_identifier(&modelica_identifier("left/right")));
    }
}
