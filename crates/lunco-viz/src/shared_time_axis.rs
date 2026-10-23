//! View-only aggregation of native history bounds. Samples keep their own
//! timestamps and values; a grid shares only the horizontal viewport.

/// Union of displayed time intervals, including late-starting and idle signals.
pub(crate) fn shared_time_span(spans: impl IntoIterator<Item = [f64; 2]>) -> Option<[f64; 2]> {
    spans.into_iter().fold(None, |combined, [first, latest]| {
        Some(combined.map_or([first, latest], |[low, high]: [f64; 2]| {
            [low.min(first), high.max(latest)]
        }))
    })
}

#[cfg(test)]
mod tests {
    use super::shared_time_span;

    #[test]
    fn late_starting_sensor_and_idle_channel_keep_one_recording_interval() {
        assert_eq!(
            shared_time_span([[2.016_666_7, 6.1], [0.0, 3.0]]),
            Some([0.0, 6.1])
        );
        assert_eq!(
            shared_time_span([[0.0, 3.0], [2.016_666_7, 6.1]]),
            Some([0.0, 6.1])
        );
    }

    #[test]
    fn empty_and_negative_time_domains_do_not_invent_a_zero_origin() {
        assert_eq!(shared_time_span([]), None);
        assert_eq!(
            shared_time_span([[-3.0, -1.0], [-2.0, -0.5]]),
            Some([-3.0, -0.5])
        );
    }
}
