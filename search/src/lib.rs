//! Fork search — local index over verified snapshots.
//!
//! v0 is intentionally thin: the CLI already does keyword search by walking
//! snapshot JSON. This crate is the future home of a Tantivy (or similar)
//! full-text + eventual hybrid index so search stays fast as the corpus grows.

pub fn placeholder() {
    // Real index comes next: snapshot JSON → Tantivy documents keyed by body_digest.
}
