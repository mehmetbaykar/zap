use std::time::Duration;

use super::{
    CLIENT_WATCHDOG_SAFETY_MARGIN, DEFAULT_ORCHESTRATED_IDLE_TIMEOUT_SECONDS, HARD_FLOOR,
    watchdog_timeout_for_stamped_seconds,
};

#[test]
fn watchdog_timeout_subtracts_the_safety_margin() {
    assert_eq!(
        watchdog_timeout_for_stamped_seconds(60),
        Duration::from_secs(30)
    );
}

#[test]
fn watchdog_timeout_clamps_short_values_to_the_hard_floor() {
    assert_eq!(watchdog_timeout_for_stamped_seconds(10), HARD_FLOOR);
}

#[test]
fn watchdog_timeout_uses_the_default_when_unset_or_negative() {
    let expected = Duration::from_secs(DEFAULT_ORCHESTRATED_IDLE_TIMEOUT_SECONDS as u64)
        - CLIENT_WATCHDOG_SAFETY_MARGIN;
    assert_eq!(watchdog_timeout_for_stamped_seconds(0), expected);
    assert_eq!(watchdog_timeout_for_stamped_seconds(-42), expected);
}

#[test]
fn watchdog_timeout_preserves_large_values_after_the_margin() {
    assert_eq!(
        watchdog_timeout_for_stamped_seconds(900),
        Duration::from_secs(870)
    );
}
