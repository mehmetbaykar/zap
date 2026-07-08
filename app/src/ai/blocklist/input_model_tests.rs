//! Unit tests for NLD history-match resolution between shell command history
//! and agent prompt history.
//!
//! These cover [`super::resolve_history_match`]: when both sources match, the
//! later timestamp wins; when only one matches, that source locks the mode.
//! Adapted from upstream warp #12586 for Zap's `InputType`-only autodetection
//! API (no `InputTypeAutoDetectionSource`).

use chrono::{DateTime, Duration, Local};

use super::{resolve_history_match, HistoryMatch, InputType};

/// Returns a timestamp and a strictly-later timestamp, for ordering assertions.
fn earlier_and_later() -> (DateTime<Local>, DateTime<Local>) {
    let earlier = Local::now();
    let later = earlier + Duration::seconds(1);
    (earlier, later)
}

#[test]
fn no_match_from_either_source_is_not_history_match() {
    // Neither command nor prompt history matched: the caller must fall through
    // to the classifier, so we cannot report a history-match decision.
    assert_eq!(
        resolve_history_match(HistoryMatch::NoMatch, HistoryMatch::NoMatch),
        None,
    );
}

#[test]
fn prompt_only_match_locks_to_ai() {
    let (_, prompt_ts) = earlier_and_later();
    assert_eq!(
        resolve_history_match(HistoryMatch::NoMatch, HistoryMatch::MatchedAt(prompt_ts)),
        Some(InputType::AI),
    );
}

#[test]
fn command_only_match_locks_to_shell() {
    let (command_ts, _) = earlier_and_later();
    assert_eq!(
        resolve_history_match(HistoryMatch::MatchedAt(command_ts), HistoryMatch::NoMatch),
        Some(InputType::Shell),
    );
}

#[test]
fn command_only_match_without_timestamp_locks_to_shell() {
    // History-file commands can match without carrying a timestamp.
    assert_eq!(
        resolve_history_match(HistoryMatch::MatchedWithoutTimestamp, HistoryMatch::NoMatch),
        Some(InputType::Shell),
    );
}

#[test]
fn both_match_prompt_newer_locks_to_ai() {
    let (command_ts, prompt_ts) = earlier_and_later();
    assert_eq!(
        resolve_history_match(
            HistoryMatch::MatchedAt(command_ts),
            HistoryMatch::MatchedAt(prompt_ts),
        ),
        Some(InputType::AI),
    );
}

#[test]
fn both_match_command_newer_locks_to_shell() {
    let (prompt_ts, command_ts) = earlier_and_later();
    assert_eq!(
        resolve_history_match(
            HistoryMatch::MatchedAt(command_ts),
            HistoryMatch::MatchedAt(prompt_ts),
        ),
        Some(InputType::Shell),
    );
}

#[test]
fn both_match_equal_timestamps_prefer_shell() {
    // The newer-wins check is strict, so a tie cannot prove the prompt is more
    // recent and we preserve the Shell short-circuit.
    let ts = Local::now();
    assert_eq!(
        resolve_history_match(HistoryMatch::MatchedAt(ts), HistoryMatch::MatchedAt(ts)),
        Some(InputType::Shell),
    );
}

#[test]
fn both_match_command_without_timestamp_locks_to_ai() {
    // A timestamped prompt match beats a command match with no timestamp
    // (e.g. a shell history-file entry): the prompt is the only entry whose
    // recency we can establish, so it is treated as more recent.
    let (_, prompt_ts) = earlier_and_later();
    assert_eq!(
        resolve_history_match(
            HistoryMatch::MatchedWithoutTimestamp,
            HistoryMatch::MatchedAt(prompt_ts),
        ),
        Some(InputType::AI),
    );
}

#[test]
fn both_match_prompt_without_timestamp_locks_to_shell() {
    // Without a prompt timestamp we cannot prove the prompt is newer, so we
    // preserve the Shell short-circuit.
    let (command_ts, _) = earlier_and_later();
    assert_eq!(
        resolve_history_match(
            HistoryMatch::MatchedAt(command_ts),
            HistoryMatch::MatchedWithoutTimestamp,
        ),
        Some(InputType::Shell),
    );
}

#[test]
fn both_match_without_timestamps_prefer_shell() {
    assert_eq!(
        resolve_history_match(
            HistoryMatch::MatchedWithoutTimestamp,
            HistoryMatch::MatchedWithoutTimestamp,
        ),
        Some(InputType::Shell),
    );
}
