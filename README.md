# ragrig-gpui

Reusable [GPUI](https://gpui.rs) components for Ragrig: a streaming RAG chat,
a Markdown transcript, source chips, attachment context, session picking, agent
status, a full settings page, and a dedicated Tokio controller. The crate has
no editor; Sciscraper and other hosts compose these pieces into their own
layout.

Status: **M2 complete (0.2.0)** — controller, RAG chat demo, sessions, library
ingestion, and settings. The public API may still change before 1.0. See
[`PLAN.md`](PLAN.md) for milestones, decisions, and measured results.

## Quick start

Requirements:

- Rust 1.88 or newer. Tested with Rust 1.99.
- On Linux, GPUI links `libxkbcommon` and `libxkbcommon-x11`; the runtime
  libraries are not sufficient:

```sh
sudo apt install libxkbcommon-dev libxkbcommon-x11-dev
```

- A local [Ollama](https://ollama.com) server with the configured chat and
  embedding models for the full RAG demo. Without it the window still opens;
  requests report the connection error in the status line.
- Alternatively set `DEEPSEEK_API_KEY` and select DeepSeek in Settings. Keys
  are read from the environment only and never persisted.

Run the M2 demo from this crate's directory:

```sh
RAGRIG_WORKSPACE=/tmp/ragrig-demo \
RAGRIG_CORPUS=/path/to/papers \
RAGRIG_MODEL=llama3.2:3b \
cargo run --example ragchat
```

`RAGRIG_WORKSPACE` defaults to the current directory. `RAGRIG_CORPUS` indexes
a folder on startup; `RAGRIG_MODEL` and `RAGRIG_EMBED_MODEL` override the
settings defaults; `RAGRIG_ASK` submits a first question after the window
opens (use it with an already indexed workspace, because indexing and asking
are serialised).

Or select the manifest explicitly from anywhere:

```sh
cargo run --manifest-path /path/to/ragrig-gpui/Cargo.toml --example ragchat
```

### What to try

> `cargo run --example` resolves against the package in the current directory.
> A parent directory with its own `Cargo.toml` is a different package and
> reports `no example target named ragchat`.

- **Library** (left dock): enter a folder, press **Index**, watch documents and
  chunks appear. Indexing is incremental and can be stopped.
- Click the paperclip on a document row to attach it to the next question.
- **Chat** (center): ask a question. Tokens stream, the footer reports chunks,
  sources, tokens, and elapsed time. **Stop** keeps the partial answer; retry a
  failed answer with the last prompt.
- **Sessions** (left dock): new, load, and refresh saved conversations. Apply
  in Settings rebuilds the agent and reloads the current saved session.
- **Settings** (right dock): workspace, provider, models, generation, prompt,
  embeddings, retrieval, PDF backend, chunking, and memory. **Apply** validates
  and rebuilds. **Profiles** save/load JSON under `.ragrig/profiles`.
- **Components** (right dock): live Ragrig pipeline status.
- Chat with an empty library; Ragrig falls back to pure chat.

The M1 widget gallery remains available and needs no Ragrig backend:

```sh
cargo run --example gallery --no-default-features
```

The M0 bridge spike is still there:

```sh
cargo run --example spike --no-default-features
```

## Features

| Feature | Default | Purpose |
|---|---|---|
| `ragrig` | yes | `RagController`, `RagChat`, `RagSettings`, Tokio worker |
| `grobid` | no | GROBID/BibTeX reference ingestion via `grobid-bibtex` |

arXiv and Semantic Scholar discovery are deprecated and will not be ported.

## Architecture

```text
host (e.g. examples/ragchat.rs)
  DockArea, panels, agent bar, session picker
        │
ragrig-gpui
  ChatView / Composer / AgentBar / SessionPicker / ...
  RagChat  ── transcript + attachments + composer wiring
  RagController ── commands/events, mirrored status
        │  bounded async_channel
  worker thread ── multi-thread Tokio runtime
        │
  AgentSession ── RagAgent, vector store, session store
```

- `RagController::new(RagSetup, cx)` starts one worker thread per controller.
- `RagChat::new(window, cx, controller)` wires the M1 primitives and re-emits
  `RagChatEvent`s for status, source clicks, and copy.
- `RagSettings::to_setup()` validates and converts to `RagrigConfig` plus a
  prompt override and stream flag.
- `rag_settings(id, settings, controller, cx)` builds the complete page;
  Apply is explicit.

## Checks

```sh
cargo fmt --check
cargo check --all-targets --all-features
cargo test --all-features
cargo clippy --all-targets --all-features -- -D warnings
cargo doc --no-deps --all-features
```

Current suite: 44 library tests (including six actor tests with a fake
generator and embedder), 2 gallery tests, 5 active spike tests plus one
ignored typing benchmark, and 2 doctests.

On a machine without the xkbcommon development packages, linking fails even
though the Rust crates compile. Install the packages above; do not paper over
the link error in CI.

## Notes for integrators

- Depend on `gpui-kit`, never on `gpui` directly. The facade keeps the backend
  swappable and is the version contract for this crate.
- Ragrig runs on a dedicated Tokio runtime thread. Never drive its futures on
  GPUI's executor.
- Streaming tokens are batched on the worker; the UI receives roughly 30
  batches per second, not one wakeup per token.
- Inside a widget's own event callback, defer mutations of that widget with
  `cx.defer`. `RagChat` does this for retry.
- `ChatView` keeps one `TextViewState` per message and appends with
  `push_str`. Streaming bodies stay raw; `finish`/`stop` stabilise fences once.
- A true inline streaming caret is not implemented: it would mutate the
  Markdown body. The view uses a blinking status dot instead.
- Ingestion holds the session lock and runs on the session's own agent.
  Cloning some vector stores deep-copies them; do not ingest through a cloned
  agent.
- Secrets come from the environment (`DEEPSEEK_API_KEY`) and are never
  persisted. Profiles contain `"deepseek_api_key": null`.
- The checked-in `Cargo.lock` is intentional: `gpui-kit 0.7.3`'s optional
  `gpui-fast` branch pins `cocoa =0.26.0`, while a pre-existing lockfile may
  hold `0.26.1`. A fresh resolve succeeds; a stale one fails.

## License

MIT
