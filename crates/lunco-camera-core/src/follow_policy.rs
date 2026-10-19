//! Optional presentation policy over the existing native spring arm.
//! Policy selects a rig; the native solver retains collision and pose ownership.
use crate::FollowAttitude;
pub use lunco_hooks::HookValue as FollowFacts;
use lunco_hooks::{HookValue, RuntimeExecutionContext};

pub const FOLLOW_POLICY_HOOK: &str = "camera.follow";
lunco_hooks::declare_hook! {
    id: FOLLOW_POLICY_HOOK,
    owner: "lunco-camera-core",
    description: "Select vessel-specific follow settings from live rig, subject and obstruction facts; native collision remains authoritative.",
    signature: [ctx: Map],
    output: Map,
    deterministic: false,
    required: false,
    installable: true,
}

/// One presentation-frame rig decision. All distances and angles retain f64
/// until the explicit render rotation boundary in the camera adapter.
#[derive(Clone, Debug)]
pub struct FollowPlan {
    pub distance: f64,
    pub yaw: f64,
    pub pitch: f64,
    pub height: f64,
    pub focus_x: f64,
    pub focus_z: f64,
    pub attitude: FollowAttitude,
    pub track_heading: bool,
    pub rotation_alpha: f64,
    pub position_alpha: f64,
}
impl FollowPlan {
    pub fn facts(&self) -> HookValue {
        HookValue::map([
            ("distance", HookValue::Float(self.distance)),
            ("yaw", HookValue::Float(self.yaw)),
            ("pitch", HookValue::Float(self.pitch)),
            ("height", HookValue::Float(self.height)),
            ("focus_x", HookValue::Float(self.focus_x)),
            ("focus_z", HookValue::Float(self.focus_z)),
            (
                "attitude",
                HookValue::str(match self.attitude {
                    FollowAttitude::Heading => "heading",
                    FollowAttitude::WorldLocked => "world",
                    FollowAttitude::FullAttitude => "body",
                }),
            ),
            ("track_heading", HookValue::Bool(self.track_heading)),
            ("rotation_alpha", HookValue::Float(self.rotation_alpha)),
            ("position_alpha", HookValue::Float(self.position_alpha)),
        ])
    }
    fn apply(&mut self, value: HookValue) -> Result<(), String> {
        let HookValue::Map(fields) = value else {
            return Err("camera.follow must return a map".into());
        };
        for (name, value) in fields {
            match name.as_str() {
                "attitude" => {
                    self.attitude = match value.as_str() {
                        Some("heading") => FollowAttitude::Heading,
                        Some("world") => FollowAttitude::WorldLocked,
                        Some("body") => FollowAttitude::FullAttitude,
                        _ => {
                            return Err(
                                "camera.follow attitude must be heading, world or body".into()
                            );
                        }
                    }
                }
                "track_heading" => {
                    self.track_heading = match value {
                        HookValue::Bool(v) => v,
                        _ => return Err("camera.follow track_heading must be boolean".into()),
                    }
                }
                _ => {
                    let n = value
                        .as_f64()
                        .filter(|v| v.is_finite())
                        .ok_or_else(|| format!("camera.follow {name} must be finite numeric"))?;
                    let slot = match name.as_str() {
                        "distance" if n > 0.0 => &mut self.distance,
                        "yaw" => &mut self.yaw,
                        "pitch" if n.abs() < std::f64::consts::FRAC_PI_2 => &mut self.pitch,
                        "height" => &mut self.height,
                        "focus_x" => &mut self.focus_x,
                        "focus_z" => &mut self.focus_z,
                        "rotation_alpha" if (0.0..=1.0).contains(&n) => &mut self.rotation_alpha,
                        "position_alpha" if (0.0..=1.0).contains(&n) => &mut self.position_alpha,
                        _ => {
                            return Err(format!("camera.follow unsupported field or range: {name}"));
                        }
                    };
                    *slot = n;
                }
            }
        }
        Ok(())
    }
}
/// An absent policy preserves the authored rig. A registered policy's failure
/// is an error; the adapter holds its pose and publishes a diagnostic.
pub fn resolve_follow_plan(
    facts: HookValue,
    mut plan: FollowPlan,
    context: RuntimeExecutionContext,
) -> Result<FollowPlan, String> {
    if let Some(result) = lunco_hooks::invoke_with_context(FOLLOW_POLICY_HOOK, &[facts], context) {
        plan.apply(result.map_err(|e| format!("camera.follow failed: {e}"))?)?;
    }
    Ok(plan)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn plan() -> FollowPlan {
        FollowPlan {
            distance: 10.0,
            yaw: 0.0,
            pitch: -0.2,
            height: 1.0,
            focus_x: 0.0,
            focus_z: 0.0,
            attitude: FollowAttitude::Heading,
            track_heading: true,
            rotation_alpha: 0.2,
            position_alpha: 0.2,
        }
    }
    #[test]
    fn validates_partial_decisions_and_custom_easing() {
        let mut p = plan();
        p.apply(HookValue::map([
            ("attitude", HookValue::str("world")),
            ("focus_z", HookValue::Float(2.0)),
            ("rotation_alpha", HookValue::Float(0.5)),
        ]))
        .unwrap();
        assert_eq!(p.attitude, FollowAttitude::WorldLocked);
        assert_eq!(p.focus_z, 2.0);
        assert_eq!(p.rotation_alpha, 0.5);
    }
    #[test]
    fn invalid_decisions_are_errors() {
        for value in [
            HookValue::str("bad"),
            HookValue::map([("distance", HookValue::Float(-1.0))]),
            HookValue::map([("yaw", HookValue::Float(f64::NAN))]),
            HookValue::map([("position_alpha", HookValue::Float(1.1))]),
            HookValue::map([("track_heading", HookValue::Int(1))]),
            HookValue::map([("unconsumed", HookValue::Float(1.0))]),
        ] {
            assert!(plan().apply(value).is_err());
        }
    }
}
