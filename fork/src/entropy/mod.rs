//! Entropy & beacon helpers for Fork.
//!
//! See SPEC/entropy.md.
//!
//! Rules:
//! - Raw RF never used directly for keys.
//! - Debias → extract → mix into system pool (additive only).
//! - Beacon digests provide freshness anchoring against public broadcasts (e.g. WWV 10 MHz).

#![allow(dead_code)]

/// Placeholder for a conditioned entropy byte source.
/// Real implementation will call out to rtl_sdr / existing capture tools,
/// apply whitening + hash extraction, and optionally feed the kernel pool.
pub fn conditioned_bytes(_n: usize) -> Vec<u8> {
    // In production: mix hardware noise with OsRng /getrandom.
    // Never sole-source from the radio.
    vec![]
}

/// Placeholder for capturing a short beacon window and returning its digest.
/// Primary target: WWV 10 MHz (or 15 MHz).
pub fn beacon_digest_placeholder() -> String {
    "sha256:beacon-not-yet-captured".to_string()
}
