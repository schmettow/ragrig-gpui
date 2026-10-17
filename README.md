# ragrig-gpui

Reusable [GPUI](https://gpui.rs) components for Ragrig: RAG chat, a streaming
transcript, a composer, reference ingestion, library management, and settings.
The crate has no editor; Sciscraper and other hosts compose these pieces into
their own layout.

Status: **M0 foundation spike complete**. The public API is still provisional
until M1. See [`PLAN.md`](PLAN.md) for the milestones, decisions, and measured
spike results.

## Quick start

Requirements:

- Rust 1.88 or newer. The M0 spike was tested with Rust 1.99.
- On Linux, the GPUI backend links `libxkbcommon` and `libxkbcommon-x11`.
  The runtime libraries are not sufficient:

```sh
sudo apt install libxkbcommon-dev libxkbcommon-x11-dev
```

Run the spike:

```sh
cargo run --example spike --no-default-features
```

`--no-default-features` skips the Ragrig dependency graph; the spike uses a
fake streaming worker. The default build (`--features ragrig`) is the one M2
will use.

### What to try

- Type a question and press **Ctrl+Enter** (Linux/Windows) or **⌘+Enter**
  (macOS). **Enter** inserts a newline.
- Watch the answer stream from a dedicated Tokio thread and the transcript
  follow the tail.
- Scroll up while streaming: following pauses until you return to the end.
- Press **Stop** during an answer: the partial answer stays, marked `stopped`.
- Open the **Fake RAG** popover in the agent bar.
- Use the **Library** and **Settings** dock panels. The Settings fields are
  live but not yet wired to the mock agent.

The spike can self-drive for screenshots and measurement:

```sh
RAGRIG_SPIKE_AUTORUN="Explain retrieval-augmented generation" \
  cargo run --example spike --no-default-features

RAGRIG_SPIKE_AUTORUN="Explain retrieval-augmented generation" \
RAGRIG_SPIKE_METRICS=1 \
  cargo run --release --example spike --no-default-features
```

`RAGRIG_SPIKE_METRICS=1` logs each bridge event's wall-clock time and lag.

## Features

| Feature | Default | Purpose |
|---|---|---|
| `ragrig` | yes | Ragrig controller and its Tokio bridge |
| `grobid` | no | GROBID/BibTeX reference ingestion via `grobid-bibtex` |

arXiv and Semantic Scholar discovery are deprecated and will not be ported.

## Checks

```sh
cargo fmt --check
cargo check --all-targets --all-features
cargo test --all-features
cargo clippy --all-targets --all-features -- -D warnings
cargo doc --no-deps --all-features
```

On a machine without the xkbcommon development packages, linking fails even
though the Rust crates compile. Install the packages above; do not paper over
the link error in CI.

The typing benchmark is opt-in:

```sh
cargo test --release --example spike --no-default-features \
  measure_typing -- --ignored --nocapture
```

## Notes for integrators

- Depend on `gpui-kit`, never on `gpui` directly. The facade keeps the backend
  swappable and is the version contract for this crate.
- `ragrig` runs on a dedicated Tokio runtime thread. Do not execute its
  futures on GPUI's executor.
- Secrets come from the environment (`DEEPSEEK_API_KEY`) and are never
  persisted.
- The checked-in `Cargo.lock` is intentional: `gpui-kit 0.7.3`'s optional
  `gpui-fast` branch pins `cocoa =0.26.0`, while a pre-existing lockfile may
  hold `0.26.1`. A fresh resolve succeeds; a stale one fails. Revisit this
  before the first crates.io release.

## License

MIT
