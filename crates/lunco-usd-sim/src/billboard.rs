//! USD-authored **billboards** — a screen-facing text label a prim declares for
//! itself, with the content written in the scene rather than compiled in.
//!
//! Before this, the only world-space labels in the tree were the autopilot
//! checkpoint numbers, drawn by a system that knew what a checkpoint was and
//! could label nothing else. A waypoint that wanted to show its own number and
//! coordinates had no way to say so. Now the prim says it:
//!
//! ```usda
//! def Xform "W3" (
//!     prepend references = @lunco://vessels/markers/waypoint.usda@</WaypointMarker>
//! )
//! {
//!     double3 xformOp:translate = (121.0, -1950.9, 87.0)
//!     uniform token[] xformOpOrder = ["xformOp:translate"]
//!     custom bool   lunco:billboard = true
//!     custom string lunco:billboard:text = "{index}\n{lat:.5}, {lon:.5}\n{height:.1} m"
//!     custom float  lunco:billboard:offsetY = 4.0
//!     custom float  lunco:billboard:fadeEnd = 900.0
//! }
//! ```
//!
//! ## The placeholders
//!
//! | token | value |
//! |---|---|
//! | `{name}` | the prim's leaf name (`W3`) |
//! | `{index}` | trailing digits of the leaf name (`W3` → `3`), else the name |
//! | `{label}` | `lunco:label` if authored, else the leaf name |
//! | `{lat}` `{lon}` | geodetic degrees, resolved live through the site anchor |
//! | `{height}` | metres, body datum |
//!
//! Each accepts an optional `:.N` precision for the numeric ones
//! (`{lat:.5}`). An unknown token is left verbatim — a typo shows up as
//! `{latt}` on screen rather than silently rendering an empty label, which is
//! the difference between a bug you see and one you don't.
//!
//! Coordinates are resolved from the entity's **big_space** position, never
//! `Transform.translation`: a grid-direct prim's transform is grid-absolute only
//! while it stays in cell 0, so a moving rover's label would otherwise freeze
//! its coordinates at the cell boundary and quietly under-report by kilometres.

use bevy::prelude::*;

/// A prim that asked to carry a screen-facing text label.
///
/// Data only — the renderer lives with the other viewport overlays, because
/// drawing needs the camera and an egui painter, and this crate must stay
/// usable headless.
#[derive(Component, Clone, Debug)]
pub struct UsdBillboard {
    /// Template with `{...}` placeholders; see the module docs.
    pub template: String,
    /// Metres above the prim origin to float the label.
    pub offset_y: f32,
    /// Distance (m) past which the label is not drawn. Labels are cheap but not
    /// free, and a hundred of them at 5 km is unreadable clutter, not context.
    pub fade_end: f32,
}

impl Default for UsdBillboard {
    fn default() -> Self {
        Self {
            template: "{label}".into(),
            offset_y: 3.0,
            fade_end: 1200.0,
        }
    }
}

/// Values a billboard can interpolate. Assembled per frame by the renderer.
#[derive(Default, Clone, Copy)]
pub struct BillboardFacts<'a> {
    pub name: &'a str,
    pub label: Option<&'a str>,
    /// `None` when the scene is not site-anchored — geo tokens then render as
    /// `—` rather than a fabricated zero.
    pub geo: Option<lunco_celestial::Geodetic>,
}

/// Expand `{...}` placeholders in `template`.
///
/// Unknown tokens survive verbatim so an authoring typo is visible on screen.
pub fn render_billboard(template: &str, facts: &BillboardFacts<'_>) -> String {
    let mut out = String::with_capacity(template.len() + 32);
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        let Some(close) = after.find('}') else {
            // Unterminated brace: emit the remainder as-is.
            out.push_str(&rest[open..]);
            return out;
        };
        let token = &after[..close];
        match expand_token(token, facts) {
            Some(v) => out.push_str(&v),
            None => {
                out.push('{');
                out.push_str(token);
                out.push('}');
            }
        }
        rest = &after[close + 1..];
    }
    out.push_str(rest);
    // `\n` authored in a USD string arrives as a literal backslash-n; treat it
    // as the line break the author plainly meant.
    out.replace("\\n", "\n")
}

