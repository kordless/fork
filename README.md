# Fork

**Fork preserves the web's human entropy — versioned, hash-anchored, and copied into enough hands that no one can quietly rewrite it, delete it, or drown it in synthetic sameness.**

All in Rust.

## What this is

An *active* archive of the human web. Not a mausoleum. You can go back and use what was preserved.

- **Selection first** (tailscore) — the tails and non-optimized corners that popularity-weighted crawlers miss.
- **Verifiability** — content-addressed snapshots, per-URL hash chains, digests you can check.
- **Redistribution** — many copies, no single institutional choke point.
- **Active retrieval** — `fork get` reconstitutes what was snapped so the past is usable again.

The same entropy thesis as the DMDS/AETHER paper, applied at the source: keep the high-entropy human corpus before it is drowned in synthetic sameness.

## Quick start

```bash
# Build (needs recent Rust)
cargo build -p fork --release

# Snap a page
cargo run -p fork -- snap https://example.com

# Retrieve it later
cargo run -p fork -- get <body_digest or short id>

# Verify local collection
cargo run -p fork -- verify ./snapshots

# Keyword search across snapshots
cargo run -p fork -- search "some terms"

# Rough prioritization heuristic
cargo run -p fork -- tailscore https://some.personal.site
```

## Commands

| Command | Purpose |
|---------|---------|
| `fork snap <url>` | Fetch + write snapshot (body + JSON with digests, extraction, links, prior pointer) |
| `fork get <target>` | Reconstitute / display a snapshot (active use) |
| `fork verify [dir]` | Check body digests against recorded hashes |
| `fork diff a b` | Compare two snapshots |
| `fork search <query>` | Keyword search over local snapshots |
| `fork tailscore <url>` | Placeholder at-risk / humanness score |

## Layout

```
SPEC/snapshot.md     # the format that makes this a fork
SPEC/entropy.md      # sky entropy + beacon freshness anchoring
fork/                # the CLI (snap / get / verify / diff / search / tailscore / protocols)
grubcrawler/         # concurrent crawler gateway (feeds the same snapshot shape later)
search/              # future Tantivy index
community/           # intentionally thin surface
```

## Self-Forging Multi-Protocol Architecture (Sigil Integration)

Fork integrates with **[Sigil](https://github.com/DeepBlueDynamics/sigil)** — DeepBlue Dynamics' self-forging Rust agent framework.

Each Fork instance runs a local model agent. When a node encounters an unhandled protocol URL scheme (`dns://`, `finger://`, `gopher://`, `gemini://`, `ftp://`, `news://`), Sigil orchestrates the workflow:
1. **Self-Forging Protocol Extension**: The agent autonomously scaffolds, verifies, and tests a new Rust `ProtocolHandler` implementation matching the Fork specification.
2. **Automated CI & Merge**: The agent generates tests, runs verification, and opens a GitHub Pull Request to merge the new protocol component back into `kordless/fork` without requiring manual developer coding.
3. **Pluggable Protocol Trait**: All network extensions implement `ProtocolHandler` under `fork::protocol`, keeping snapshot hashing, WARC digests, and `tailscore` evaluation consistent across all transport layers.

## Scope

- Content + selection + verifiability = spine
- Protocol work = plumbing only (WARC, hashes, existing distribution)
- Platform / social rebuild = out of scope

## Milestone direction

v0.1 target remains: thousands of at-risk pages, snapshotted, digest-verified, and searchable so the collection is already active.

The history is the only branch you can actually check out — and then run.

---
Started 2026-08-19 · DeepBlue Dynamics
