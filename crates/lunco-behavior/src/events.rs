//! Pure event-matching predicates — the language-agnostic core of `wait_for` /
//! trigger-zone conditions. The host passes the tick's events as `(name, source)`
//! pairs (source = emitter gid, `None` if unknown); no engine or script types
//! leak in, so this is shared by every runtime and trivially testable.

/// True if any event matches `name` and — when `source` is `Some` — was emitted
/// by that source. `wait_for("GO")` passes `source = None`; `wait_for_from`
/// passes the specific emitter gid.
pub fn event_matches(events: &[(&str, Option<u64>)], name: &str, source: Option<u64>) -> bool {
    events
        .iter()
        .any(|(n, s)| *n == name && (source.is_none() || *s == source))
}

/// The zone name of a trigger event, or `None` if it isn't an `enter:`/`exit:`
/// pulse. `zone_of("enter:pad_2") == Some("pad_2")`.
pub fn zone_of(event_name: &str) -> Option<&str> {
    event_name
        .strip_prefix("enter:")
        .or_else(|| event_name.strip_prefix("exit:"))
}

/// True if `event_name` is an ENTER pulse for `zone` (`"enter:<zone>"`).
pub fn entered_zone(event_name: &str, zone: &str) -> bool {
    event_name
        .strip_prefix("enter:")
        .is_some_and(|z| z == zone)
}

/// True if `event_name` is an EXIT pulse for `zone` (`"exit:<zone>"`).
pub fn exited_zone(event_name: &str, zone: &str) -> bool {
    event_name.strip_prefix("exit:").is_some_and(|z| z == zone)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_by_name_any_source() {
        let ev = [("GO", Some(1)), ("STOP", Some(2))];
        assert!(event_matches(&ev, "GO", None));
        assert!(!event_matches(&ev, "WAIT", None));
    }

    #[test]
    fn matches_by_name_and_source() {
        let ev = [("enter:pad", Some(42)), ("enter:pad", Some(7))];
        assert!(event_matches(&ev, "enter:pad", Some(7)));
        assert!(!event_matches(&ev, "enter:pad", Some(99)));
    }

    #[test]
    fn zone_helpers() {
        assert_eq!(zone_of("enter:pad_2"), Some("pad_2"));
        assert_eq!(zone_of("exit:bay"), Some("bay"));
        assert_eq!(zone_of("COLLISION_START"), None);
        assert!(entered_zone("enter:pad_2", "pad_2"));
        assert!(!entered_zone("enter:pad_2", "bay"));
        assert!(exited_zone("exit:bay", "bay"));
        assert!(!exited_zone("enter:bay", "bay"));
    }
}
