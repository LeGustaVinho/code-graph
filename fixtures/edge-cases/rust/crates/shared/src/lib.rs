//! Helpers shared by every crate of the workspace.

/// Rounds a price to cents.
pub fn round_cents(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}
