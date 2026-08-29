//! Shared human-readable entity labels.
//!
//! `Name` is the authored/projected hierarchy address and may contain a
//! generated runtime prim name. Presentation surfaces use this resolver so
//! they agree on the semantic label without changing the underlying address.

use bevy::prelude::Name;

use crate::markers::{Callsign, CatalogEntryId};

/// Resolve the one presentation label shared by UI, API, and scripting.
///
/// An authored USD `ui:displayName` (`Callsign`) wins, then a catalog identity,
/// then the leaf of the entity's `Name`. The full `Name` remains available to
/// callers that need the canonical hierarchy address.
pub fn entity_display_name(
    name: Option<&Name>,
    callsign: Option<&Callsign>,
    catalog_id: Option<&CatalogEntryId>,
) -> String {
    callsign
        .map(|value| value.0.trim())
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .or_else(|| {
            catalog_id
                .map(|value| value.0.as_str())
                .filter(|value| !value.trim().is_empty())
                .map(humanize_identifier)
        })
        .or_else(|| {
            name.map(Name::as_str)
                .map(leaf)
                .map(strip_generated_runtime_suffix)
                .filter(|value| !value.trim().is_empty())
                .map(humanize_identifier)
        })
        .unwrap_or_default()
}

/// Convert an authored identifier into a compact human-readable label.
///
/// Separators become spaces, lower-to-upper transitions become word breaks,
/// and the first character of each word is capitalized. Existing acronyms are
/// preserved, so `Wheel_FL` becomes `Wheel FL` and `rocker_bogie` becomes
/// `Rocker Bogie`.
pub fn humanize_identifier(value: &str) -> String {
    let mut label = String::new();
    let mut previous: Option<char> = None;

    for character in value.trim().chars() {
        if !character.is_ascii_alphanumeric() {
            if !label.is_empty() && !label.ends_with(' ') {
                label.push(' ');
            }
            previous = None;
            continue;
        }

        if previous
            .is_some_and(|previous| previous.is_ascii_lowercase() && character.is_ascii_uppercase())
            && !label.ends_with(' ')
        {
            label.push(' ');
        }

        if label.is_empty() || label.ends_with(' ') {
            label.push(character.to_ascii_uppercase());
        } else {
            label.push(character);
        }
        previous = Some(character);
    }

    label.trim().to_string()
}

fn leaf(value: &str) -> &str {
    value.rsplit(['/', '\\']).next().unwrap_or(value)
}

fn strip_generated_runtime_suffix(value: &str) -> &str {
    let Some((prefix, suffix)) = value.rsplit_once('_') else {
        return value;
    };
    if prefix.is_empty() || suffix.len() < 10 || !suffix.bytes().all(|byte| byte.is_ascii_digit()) {
        value
    } else {
        prefix
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authored_display_name_has_priority() {
        let name = Name::new("/Scene/rocker_bogie_123");
        let callsign = Callsign("Survey Rover".into());
        let catalog = CatalogEntryId("rocker_bogie".into());

        assert_eq!(
            entity_display_name(Some(&name), Some(&callsign), Some(&catalog)),
            "Survey Rover"
        );
    }

    #[test]
    fn catalog_identity_hides_generated_runtime_suffix() {
        let name = Name::new("/Scene/rocker_bogie_123");
        let catalog = CatalogEntryId("rocker_bogie".into());

        assert_eq!(
            entity_display_name(Some(&name), None, Some(&catalog)),
            "Rocker Bogie"
        );
    }

    #[test]
    fn name_falls_back_to_a_readable_leaf() {
        let name = Name::new("/Scene/Wheel_FL");

        assert_eq!(entity_display_name(Some(&name), None, None), "Wheel FL");
        assert_eq!(humanize_identifier("solarPanel"), "Solar Panel");
    }

    #[test]
    fn name_fallback_hides_a_generated_runtime_suffix() {
        let name = Name::new("/Scene/rocker_bogie_109565866912737");

        assert_eq!(entity_display_name(Some(&name), None, None), "Rocker Bogie");
    }
}