/// One placeholder → its text, or `None` if the token is not recognised.
fn expand_token(token: &str, facts: &BillboardFacts<'_>) -> Option<String> {
    let (name, precision) = match token.split_once(":.") {
        Some((n, p)) => (n, p.parse::<usize>().ok()),
        None => (token, None),
    };
    let num = |v: f64, default_prec: usize| format!("{v:.*}", precision.unwrap_or(default_prec));

    match name {
        "name" => Some(facts.name.to_string()),
        "label" => Some(facts.label.unwrap_or(facts.name).to_string()),
        "index" => Some(index_of(facts.name)),
        "lat" => Some(facts.geo.map_or_else(|| "—".into(), |g| num(g.lat_deg, 5))),
        "lon" => Some(facts.geo.map_or_else(|| "—".into(), |g| num(g.lon_deg, 5))),
        "height" => Some(facts.geo.map_or_else(|| "—".into(), |g| num(g.height_m, 1))),
        _ => None,
    }
}

/// Trailing digits of a prim name — `W3` → `3`, `Waypoint_12` → `12`. Falls
/// back to the whole name when it does not end in digits, so `{index}` on a
/// prim called `Base` reads `Base` rather than empty.
fn index_of(name: &str) -> String {
    let digits: String = name
        .chars()
        .rev()
        .take_while(char::is_ascii_digit)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    if digits.is_empty() {
        name.to_string()
    } else {
        digits
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts() -> BillboardFacts<'static> {
        BillboardFacts {
            name: "W3",
            label: None,
            geo: Some(lunco_celestial::Geodetic::new(26.03713, 3.65841, -1950.88)),
        }
    }

    /// The shape the twin's route waypoints actually author: number, then
    /// coordinates, then height.
    #[test]
    fn renders_number_geolocation_and_height() {
        let s = render_billboard("{index}\\n{lat:.4}, {lon:.4}\\n{height:.1} m", &facts());
        assert_eq!(s, "3\n26.0371, 3.6584\n-1950.9 m");
    }

    /// Precision is per-token and optional; the defaults suit lunar sites (5
    /// decimal degrees ≈ 0.3 m, height to 0.1 m).
    #[test]
    fn precision_defaults_and_overrides() {
        assert_eq!(render_billboard("{lat}", &facts()), "26.03713");
        assert_eq!(render_billboard("{lat:.1}", &facts()), "26.0");
    }

    /// An un-anchored scene must not invent coordinates.
    #[test]
    fn missing_site_anchor_renders_a_dash_not_a_zero() {
        let f = BillboardFacts {
            name: "W3",
            label: None,
            geo: None,
        };
        assert_eq!(render_billboard("{lat} {lon} {height}", &f), "— — —");
    }

    /// A typo stays visible instead of vanishing — the whole reason unknown
    /// tokens are not silently dropped.
    #[test]
    fn unknown_token_survives_verbatim() {
        assert_eq!(render_billboard("{latt}", &facts()), "{latt}");
        assert_eq!(render_billboard("a {unclosed", &facts()), "a {unclosed");
    }

    #[test]
    fn label_falls_back_to_the_prim_name() {
        assert_eq!(render_billboard("{label}", &facts()), "W3");
        let named = BillboardFacts {
            label: Some("Point of Interest"),
            ..facts()
        };
        assert_eq!(render_billboard("{label}", &named), "Point of Interest");
    }

    #[test]
    fn index_reads_trailing_digits_or_the_whole_name() {
        assert_eq!(index_of("W3"), "3");
        assert_eq!(index_of("Waypoint_12"), "12");
        assert_eq!(index_of("Base"), "Base");
    }
}
