# ragrig-gpui — port plan

Status: M0 spike and M1 primitives complete (2026-10-17). M2 is next.
Date: 2026-10-17.

## 1. Starting point

- `ragrig-egui` 0.5.0 is 3,045 lines in nine modules: `chat_view`,
  `composer`, `attachments`, `sessions`, `agent_bar`, `dock`, `form`,
  `markdown`, and a small `lib.rs`. It is deliberately egui-only: no
  `ragrig`, no runtime; the host owns messages, input, and worker state.
- `ragrig-cli` 1.5.0 is a 5,257-line REPL plus `search.rs` (207),
  `grobid.rs` (887) and `cite.rs` (939). Its command surface is the
  specification for the demo: queries, attach/download, paper search,
  sessions, chat/embed/memory/parser/chunker hot-swap, profiles, corpora,
  BibTeX/GROBID, logs.
- `ragrig` 1.1.x is async, built on `tokio` (full) and `reqwest` 0.13.
  Its trait objects are `Send + Sync`, so an `AgentSession` can live in a
  dedicated Tokio actor. Progress and cancellation are synchronous callback
  traits (`Progress`, `Cancel`, `CancellationToken`).
- `ragrig-gpui` contains the M0 spike, the M1 primitive library and widget
  gallery, and the plan below. The crate names `ragrig-gpui` and
  `sciscraper-gpui` are free on crates.io.

## 2. What changed in the GPUI ecosystem

There are two incompatible GPUI lines on crates.io:

1. **Official `gpui` 0.2.2** (Zed, published 2025-10-22, edition 2024).
   `gpui-component` 0.5.1 (2026-02-05) is the last component line built on it.
2. **`gpui-pre` 0.3.9** (a published snapshot of Zed, 2026-10-12) used by
   the current **`gpui-base` 0.7.3 / `gpui-component` 0.7.3 / `gpui-kit`
   0.7.3** stack (GPUI Kit, Longbridge).

The current stack is substantially ahead: 75+ styled components, a
virtualized chat `MessageScroller` with tail-following and jump-to-latest, a
native Markdown `TextView`, a textarea with inline tokens and
`InputEvent::PressEnter { secondary, .. }`, `resizable`, `dock`, `table`,
`virtual_list`, `select`, `setting` pages, dialogs, sheets, popovers,
notifications, tooltips, and AccessKit semantics.

**Recommendation: `gpui-kit = "0.7.3"`.** It re-exports GPUI, base,
components, and Lucide assets through one facade. The library must use
`gpui_kit::*` (never import `gpui` directly) so the backend stays swappable.
The cost is dependence on a third-party published snapshot; pin the 0.7
minor, re-export `gpui_kit`, and document the compatibility contract.
The `gpui-fast` fork remains a host-level graph choice, not a feature of
this library.

Fallback if the spike fails: official `gpui` 0.2.2 + `gpui-component` 0.5.1.
That line is eight months old and lacks `MessageScroller`, the newer
`setting` framework, and several input improvements, so it is a fallback,
not the plan.

### Observed environment risk

This machine has `libxkbcommon.so.0` but neither `xkbcommon.pc` nor the
unversioned `libxkbcommon.so` linker symlink. The Rust crates compile, but
linking fails with `unable to find library -lxkbcommon`. Install
`libxkbcommon-dev` and `libxkbcommon-x11-dev` before building on Linux.
`fontconfig`, `wayland-client`, and `x11` are present. ALSA is only needed for
the optional `speech` feature.

## 3. Recommended architecture

### 3.1 Layers

```text
examples/gallery.rs (M1 widget gallery, no controller)
examples/ragchat.rs (M2 full demo: controller, library, sessions, settings)
        │
ragrig-gpui (library)
  model/          ChatState, messages, attachments, components, sessions, settings
  chat_view.rs    transcript + per-message TextViewState + source chips
  composer.rs     auto-growing input, send/stop events
  attachments.rs  attachment chips
  session_picker.rs, agent_bar.rs, side_panel.rs
  settings.rs     ChatSettings + SettingPage helper
  rag/            protocol, Tokio worker, RagController, RagChat, RagSettings
        │
gpui-kit 0.7.3 (gpui-pre + gpui-base + gpui-component + assets)
```

