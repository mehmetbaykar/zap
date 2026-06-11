//! Token estimation — aligned with opencode `packages/opencode/src/util/token.ts`.
//!
//! ```ts
//! const CHARS_PER_TOKEN = 4
//! export function estimate(input: string) {
//!   return Math.max(0, Math.round((input || "").length / CHARS_PER_TOKEN))
//! }
//! ```
//!
//! Uses `chars().count()` instead of `len()` to avoid UTF-8 multi-byte characters skewing the estimate sky-high.
//! In opencode's JS, `.length` is 1 for characters within the BMP, which matches chars().count() in most cases;
//! for emoji beyond the BMP, JS gives 2 (UTF-16 surrogate pair) while Rust's chars().count() gives 1 —
//! this small discrepancy has no real impact on head/tail splitting.
use super::consts::CHARS_PER_TOKEN;

/// Equivalent to `Math.round(len / 4)`. Returns 0 for an empty string.
pub fn estimate(input: &str) -> usize {
    let n = input.chars().count();
    // Math.round behaves as standard rounding in JS (rather than the round-half-to-even of banker's rounding),
    // so (n + 2) / 4 here is equivalent to round(n / 4) for positive integers.
    (n + CHARS_PER_TOKEN / 2) / CHARS_PER_TOKEN
}

/// Estimate after JSON serialization — aligned with opencode `compaction.ts:241`:
/// `Token.estimate(JSON.stringify(msgs))`
pub fn estimate_json<T: serde::Serialize>(value: &T) -> usize {
    serde_json::to_string(value)
        .map(|s| estimate(&s))
        .unwrap_or(0)
}
