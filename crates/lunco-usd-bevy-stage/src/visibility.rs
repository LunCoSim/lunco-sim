//! Default-time USD visibility facts needed when a live namespace opinion
//! replaces ordinary visibility on one or more imageable ancestors.
use crate::{UsdRead, UsdReadSource};
use openusd::{sdf::Path, tf::Token, usd::Prim};
use openusd_schemas::geom::{Imageable, ImageableExt, ImageableSchema, Purpose, PurposeVisibility};
use std::collections::HashMap;

fn local_invisible(prim: &Prim) -> openusd::Result<bool> {
    let Some(imageable) = Imageable::from_prim(prim.clone())? else {
        return Ok(false);
    };
    Ok(imageable
        .visibility_attr()
        .get_at::<Token>(None)?
        .as_deref()
        == Some("invisible"))
}

/// Purpose visibility is independent of authored primary pruning only when
/// a live opinion reveals an otherwise invisible branch.
fn purpose_invisible(prim: &Prim, purpose: Purpose) -> openusd::Result<bool> {
    if purpose == Purpose::Default {
        return Ok(false);
    }
    for ancestor in prim.ancestors() {
        let Some(imageable) = Imageable::from_prim(ancestor)? else {
            continue;
        };
        let Some(attr) = imageable.purpose_visibility_attr(purpose)? else {
            continue;
        };
        if attr.has_authored_value()?
            && let Some(value) = attr.get_at::<PurposeVisibility>(None)?
        {
            return Ok(value == PurposeVisibility::Invisible);
        }
    }
    Ok(purpose == Purpose::Guide)
}

/// The worker retains these independent reads only for normally hidden prims.
/// Defer any error in a masked opinion until a live reveal actually needs it.
#[derive(Clone, Debug)]
pub(crate) struct HiddenVisibility {
    ordinary: Result<bool, String>,
    purpose: Result<bool, String>,
}

impl HiddenVisibility {
    pub(crate) fn from_prim(prim: &Prim, purpose: Purpose) -> Self {
        Self {
            ordinary: local_invisible(prim).map_err(|e| e.to_string()),
            purpose: purpose_invisible(prim, purpose).map_err(|e| e.to_string()),
        }
    }
}

impl UsdReadSource<'_> {
    /// Resolve default-time visibility with exact runtime/sampled ordinary
    /// opinions. Never opens a stage or changes the authored namespace.
    pub fn invisible_with_overrides(
        &self,
        path: &Path,
        overrides: &HashMap<String, bool>,
    ) -> Result<bool, String> {
        for ancestor in path.ancestors_below_root() {
            if let Some(visible) = overrides.get(ancestor.as_str()) {
                if !*visible {
                    return Ok(true);
                }
                continue;
            }
            let hidden = match self {
                Self::Prepared(plan) => {
                    let prim = plan
                        .prim(&ancestor)
                        .ok_or_else(|| format!("missing prepared visibility for {ancestor}"))?;
                    prim.hidden_visibility
                        .as_ref()
                        .map_or(Ok(false), |facts| facts.ordinary.clone())?
                }
                Self::Live(view) => {
                    let prim = view
                        .stage()
                        .prim(ancestor.clone())
                        .map_err(|e| e.to_string())?;
                    local_invisible(&prim).map_err(|e| format!("{ancestor}: {e}"))?
                }
            };
            if hidden {
                return Ok(true);
            }
        }
        match self {
            Self::Prepared(plan) => {
                let prim = plan
                    .prim(path)
                    .ok_or_else(|| format!("missing prepared visibility for {path}"))?;
                prim.hidden_visibility
                    .as_ref()
                    .map_or(Ok(false), |facts| facts.purpose.clone())
            }
            Self::Live(view) => {
                let prim = view.stage().prim(path.clone()).map_err(|e| e.to_string())?;
                purpose_invisible(&prim, UsdRead::purpose(self, path)?)
                    .map_err(|e| format!("{path}: {e}"))
            }
        }
    }
}