- `ragrig` is a default feature: the controller, adapters, sessions, indexing,
  and attachments are part of the normal build.
- `grobid` adds the BibTeX/GROBID path through `grobid-bibtex`.
- There is no `discovery` feature: arXiv and Semantic Scholar are deprecated.
- The M0 example builds with `--no-default-features` so the UI gate does not
  pay for the Ragrig dependency graph.

### 3.2 State ownership (hybrid)

- **Entities** own state that GPUI must retain: `ChatView` (messages +
  `MessageScrollerState`), `Composer` (`TextareaState` + busy flags),
  `SessionPicker`, `LibraryView`, `SettingsView`.
- **Plain builders** render host-owned data: `MessageBubble`, attachment
  chips, agent bar, section panel, source list rows, settings rows.
- Pure `ChatState::apply(ChatEvent)` and `LibraryState::apply(IndexEvent)`
  reducers carry the streaming/indexing semantics, so tests do not need a
  window and sciscraper can reuse the logic with its own view.
- `ChatView` is the batteries-included projection; `MessageBubble` is the
  escape hatch for hosts that own their transcript.

This keeps the egui crate's host-owned spirit without fighting GPUI's
retained entity model.

### 3.3 Runtime bridge

`ragrig` uses `tokio` + `reqwest`; GPUI's executor is not a Tokio reactor.
Do not run ragrig futures directly on GPUI's executor.

```text
GPUI thread                         dedicated std::thread
───────────                         ─────────────────────
Entity<RagController>               Tokio runtime (multi-thread)
  send RagCommand  ───────────────► actor owns AgentSession
  receive RagEvent ◄─────────────── emits tokens/progress/results
  update entities + notify           CancellationToken per job
```

- One long-running actor owns the `AgentSession`; a running operation takes
  the session out, runs in a spawned task, and returns it on completion.
  While busy, stateful commands are rejected; `Cancel`/`Status`/`Shutdown`
  are still accepted.
- Events travel over a bounded async channel and are drained on the GPUI
  side, batching tokens into one entity update per frame.
- The controller owns the Tokio runtime by default. An external
  `tokio::runtime::Handle` constructor can be added later if a host already
  has a runtime.

### 3.4 Chat and streaming

- `ChatState` is the source of truth; `ChatEvent` is
  `User`, `AssistantDelta(String)`, `AssistantDone { meta, sources }`,
  `Error`, `Cancelled`, `Clear`, `LoadTurns`.
- `ChatView` renders with `MessageScroller`:
  virtualized rows, `FollowMode::Tail`, a built-in jump-to-latest button,
  `Role::Log`, and `remeasure_items` for the streaming row.
- Assistant text uses `gpui_component::text::markdown(...)` (`TextView`).
  Keep both a mutable accumulation `String` and a cached `SharedString`
  rebuilt per token batch, so a frame does not clone/parse the whole answer.
- Batch tokens (~30–60 Hz) in the worker/controller. If live Markdown
  proves expensive, stream as plain text and switch to Markdown on
  completion; keep this behind one `StreamingRenderMode` switch.
- A cancelled or failed answer keeps any partial text and is marked
  `Stopped`/`Error`; metadata is attached only when the pipeline completes.

### 3.5 Layout and settings

- Main window: left `Library` panel, center chat, optional right
  `Sources`/`Settings` sheet, bottom `AgentBar`/status.
- Use `h_resizable` / `ResizablePanelGroup`; a thin `SidePanel` widget owns
  section headers and a horizontal collapse strip, not the resizing logic.
- Use the full `dock::DockArea` only when sciscraper-gpui needs tabs and
  nested splits. Porting the egui dock's drag internals would be reinvention.
