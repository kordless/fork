# Entropy & Freshness Anchoring

## Purpose

Fork needs two things from entropy:

1. **Key material** for signing snapshot batches (vendor-independent provenance).
2. **Freshness** — proof that a signed batch is no older than a particular moment in real time.

The second is the more important and the less commonly solved.

## Source

A $30 RTL-SDR (or any modest SDR) tuned to a quiet HF band or to a known time/standard station supplies atmospheric + thermal noise at high sample rates. This is the random.org / rtl-entropy lineage.

**Iron rules**

- Raw RF is biased, correlated, and adversarially injectable. It must never be used as a key directly.
- Pipeline: capture → debias / whitening → cryptographic extractor (hash) → mix into the system entropy pool *alongside* `/dev/urandom`. Mixed entropy can only improve the pool; sole-sourced sky noise is strictly worse than boring kernel entropy.
- Honest claim: provenance and independence from any single vendor or OS image. Not “stronger crypto.”

## Freshness via Beacon Digest

Signatures prove who. Per-URL hash chains prove order. They do not prove *when*.

A compromised signer can backdate snapshots. To close this:

- Periodically (or per batch) capture a short window of a widely-receivable broadcast (WWV, CHU, other stable shortwave time stations, or any signal with many independent listeners).
- Compute a digest of that capture (`beacon_digest`).
- Include `beacon_digest` in the batch Merkle root (or as a committed field in the manifest).

Anyone who recorded the same frequency at roughly the same time can recompute and corroborate. The batch is then bounded in time by physics and by the existence of independent spectrum logs. This also bootstraps a radio-archive side channel: corroboration requires other people keeping the air.

## Snapshot / Bundle Fields

Optional but recommended for signed batches:

```json
{
  "beacon": {
    "digest": "sha256:...",
    "freq_hz": 10000000,
    "station": "WWV",
    "captured_at": "2026-08-19T08:00:00Z",
    "duration_ms": 2000,
    "receiver": "rtl-sdr"
  }
}
```

The `beacon.digest` is what gets folded into the Merkle root.

## Implementation Notes (v0)

- `fork/src/entropy/` contains the reference conditioner and beacon helper.
- Hardware is optional. The CLI must still function with pure `/dev/urandom` + system time; the SDR path is an enhancement for high-assurance or air-gapped signing.
- Prefer existing tools where possible (`rtl_sdr`, `rtl_entropy`, `sox`, etc.) and keep the Rust surface thin.
