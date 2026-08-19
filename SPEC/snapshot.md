# Fork Snapshot Specification v0.1

**Status:** Working draft — the piece that makes this a fork, not a mirror.

## Goal

A content-addressed, versioned, verifiable record of a web resource at a point in time, designed so that:

- Quiet rewriting becomes a detectable hash mismatch.
- Deletion is answered by redistribution of the signed bundle.
- Synthetic flooding is resisted by prioritising human / at-risk sources via tailscore.

Plumbing is commodity: WARC for the raw fetch, SHA-256 for content addressing, Merkle trees for batches, detached signatures for authenticity. We invent as little as possible.

## Snapshot Object

A single snapshot is a JSON document (or CBOR later) with the following required fields:

```json
{
  "spec": "fork-snapshot/0.1",
  "url": "https://example.com/page",
  "fetched_at": "2026-08-19T08:00:00Z",
  "warc_digest": "sha256:...",          // digest of the raw WARC record
  "body_digest": "sha256:...",          // digest of the primary response body (bytes)
  "extraction_digest": "sha256:...",    // digest of the normalized text/extraction
  "content_type": "text/html; charset=utf-8",
  "status": 200,
  "prior": null | "sha256:...",         // body_digest of the previous snapshot of this URL, if known
  "tailscore": 0.0-1.0,                 // optional at-risk / humanness score
  "meta": { ... }                       // free-form (title, language, links, etc.)
}
```

### Digests

- All digests are `sha256:` followed by lowercase hex.
- `body_digest` is over the exact response body bytes as received (after any content-encoding is decoded).
- `extraction_digest` is over a deterministic normalized extraction (whitespace collapsed, scripts/styles stripped, etc.). The exact normalization rules are versioned and must be recorded.
- `warc_digest` covers the full WARC record that contains the request/response pair.

### Prior pointer

Forms a per-URL hash chain. Given two snapshots of the same URL, `diff` is simply the comparison of their digests and the chain linkage. A broken chain or mismatched prior is evidence of a gap or rewrite.

## Bundle

A bundle is a signed collection of snapshots plus optional WARC payloads:

```
bundle/
  manifest.json          # list of snapshot digests + Merkle root
  snapshots/             # individual snapshot JSON files, named by body_digest
  warc/                  # optional: the raw WARC records
  root.sig               # detached signature over the Merkle root
```

The Merkle root is computed over the sorted list of `body_digest`s (or full snapshot digests). The signature is over that root (and a timestamp / key id).

## CLI Surface (reference)

```
fork snap <url> [--out dir] [--prior digest]
    Fetch, write WARC + snapshot, print digests.

fork verify <bundle>
    Check all digests, chain integrity, and signature.

fork diff <url>@<t1> <url>@<t2>
    Show what changed between two snapshots (hash-level first, then optional text diff).

fork tailscore <url-or-file>
    Compute or display the at-risk / humanness heuristic.
```

## Design Rules

1. **Pluggable Protocol Surface.** Use WARC, content hashes, existing signature schemes, IPFS/torrents for redistribution. Protocol handlers (`http`, `dns`, `finger`, `gopher`, `gemini`, `ftp`, `news`) are self-forged by Sigil agents as unhandled schemes are encountered.
2. **Selection before volume.** Tailscore decides priority. The goal is not “everything” but “the parts that would otherwise disappear and that carry human signal.”
3. **Verifiability is the differentiator.** If a later copy cannot prove it matches an earlier signed root, it is not the same history.
4. **Redistribution is the durability strategy.** Many independent seeds beat one perfect archive.

## Self-Forging Agent Merging (Sigil Integration)

Fork protocol adapters follow Sigil's self-forging agent workflow:
- Each node agent detects unhandled URL schemes (e.g., `finger://`, `dns://`, `gopher://`).
- The Sigil engine writes, tests, and verifies the corresponding `ProtocolHandler` implementation locally.
- The agent opens a GitHub Pull Request against `DeepBlueDynamics/fork`.
- Automated CI evaluates build, lint, and security constraints before auto-merging into `main`.

## Relation to DMDS / Entropy Preservation

Post-2022 the open web is increasingly model-generated. The pre-flood human corpus is non-renewable. This snapshot format is a seed bank: cultural memory and uncontaminated training distribution. DMDS tries to keep entropy inside the model; Fork keeps it at the source.

## Open Questions for v0.1

- Exact normalization function for `extraction_digest` (must be pure and versioned).
- Preferred signature scheme (minisign / age / SSH / raw ed25519).
- Minimal viable tailscore features (domain age, inbound link rarity, presence of personal markers, absence of common generator fingerprints, etc.).
- How to store and query the per-URL prior chain at scale.