- Settings use `Sheet` + `SettingPage` / `SettingGroup` / `SettingItem`.
  Appearance applies immediately; pipeline changes use an explicit Apply.
  Changing embedder or chunker warns that a reindex is required.

### 3.6 Composer and citations

`gpui-base`'s `TextareaState` already provides the behaviors the egui
composer implemented by hand:

- `auto_grow(1, 6)` for the six-row cap;
- `InputEvent::PressEnter { secondary, .. }` with the `secondary-enter`
  binding → Ctrl/⌘+Enter submits, plain Return inserts a newline;
- `InputEvent::Change` for enable/send state;
- `insert()`, `replace_all()`, `set_selected_range()`, `focus()` for
  programmatic insertion;
- `InlineToken` spans for atomic `@Key` citations, with click/hover
  callbacks — a direct fit for sciscraper's reference attach.

`Composer::append_text` inserts at the caret (or at the end, configurable)
and keeps focus. `Composer::append_citation` can use an inline token later.

## 4. Port mapping

| ragrig-egui | ragrig-gpui replacement |
|---|---|
| `chat_view::ChatView` | `ChatView` entity + `MessageBubble`; `MessageScroller`; `text::markdown` |
| `chat_view::MessageMeta` | plain `MessageMeta` + `SourceRef` list from `RagResponse.retrieved` |
| `composer::Composer` | `Composer` entity over `TextareaState`; send/stop; submit event |
| `composer::append` | `Composer::append_text` / `append_citation` using `insert` + focus |
| `attachments` | plain `Attachment` model + chip row; `Button`/`Tag` + tooltips |
| `sessions::SessionRow` | `SessionPicker` entity backed by `Select` / `DropdownMenu` |
| `agent_bar::AgentBar` | stateless `AgentBar` + `Popover` menus; status dots from theme tokens |
| `dock::SectionDock` | `SidePanel` section stack inside `ResizablePanelGroup` |
| `form::{setting_row,note,warning}` | `gpui_component::setting` (`SettingGroup`/`SettingItem`) |
| `markdown` (pulldown-cmark) | delete; use `gpui_component::text::markdown` |
| `ChatRole`, `MessageState`, `ComponentStatus`, `AttachmentKind` | keep as plain enums; add `ComponentKind` and `LibraryAction` enums |

## 5. Demo scope mapping from ragrig-cli

| CLI surface | GUI equivalent |
|---|---|
| any text | composer → chat |
| `/attach`, `/attach clear` | attachment chips + file prompt/drop; one-shot clear on send |
| `/download <url>` | "Add URL" action routed to a selected corpus |
| `/scholar`, `/arxiv`, `/get` | Discover view: search, results table, bulk download |
| `/refs` | source-list action "extract references" (feature-gated) |
| `/chat ...` | Settings → Chat (provider, model, params, context, prompt) |
| `/embed ...` | Settings → Embedding + Library index actions (index/update/purge/top-k/threshold) |
| `/memory ...` | Settings → Memory (strategy, model, rewrite prompt) |
| `/parser`, `/chunker` | Settings → Parsing/Chunking |
| `/prompt ...` | Settings → prompt file pickers |
| `/profile ...` | Profile menu: save/load/list (and delete) |
| `/corpus ...` | Library → corpora list, on/off, sync/rebuild/remove |
| `/bibtex ...` | Settings → Bibliography + composer `@key` tokens (feature `grobid`) |
| `/hist ...` | Session picker |
| `/log ...` | Activity sheet / log level; MVP shows operations and errors |
| `/help`, `exit` | Help sheet; window close |

No slash-command parser is needed. A command palette (`command` component)
can optionally expose the same actions to power users.

## 6. Milestones

Estimates are rough, single developer, excluding upstream surprises.

### M0 — Foundation spike (done, 2026-10-17)

Delivered in `examples/spike.rs` and `src/`:

