# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## Unreleased

### Added

- **Layered Ragrig configuration.** `RagSettings::from_layers(workspace)`
  merges `~/.ragrig/config.json` and `<workspace>/.ragrig/config.json` per
  setting over the built-in defaults (workspace last). `home_ragrig_dir()`
  honors `RAGRIG_HOME`. Profile listing now includes global profiles and
  `load_profile` falls back from the workspace to `~/.ragrig`; a workspace
  profile still wins and **Update Profile** stays workspace-local.
- **Project data directory.** `RagSetup::data_dir` selects where the vector
  store, sessions, and profiles live (default `<workspace>/.ragrig`). The
  store therefore opens at `<data_dir>/.ragrig_store` instead of the workspace
  root, which keeps a Sciscraper project to the layout described in its
  `.sciscraper` descriptor.
- **Switchable status-bar components.** `AgentComponent` now carries
  `ComponentToggle` switches, rendered in its popover. `AgentBarEvent::Toggle`
  reports `{component, toggle, enabled}` to the host. `ComponentKind::Memory`
  and `IconName::Brain` label the new voice. `RagController::agent_components()`
  adds **Chat memory** and extends **Hybrid retrieval** with switches, and the
  controller mirrors both values through `memory_enabled()` and `hybrid()`.
- **Host-supplied component marks.** `AgentComponent::icon` overrides the
  catalog icon of one pipeline component; `AgentBar` falls back to the kind's
  icon for raster marks. `RagController::agent_components()` now places **Chat
  memory** directly after the chat model. `Composer::send_icon` / `send_icon()`
  brand the send button.
- **Full-colour marks.** `ChatIcon::png` wraps image bytes for `img` rather
  than the alpha-masked `svg` element, so a multi-colour logo keeps its
  colours. `EmptyState` renders such a mark at 56 px; `ChatIcon::as_icon()`
  still returns the monochrome form used by buttons.
- **Deferred numeric validation.** `number_control` now leaves the draft alone
  while typing, commits on Enter or blur, applies stepper clicks immediately,
  and shows the accepted range under the field when a draft is rejected. It
  keeps the previous value instead of clamping mid-keystroke.
- **Hybrid retrieval switch.** `RagSettings::hybrid` (default on) and
  `RagSetup::hybrid` reach the worker. Off builds `EmbedderSpec::None`, so no
  embedding model is required and vector search is skipped; attached documents
  and the bibliographic library still reach the prompt. Ingestion reports a
  clear error while hybrid retrieval is off. `AgentInfo::hybrid` reports the
  built embedder state, and `RagField::Hybrid` tracks the settings row.
- **`RagSetup::memory_enabled()`** reports whether the configured memory
  strategy is anything but `Off`.
- **Retrieval enrichment.** `RagCommand::Ask` carries an optional `retrieval`
  text and `RagController::ask_with_retrieval` sets it. The worker prepends it
  to the prompt before embedding and searching, so a host can let a marked
  passage drive similarity search; the host transcript still echoes only the
  question. `RoundtripContext::text()` exposes the raw passage for this.
- **Prompt files in `RagSettings`.** `system_prompt_path` selects a Markdown
  file that overrides the inline prompt; `built_in_prompt` carries the host's
  compiled-in default. `prompt_path()` resolves a relative path against the
  workspace, `effective_prompt()` reads the selected file, and
  `use_built_in_prompt()` clears the override. Profiles round-trip the prompt
  file and preserve the host's built-in prompt.
- **`PromptActions`** adds host buttons to the generic system-prompt row:
  **Open prompt**, **Export <built-in name>**, and **Use built-in**. A host
  supplies callbacks to open its editor and export its compiled prompt;
  `rag_model_page` takes `Option<PromptActions>`.
- **Editor roundtrip contract.** `editor::EditorSurface` describes the minimal
  editor a host plugs in; `editor::RoundtripHost` supplies the active document,
  selection, and guarded write-back (with the window needed by a GUI editor);
  `editor::Roundtrip` builds the document
  and selection containers and routes replacements. The protocol
  (`RoundtripContext`, `Replacement`, `verified_bytes`, `replace_at`) moved here
  from `sciscraper-gpui`, so the editor backend is now swappable.
