# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## Unreleased

### Added

- `RagField`, `RagSection`, and `RagChanges`: field-level pending-change
  tracking. `RagSettings::dirty_since(&applied)` reports the changed fields,
  and `RagChanges` answers per-field and per-section questions.
- `RagSettings::reset_to_profile_defaults()` reloads the selected profile, or
  restores the built-in defaults while preserving the workspace and
  bibliography.
- `SettingRow` with `dirty_color` and `dirty_header`: a settings row whose title
  becomes bold violet while the value differs from the applied configuration.
- Reusable row controls: `text_control`, `number_control`, `switch_control`,
  `dropdown_control`, `path_control`, and `token_control`.
- `rag_model_page` and `rag_references_group` take a `Changes` closure so hosts
  can show unsaved marks; `no_changes()` keeps the standalone use simple.

### Changed

- **Breaking:** `rag_model_page(settings, changes, cx)` no longer takes the
  controller and no longer renders the Profiles and Apply groups. Hosts place
  those on their own Overview page; `rag_settings_page` still renders the full
  standalone page.
- The agent bar uses `Layers` for the vector store and `Columns3` for the
  chunker. The previous `Server` and `Hash` SVGs contain `<line>` elements that
  the SVG rasteriser drops, which left blank icons.

### Fixed

- Path fields shrink to the controls column; the **Browse…** button no longer
  clips when the settings surface is narrow.
- Vertical rows (system prompt, folder corpora) now give their control the full
  row width.

## v0.2.0 — 2026-10-17

The first publication. The version stays at 0.2.0 because the planned 0.3.0
bump is tied to M3 (deeper library management), which is still open.

### Added

- **GPUI primitives**: chat view with a virtual `MessageScroller`, incremental
  Markdown, source chips, message metadata and copy/retry actions; composer
  with submit/stop and attachment chips; agent status bar; session picker;
  settings helpers; titled, scrollable side panels.
- **Ragrig controller** (`ragrig` feature, default): one dedicated Tokio
  runtime thread per controller, a bounded command/event bridge, streamed
  token batching, cancellation, and a complete `RagSettings` page. The agent
  is rebuilt on Apply; profiles are stored under `.ragrig/profiles`.
- **`RagChat`**: transcript + attachments + composer wiring with status,
  source-click and copy events.
- **Library ingestion**: named folder corpora, incremental indexing, progress
  and cancellation, and per-document rows with paths from the vector store.
- **GROBID references** (`grobid` feature): the `grobid-bibtex` pre-pass,
  progress/report types, PDF path resolution, and two-line reference rows with
  hover **Copy @key** and **Copy path** actions.
- **Roundtrip citations**: `@Key` mentions in a question are resolved against
  `<workspace>/.ragrig/references.bib` and the active corpora, parsed, and
  attached as one-shot context (3 papers, 100,000 characters each). The
  transcript shows an `Attached: @Key` note and any misses above the answer.
  The switch lives under Settings → References and is on by default.
- **Context formatting**: `SciscraperAttach` emits the co-writer prompt's
  `<sciscraper:v1:context source="creator">` / `source="corpus"` containers,
  budgets explicit attachments at 70% of the configured context window, and
  marks shortened papers with `<sciscraper:v1:truncated .../>`.
- **Reference multi-select**: `ReferenceSelection` keyed by citation key, a
  hover-revealed **+** on each row, and `ReferenceSelectionBar` with bulk
  **Copy @keys**, **Copy paths**, **Attach to next question**, and **Clear**.
- Examples: `spike` (M0 bridge), `gallery` (M1 primitives), and `ragchat`
  (the full RAG/reference demo, available with the `ragrig` feature).

### Fixed

- Host prompts are applied through `RagAgent::set_system_prompt`, which
  appends `{context}` when missing. Previously a settings prompt without the
  placeholder silently dropped retrieval and attachments.
- Attachment context is budgeted against `context_tokens`, so a cited PDF no
  longer pushes itself (and the question) out of a smaller model's window.

### Known limitations

- Library management is additive: the demo can index a folder and inspect
  documents, but it has no corpus removal/purge or document delete/reindex
  action, and dock layout is not persisted. These remain M3.
- The composite `RagChat` widget and the settings page have no direct
  automated tests; their parts are covered separately or exercised manually.
- arXiv and Semantic Scholar discovery are intentionally absent; GROBID/BibTeX
  is the only reference integration.

### Quality

- **Test coverage** (`cargo llvm-cov --all-features`; unit and example tests;
  doctests are not measured on stable Rust):
  **64.1% lines** (2,446 missed of 6,811), **66.4% regions** and **60.7%
  functions**. The reusable library modules are mostly well covered:
  `cancel.rs`, `side_panel.rs`, `markdown.rs` (lines), `model/session.rs`,
  `model/settings.rs` and `lib.rs` at 100%; `rag/attach.rs` 97.7%,
  `model/chat.rs` 91.2%, `model/attachment.rs` 89.6%, `chat_view.rs` 86.6%,
  `attachments.rs` 86.5%, `references.rs` 82.1%, `composer.rs` 79.5%,
  `model/component.rs` 79.0%, `rag/config.rs` 76.4% and `rag/actor.rs` 74.4%
  line coverage.
  The uncovered surface is concentrated in GPUI composition and settings
  rendering: `rag/chat.rs` and `rag/settings_page.rs` 0.0%,
  `rag/controller.rs` 13.7%, `session_picker.rs` 39.7%, `rag/protocol.rs`
  43.6%, `agent_bar.rs` 65.8% and `settings.rs` 65.0% line coverage. The
  worker itself is exercised by nine actor tests with a fake generator and
  embedder.
- **Tests**: `cargo test --all-features` passes **69 library tests, 2 gallery
  tests, 5 spike tests and 2 doctests**; the typing benchmark and the GROBID
  end-to-end test are ignored by default (2 ignored).
- **Live GROBID**: `http://localhost:8070` was running. The ignored
  `grobid_extracts_the_test_library_end_to_end` test passed: it copied the
  three `test_library/` PDFs, extracted and renamed them, merged the
  bibliography, verified every row's PDF path, and resolved the first key
  through the real PDF parser. The demo then answered a question containing
  `@Kano2025` from the attached paper's body text with llama3.2:3b at a
  4096-token context.
- **Verification**: `cargo fmt --check`, `cargo check --all-targets
  --all-features`, `cargo test --all-features`, `cargo clippy --all-targets
  --all-features -- -D warnings` and `cargo doc --no-deps --all-features` are
  green. The feature matrix also checks with default features and with
  `--no-default-features`.
- **Package**: `cargo package --list --allow-dirty` ships 38 files. The local
  `test_library/` corpus is excluded from the package. `cargo publish
  --dry-run --allow-dirty` packaged and verified the crate (738.6 KiB,
  172.1 KiB compressed) and resolved `ragrig` 1.1.2, `grobid-bibtex` 0.3.0,
  `gpui-kit` 0.7.3 and `biblatex` 0.12.0 from crates.io. No release blockers
  were found; `--allow-dirty` was needed only because the release changes were
  not committed yet.