- `DockSkin::dock_area` with a center chat panel, a left Library panel, and a
  right Settings panel (all built with `DockLayout::tabs()`).
- Live Markdown `TextView` rows inside a tail-following `MessageScroller`.
- Auto-growing `Textarea` (`auto_grow(1, 6)`); Ctrl/⌘+Enter submits, Return
  inserts a newline.
- Fake token stream on a dedicated Tokio thread, unbounded `async-channel`,
  `cx.spawn` bridge, ~30 Hz batches, generation guard, and `CancelFlag` stop
  that keeps the partial answer.
- `Settings` page with dropdown, number, switch, and input fields; `Popover`
  agent menu; Library placeholder.
- 5 example tests, 1 library test, 1 doctest, plus an ignored typing benchmark.

**Gate: passed.**

Measured on this machine (release, 1600×1000 window):

| Metric | Release | Debug |
|---|---|---|
| 444-char answer, 18 batches | 562 ms | not measured end-to-end |
| Token batch lag (worker send → UI handler) | 40–80 µs | 200–260 µs |
| 500 simulated keystrokes | 72 ms (144 µs/char) | 913 ms (1.83 ms/char) |
| Cold example build (deps included) | 49.8 s | not measured |
| Warm rebuild | 1.2 s | 1.5 s |

The typing numbers include the test harness's per-key `run_until_parked`, so
they are upper bounds, not paint latency.

**Findings and consequences for M1**

- Linux links `-lxkbcommon` and `-lxkbcommon-x11`; the runtime `.so.0` files
  are not enough. `libxkbcommon-dev` and `libxkbcommon-x11-dev` are required.
- `gpui-kit`'s optional `gpui-fast` branch depends on `cocoa =0.26.0` while
  `gpui-pre` resolves `cocoa 0.26.1`. A fresh resolution picks 0.26.0 and
  succeeds; a stale lockfile fails. Keep `Cargo.lock` and document the pin.
  This is a real risk for consumers with an existing workspace lockfile.
- Declared MSRV is 1.88 because `ragrig` requires it. `gpui-kit` declares no
  MSRV; only Rust 1.99 has been tested. Do not claim support below 1.99 yet.
- GPUI's test scheduler rejects foreground wakeups from other threads, so the
  Tokio bridge is tested by a plain worker test. Keep controller tests outside
  the GPUI scheduler and reserve `#[gpui_kit::test]` for input and reducers.
- Rebuilding Markdown from a growing `String` re-parses the full message per
  batch and displays an unclosed code fence as a code block mid-stream. M1
  must own an `Entity<TextViewState>` per assistant message and use
  `TextViewState::push_str` (or a stabilized streaming tail).
- `WindowBounds::Maximized` behaved like a windowed restore size on KWin/X11;
  explicit bounds are reliable. The UI scale on this HiDPI X11 setup maps
  logical to roughly physical pixels.
- Not covered in M0: `Select`, file dialog/drop, resizable split, and dock
  layout persistence. `Select` is straightforward; the dialog/drop and
  persistence land with the library work.

### M1 — Primitive port, mock data (done, 2026-10-17)

Delivered:

- `model/`: `ChatRole`, `MessageState`, `ChatState`, `MessageMeta`, `SourceRef`,
  `NewMessage`, `Attachment`, `ComponentKind`/`ComponentStatus`, `SessionEntry`,
  `ChatSettings`; all reducers are pure and unit-tested.
- `ChatView`: tail-following transcript with one `Entity<TextViewState>` per
  message, incremental `push_str`, final fence stabilization, streaming dot,
  stopped/error states, metadata footer, source chips, copy and retry events.
- `Composer`: auto-grow input, Ctrl/⌘+Enter submit, Return newline, Send/Stop
  events, draft/focus/clear helpers, streaming and disabled state.
- `AttachmentRows`, `SessionPicker`, `AgentBar` with per-component popovers,
  `SidePanel`, and `chat_settings` / `chat_settings_page`.