- **Reference editing in the roundtrip.** `EditorSurface::reveal` places the
  caret at a character offset and scrolls it into view. `RoundtripHost` gains
  `bibliography()` and `open_at(path, offset)`, and its default
  `edit_reference(key)` reads the configured `.bib`, locates the entry with the
  new `editor::bibtex_entry_offset`, and asks the host to open it. Hosts that
  keep the bibliography in memory override `edit_reference` to use the buffer.
  `Roundtrip::{edit_reference, open_at}` expose the seam, and
  `ReferenceRow::on_edit` adds the **Edit in BibTeX** action.
- **Insertion in the editor contract.** `EditorSurface::insert_text` replaces
  the selection or inserts at `EditorSurface::caret` as one undo step, and the
  default `caret()` derives a character index from the snapshot's line/column.
  Hosts implement `RoundtripHost::insert`; `Roundtrip::insert` exposes it.
  `char_index_for_line_column` is re-exported for hosts with their own caret
  representation. This is the Library → Editor seam.
- `ReferenceAction::Insert`: the reference selection bar now offers insertion
  next to attach and clear, and all three actions carry labels and visible icon
  placeholders. The bar is full width and includes the selection summary
  (`N selected · @key1, @key2`), so a host can place it at the top of a list.
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
- **Host-supplied assistant identity.** `ChatIcon` carries a catalog name, an
  asset path, or inline SVG bytes; `ChatIdentity` pairs a name with a mark.
  `ChatView::assistant` / `set_assistant` and `RagChat::set_assistant` brand the
  transcript rows, and `EmptyState::icon` accepts the same `ChatIcon`. Inline
  SVG needs no asset registration, so an application can compile its logo into
  the binary. The empty-state mark is rendered at 56 px.

### Changed

- **Breaking:** `EditorSurface::reveal(&self, offset, window, cx)` is required.
  A backend places the caret at the character offset and scrolls it into view;
  a host-owned scroll view should also follow the caret.
- **Breaking:** `rag_model_page(settings, changes, prompt, cx)` takes an
  optional `PromptActions` for the host buttons; `None` renders the prompt-file
  field alone.
- The system prompt is edited as a Markdown file instead of a single-line text
  field. An empty path uses the host's `built_in_prompt`.
- **Breaking:** `rag_model_page(settings, changes, cx)` no longer takes the
  controller and no longer renders the Profiles and Apply groups. Hosts place
  those on their own Overview page; `rag_settings_page` still renders the full
  standalone page.
- **Breaking:** `EmptyState::icon` is now a `ChatIcon`; wrap a catalog icon
  with `IconName::….into()`.
- The agent bar uses `Database` for the vector store and `Pilcrow` for the
  chunker; component buttons also have a tinted placeholder background. The
  blank icons were caused by the default `gpui_kit::assets::Assets` bundle
  omitting these files; the examples now register `AllAssets`, and
  `every_component_icon_draws_visible_pixels` rasterises every component icon
  as a regression test.

### Fixed

- Profile defaults and profiles without a prompt file restore the host's
  compiled-in prompt instead of Ragrig's generic stub.
- Path fields shrink to the controls column; the **Browse…** button no longer
  clips when the settings surface is narrow.
- Vertical rows (system prompt, folder corpora) now give their control the full
  row width.
- Decimal settings accept a complete draft such as `0.03` again. The old
  control committed every keystroke and rewrote the field, so the trailing dot
  became out-of-range and flipped to `1`. `a_decimal_commits_on_enter_not_per_keystroke`
  and `an_out_of_range_decimal_warns_and_restores` cover the widget.
- A monochrome Scico mark keeps transparent gaps between its rings; the
  previous concentric filled discs would have collapsed into a solid dot under
  GPUI's alpha-mask painting.
- Agent-bar component popovers keep their switches inside the content box. The
  label column may shrink (`min_w(0.)`), the switch does not (`flex_none()`),
  and the content no longer adds a second padding on top of the popover shell.
  `toggle_switches_stay_inside_the_popover` checks the rendered switch bounds.

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
