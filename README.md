# ragrig-gpui

Reusable [GPUI](https://gpui.rs) components for Ragrig: RAG chat, a streaming
Markdown transcript, a composer, source cards, attachment chips, session
picking, agent status, and settings. The crate has no editor; Sciscraper and
other hosts compose these pieces into their own layout.

Status: **M1 primitive library complete**. The public API may still change
before 0.2.0 (the M2 controller release). See [`PLAN.md`](PLAN.md) for the
milestones, decisions, and measured spike results.

## Quick start

Requirements:

- Rust 1.88 or newer. Tested with Rust 1.99.
- On Linux, the GPUI backend links `libxkbcommon` and `libxkbcommon-x11`.
  The runtime libraries are not sufficient:

```sh
sudo apt install libxkbcommon-dev libxkbcommon-x11-dev
```

Run the M1 gallery **from this crate's directory**:

```sh
cd ragrig-gpui
cargo run --example gallery --no-default-features
```

Or from anywhere, select the manifest explicitly:

```sh
cargo run --manifest-path /path/to/ragrig-gpui/Cargo.toml \
  --example gallery --no-default-features
```

`cargo run --example` resolves against the package in the current directory.
A parent directory that has its own `Cargo.toml` (as in a multi-crate
checkout) is a different package and will report `no example target named
gallery`.

`--no-default-features` skips the Ragrig dependency graph; the gallery uses a
local mock stream. The default build (`--features ragrig`) is the one M2 will
use.

### What to try

- Send a prompt: the answer streams from GPUI timers with live Markdown, a
  blinking streaming dot, and tail-following.
- Press **Stop**: the partial answer stays and is marked `stopped`.
- Copy a message or click a source chip under an assistant answer.
- Press **Retry** on the seeded error message.
- Toggle `references.bib`, drop `Selection`, or use the `Insert` action on the
  `refs.bib` offer.
- Pick a session from the picker above the transcript, or use the **Sessions**
  tab on the left. **New** clears the transcript and shows the empty state.
- Open the **Settings** and **Components** tabs on the right; change the model
  and generation fields. They are live in the UI but not wired to a controller
  until M2.
- Click an agent icon in the top bar for its popover and **Configure** action.
- Drag and stack the dock panels; the tabs and resizing are the real
  `DockArea` behaviour.

The M0 bridge spike remains available:

```sh
cargo run --example spike --no-default-features
```

It self-drives with `RAGRIG_SPIKE_AUTORUN="..."` and logs bridge timings with
`RAGRIG_SPIKE_METRICS=1`.

## Features

| Feature | Default | Purpose |
|---|---|---|
| `ragrig` | yes | Ragrig controller and its Tokio bridge (M2) |
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

Current suite: 31 library tests, 2 gallery tests, 5 active spike tests plus one
ignored typing benchmark, and 2 doctests.

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
- `ChatView` keeps one `TextViewState` per message. Append tokens with
  `ChatView::append`; it calls `push_str` on the right state and remeasures only
  the streaming row. Streaming bodies stay raw; `finish`/`stop` apply the
  fence stabilizer once.
- A true inline caret is not implemented: it would require changing the
  Markdown body. The current view uses a blinking status dot and a `streaming`
  label, leaving the text clean for selection and copy.
- `Composer` emits `Submit`/`Stop`; `ChatView` emits `Retry`, `SourceClicked`,
  and `Copied`. A controller can subscribe to those events without the widgets
  knowing about Ragrig.
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