- `init(cx)`, `prelude`, `#![warn(missing_docs)]`, no `unsafe`.
- `examples/gallery.rs`: seeded complete/stopped/error transcript, live mock
  stream, two-tab left and right docks, registered dock panels, settings and
  component views.
- 31 library tests, 2 gallery tests, 5 active spike tests + 1 ignored
  benchmark, 2 doctests. Clippy `-D warnings` clean.

Deviations and findings:

- A true inline streaming caret would require modifying the Markdown text. The
  view uses a blinking status dot plus a `streaming` label instead; the text
  stays clean for selection and copy.
- Streaming bodies stay raw in the `TextViewState`; `finish`/`stop` apply
  `stabilize_markdown` once, so an open fence cannot swallow later tokens.
- The GPUI test scheduler cannot advance a foreign worker thread, so the
  gallery's mock stream uses GPUI timers and deterministic `advance_clock`
  tests. The M0 Tokio worker test remains outside the GPUI scheduler.
- `CancelFlag` is not wired to the gallery: there is no controller yet. The
  Composer's `Stop` event is the seam M2 will connect to
  `CancelFlag`/`CancellationToken`.
- Dock panels are registered, but the gallery does not load or save layouts;
  M3 adds `.ragrig/gpui.json` persistence.

### M2 — Controller and RAG chat demo (done, 2026-10-17)

Delivered:

- `src/rag/`: `RagCommand`/`RagEvent` protocol, dedicated multi-thread Tokio
  worker, `RagController` entity, full `RagSettings` page, and the composite
  `RagChat` surface (transcript + attachments + composer).
- Streaming tokens are batched on the worker (~30 Hz) and delivered through a
  bounded `async_channel`; `Stop` cancels through Ragrig's
  `CancellationToken` and keeps the partial answer. Non-streaming mode is a
  setting.
- Agent construction from `RagrigConfig` (chat, embedder, parser filter,
  chunking, memory strategy, system prompt). DeepSeek keys are read from
  `DEEPSEEK_API_KEY` only and never enter a profile.
- Sessions: list, load, new, auto-save after every successful turn; Apply
  rebuilds the agent and reloads the current session when it was saved.
- Library: incremental folder ingestion with progress, cancel, document list,
  path resolution, and one-shot file attachments on the next question.
- Settings: workspace, provider, models, temperature/tokens, prompt, embeddings,
  retrieval, PDF backend, chunking, memory, profiles (`name=path` corpora are
  kept across applies).
- `examples/ragchat.rs`: four dock panels (Chat, Library, Sessions, Settings,
  Components), onboarding banner, agent bar, session picker, status line, and
  `RAGRIG_WORKSPACE` / `RAGRIG_CORPUS` / `RAGRIG_MODEL` /
  `RAGRIG_EMBED_MODEL` / `RAGRIG_ASK` environment hooks.
- 44 library tests (including six actor tests with a fake generator and
  embedder), 2 gallery tests, 5 active spike tests, 2 doctests; clippy clean.

Acceptance: verified manually against Ollama with `llama3.2:3b` and
`nomic-embed-text`: indexed a Markdown corpus (1 document, 7 chunks), streamed
an answer with `7 chunks · 1 source · 5.1 s`, displayed the source chip, and
reloaded the saved session from the picker. An empty workspace streamed a pure
chat answer with `0 documents · 0 chunks`. The 3.3 MB PDF fixture parsed and
indexed concurrently (4,757 chunks, progress visible, interruptible).

Findings:

- The initial worker path never sent `AgentReady` (only `Apply` did). The
  controller therefore stayed on "Starting the RAG worker…". Unified the
  announcement after any build.
- `BruteForceStore` deep-copies on clone in some backends: ingesting through a
  cloned `RagAgent` left the session's store empty. Ingestion now holds the
  session lock and runs on the session's own agent; generation requests wait
  for indexing to finish.
- `ChatView::remove` was added so retry can drop the failed message before
  resending its prompt without echoing the user turn.
- Widget callbacks that mutate their own entity must be deferred
  (`cx.defer`); the retry path does this to avoid a re-entrant update.

Release: **0.2.0**.

### M3 — Library management (4–6 days)

- Corpora (dir/URL), add/remove/toggle, sync/rebuild/purge, progress and
  cancel, per-document rows from `stored_documents()`, filter/search.
- Add files/folder via path prompt and drop; download URL; attach a
  library document to the next query; delete/reindex/reveal row actions.
- Pending/changed detection with `get_changed_documents` where available.
- Profiles save/load/list; workspace `.ragrig` compatibility; UI layout
  persistence in `.ragrig/gpui.json`; secrets never persisted.
- Acceptance: two corpora, incremental update, remove one corpus, attach
  a document, restart and recover state.

### M4 — GROBID and bibliography (3–6 days)

- Feature `grobid`: depend on `grobid-bibtex` 0.2.1 (`openalex` feature);
  use its `Collection::cite_entries` path rather than duplicating parsing.
- Pre-pass/rename, BibTeX merge status, `@key` lookup and inline-token
  citation attach, `/refs` extraction.
- arXiv and Semantic Scholar discovery are deprecated: do **not** port
  `ragrig-cli/src/search.rs` and do not add a `discovery` feature.
- Acceptance: with a running GROBID server, rename and merge entries and let
  `@key` attach the parsed paper to the prompt.

### M5 — Hardening and publication (3–5 days)

- Accessibility: labels for icon buttons, focus order, log role,
  reduced-motion busy pulse.
- Performance: large transcript, long answer, virtual list, token batching,
  no per-frame full-text clones; release-profile checks.
- Docs: README (GPUI Kit pin, quick start, compatibility), Changelog,
  examples; verify `cargo package --list` contains the intended example(s).
- Green checks: fmt, check all targets/features, test, clippy `-D warnings`,
  doc, package, publish dry-run. E2E with Ollama/GROBID remains manual or
  nightly.
- Acceptance: demo works on Linux; macOS/Windows smoke-tested; package
  metadata complete.

## 7. Decisions (approved)

| # | Decision |
|---|---|
| D1 | **`gpui-kit = "0.7.3"`** as the single facade; the library never imports `gpui` directly. Fallback: official `gpui 0.2.2` + `gpui-component 0.5.1`. |
| D2 | **`ragrig` is a default feature**; the controller lives in the library and the example uses it from M2. |
| D3 | Tiered releases: **0.2.0 after M2**, **0.3.0 after M3**. |
| D4 | **Single crate + example**: no workspace member; `examples/spike.rs` (M0), `examples/gallery.rs` (M1), `examples/ragchat.rs` from M2. |
| D5 | **Hybrid state**: entities for chat/composer/pickers/library/settings; plain builders for rows, chips, and the agent bar; pure reducers. |
| D6 | **Controller in the library**; no slash-command parser. |
| D7 | **Dedicated Tokio runtime thread**; one actor owns `AgentSession`; bounded events; batched tokens; `CancellationToken`. |
| D8 | **`DockArea`** for the demo because Sciscraper will use tabs. |
| D9 | **Secrets from the environment only** (`DEEPSEEK_API_KEY`); never persisted. |
| D10 | **M4 is GROBID/BibTeX only**; arXiv and Semantic Scholar are deprecated. GROBID logic comes from `grobid-bibtex`. |

Defaults carried into M1: `MessageScroller` + incremental Markdown,
keep partial answers on stop, `Settings` pages, Lucide via
`gpui-kit-assets`, manual Ollama/GROBID E2E.

Resolved by the user or by D10:

1. No paper discovery (arXiv/Semantic Scholar) in any release.
2. GROBID/BibTeX is the final milestone, not part of the first release.

Defaults for the remaining open questions (say so if any should change):

1. Profiles stay CLI-compatible; secrets never enter profiles.
2. One window with dock panels; no separate library/settings windows.
3. No offline fixture mode in the first pass.
4. Buttons/menus only; no command palette.
5. Publish primitives + `RagChatPanel`, not a turnkey app.
6. Linux first; macOS/Windows smoke tests at M5.

## 8. Risks and mitigations

| Risk | Mitigation |
|---|---|
| `gpui-pre` snapshot churn | Pin `gpui-kit = "0.7.3"`/0.7.x; re-export it; keep widgets thin; upgrade deliberately |
| Link failure on Linux | Install `libxkbcommon-dev` and `libxkbcommon-x11-dev`; runtime `.so.0` files are insufficient. Documented in README |
| `cocoa` resolution conflict on macOS | `gpui-fast` pins `cocoa =0.26.0`; keep a fresh `Cargo.lock`, and warn consumers whose workspace locks 0.26.1. Re-evaluate before publishing; fall back to official `gpui` if it persists |
| `gpui-fast` changes type identity | It is a backend choice for the host, not a feature of this crate; never import `gpui` directly |
| Live Markdown re-parses per batch | M1: one `TextViewState` per message with `push_str`; measure again; keep a plain-text streaming fallback |
| Unclosed code fences mid-stream | M1: stabilize the streaming tail (close fences, hide the trailing partial block) |
| Virtual list remeasure on streaming row | `remeasure_items` only the last row; test long answers |
| Tokio/GPUI bridge deadlocks | Single actor; bounded channel; batching on the GPUI side; cancellation token |
| GPUI test scheduler rejects cross-thread wakes | Test controller/worker code outside `#[gpui_kit::test]`; keep UI tests deterministic |
| `WindowBounds::Maximized` ignored by some WMs | Use explicit bounds and verify on the target platform |
| CLI parity is large | Tiered scope; controller reuse; no command-parser port |
| Secrets in profiles | Scrub/skip keys; env/session only |
| GROBID is fresh (0.2.1, published 2026-10-16) | Feature-gate; depend on crates.io, no path; defer if unstable |
| Compile time / binary size | Dev profile opt-level for deps; no unnecessary tree-sitter features |

## 9. Explicit non-goals for the port

- Do not port the custom `pulldown-cmark` renderer. Use `TextView`.
- Do not port the egui Markdown `Block` model.
- Do not port panel resizing or section fold logic. Use `DockArea` and
  `ResizablePanelGroup`.
- Do not port form helpers beyond a thin compatibility shim. Use `setting`.
- Do not port icons or theme colors. Use `gpui-kit-assets` and semantic tokens.
- Do not build an editor. The composer is the only text input.
- Do not emulate slash commands. UI actions replace them.
- Do not depend on GPUI under the name `gpui` anywhere in the library.

## 10. Validation commands

```sh
cargo fmt --check
cargo check --all-targets --all-features
cargo test --all-features
cargo clippy --all-targets --all-features -- -D warnings
cargo doc --no-deps --all-features
cargo run --example spike --no-default-features
```

On this machine, prepend the local link shim if the dev packages are not
installed:

```sh
RUSTFLAGS="-L native=$PWD/target/xkb-shim" cargo check --all-targets --all-features
```

The shim is a development aid, not part of the crate. `libxkbcommon-dev` and
`libxkbcommon-x11-dev` are the supported setup.

Feature matrix:

```sh
# UI only, the M0 gate
cargo test --no-default-features

# default: controller dependency included
cargo test

# all optional integrations (ragrig + grobid)
cargo test --all-features
```

There is no `discovery` feature. Do not add a `gpui-fast` feature to this
crate: it changes the backend for the whole dependency graph.

## 11. Next implementation step

M3: deepen the library. Corpora list with enable/remove/sync/rebuild/purge,
per-document chunk counts and actions, drag/drop paths, attach a library
document (already wired in the M2 demo), pending/change detection, and dock
layout persistence in `.ragrig/gpui.json`. Apply the same worker protocol;
do not move Ragrig futures onto GPUI's executor.
