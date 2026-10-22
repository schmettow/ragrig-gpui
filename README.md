# ragrig-gpui

Reusable [GPUI](https://gpui.rs) components for Ragrig: a streaming RAG chat,
a Markdown transcript, source chips, attachment context, session picking, agent
status, a full settings page, and a dedicated Tokio controller. The crate has
no editor widget; it exports the editor roundtrip contract (`editor::EditorSurface`,
`editor::RoundtripHost`, and `editor::Roundtrip`) so Sciscraper and other hosts
can plug in their own editor while keeping the same context containers and
guarded write-back.

Status: **M2 (RAG chat), M4 (GROBID references), and M5 (roundtrip
experience) complete** — controller, RAG chat demo, sessions, library
ingestion, settings, reference rows with hover copy actions, `@key` citation
attachment, and multi-select bulk actions. The public API may still change
before 1.0. See [`PLAN.md`](PLAN.md) for milestones, decisions, and measured
results.

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

Run the demo from this crate's directory:

```sh
RAGRIG_WORKSPACE=/tmp/ragrig-demo \
RAGRIG_CORPUS=/path/to/papers \
RAGRIG_MODEL=llama3.2:3b \
cargo run --example ragchat
```

Add the GROBID feature and parse the folder's references too:

```sh
RAGRIG_WORKSPACE=/home/mart/Packages/ragrig-gpui/test_library \
RAGRIG_GROBID=1 \
cargo run --example ragchat --features grobid
```

This writes `.ragrig/references.bib` in the workspace and a GROBID manifest in
the corpus folder. Renaming is off by default; enable it under **Settings →
References** if you want the PDFs renamed to `Key - Authors - Title - Year`.

`RAGRIG_WORKSPACE` defaults to the current directory. `RAGRIG_CORPUS` indexes
a folder on startup; `RAGRIG_MODEL` and `RAGRIG_EMBED_MODEL` override the
settings defaults; `RAGRIG_ASK` submits a first question after the window
opens (use it with an already indexed workspace, because indexing and asking
are serialised); include an `@Key` in that question to exercise citation
attachment; `RAGRIG_GROBID=1` parses references on startup when the `grobid`
feature is enabled.

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
- **References** (Library, with `--features grobid`): press **GROBID** to parse
  the folder's PDFs. Rows show `@key`, authors, year, and title. **Copy @key**,
  **Copy path**, and open stay visible on every row at reduced opacity and
  become fully opaque on hover; a row without a PDF explains why the path
  button is disabled. Select any number of rows and one floating bar appears
  with **Copy @keys**, **Copy paths**, **Attach to next question**, and
  **Clear**. The last-run report shows parsed, cached, merged, and duplicate
  counts.
- Click the paperclip on a document row to attach it to the next question.
- **Chat** (center): ask a question. Write `@Kano2025` (or any key shown in the
  bibliography) and the paper is parsed and attached as one-shot context; a
  note above the answer lists the attached keys and any misses. Tokens stream,
  the footer reports chunks, sources, tokens, and elapsed time. **Stop** keeps
  the partial answer; retry a failed answer with the last prompt.
- **Sessions** (left dock): new, load, and refresh saved conversations. Apply
  in Settings rebuilds the agent and reloads the current saved session.
- **Settings** (right dock): workspace, provider, models, generation, prompt,
  embeddings, retrieval, PDF backend, chunking, memory, and (with `grobid`)
  GROBID extraction plus the **Use @key citations** switch. The system prompt
  is a Markdown file: an empty path uses the host's compiled-in prompt, and a
  host can add **Open prompt**, **Export**, and **Use built-in** actions with
  `PromptActions`. Workspace, bibliography, and prompt have **Browse…**
  pickers; **Maximum tokens** and **Context tokens** are sliders over powers of
  two. **Apply** validates and rebuilds.
  **Profiles** save/load JSON under `.ragrig/profiles`. Hosts that split the
  page can use `rag_model_page` with their own Overview/Apply row; the
  standalone `rag_settings_page` keeps Profiles and Apply.
- **Pending changes**: pass a `Changes` closure (usually
  `RagSettings::dirty_since(&applied)`) and the page marks changed fields in
  bold violet, adds `●` to their sections, and shows an “Unsaved changes” line.
  `SettingRow` is the reusable marked row; `path_control`, `token_control`,
  `text_control`, `number_control`, `switch_control`, and `dropdown_control`
  are the matching controls.
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
| `grobid` | no | GROBID/BibTeX extraction, reference rows, `@key` citations, multi-select bulk actions |

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
  grobid-bibtex pre-pass ── manifest, extract, rename, merge
```

- `AgentBar` renders one icon per pipeline component and reports
  `AgentBarEvent::{Configure, Toggle, OpenSettings}`. A component can carry
  `ComponentToggle` switches; the host owns the setting and reflects the new
  value on the next render. `RagController::agent_components()` uses this for
  **Chat memory** and **Hybrid retrieval** (off means `EmbedderSpec::None`, no
  embedding model, and no vector search).

- `ChatView` keeps its own branding neutral: `ChatIdentity` (name plus
  `ChatIcon`) signs assistant rows, and `ChatIcon` accepts a catalog name, an
  asset path, or inline SVG bytes. `EmptyState` takes the same mark. This lets
  a host such as Sciscraper show its own assistant name and logo without
  shipping assets in this crate.

- `RagController::new(RagSetup, cx)` starts one worker thread per controller.
  `ask` sends one question; `ask_with_retrieval` also carries a passage that
  the worker prepends to the query before embedding and searching. The host's
  transcript still shows only the question.
- `RagChat::new(window, cx, controller)` wires the M1 primitives and re-emits
  `RagChatEvent`s for status, source clicks, and copy.
- `RagSettings::to_setup()` validates and converts to `RagrigConfig` plus a
  prompt override, stream flag, and `@key` citation switch. A non-empty
  `system_prompt_path` is read (relative to the workspace) and overrides the
  inline prompt; `built_in_prompt` is restored by profile defaults and **Use
  built-in**. A non-empty `bibliography` field becomes
  `RagSetup::bibliography`; `None` keeps the conventional
  `<workspace>/.ragrig/references.bib`.
- `RagSetup::data_dir` chooses the device for the vector store, sessions, and
  profiles; `None` uses `<workspace>/.ragrig`, so the store opens at
  `<workspace>/.ragrig/.ragrig_store`. `RagSettings::from_layers(workspace)`
  merges `~/.ragrig/config.json` (or `RAGRIG_HOME/config.json`) and
  `<workspace>/.ragrig/config.json` per setting over the built-in defaults,
  workspace last. Profile listing covers both locations.
- The worker caches a `CiteState` for the configured bibliography and resolves
  citations on its blocking pool before generation. A folder parsed with
  GROBID joins the citation corpora even when it was not indexed.
- Host prompts are applied with `RagAgent::set_system_prompt`, which appends
  `{context}` when a prompt does not contain it. Without that placeholder,
  Ragrig has nowhere to substitute retrieved chunks or attached papers, so the
  model never sees either.
- `SciscraperAttach` formats context with the Scico prompt's
  `<sciscraper:v1:context source="creator">` and `source="corpus"`
  containers. Attached `@Key` papers therefore arrive as
  `<sciscraper:v1:document name="@Key">…</sciscraper:v1:document>`, not as
  generic `[Attached: …]` blocks. Host-built containers (the document buffer,
  the bibliography, a marked selection) pass through verbatim instead of being
  wrapped a second time, so their `cursor`, `range`, and `digest` attributes
  survive.
- `ChatView::set_message_text` replaces a finished answer in place, so a host
  can lift its own protocol blocks out of the transcript after the turn ends.
- The same strategy budgets the context block for `context_tokens`: explicit
  attachments receive 70% of the window and retrieval the rest, so a 36k-char
  paper cannot push itself out of a 4096-token model. Shortened papers carry a
  `<sciscraper:v1:truncated shown="…" total="…"/>` marker, which the Scico
  prompt already understands.
- `rag_settings(id, settings, controller, cx)` builds the complete page;
  Apply is explicit. `rag_model_page(settings, changes, prompt, cx)` exposes
  the Chat/Retrieval/Parsing/Memory groups; pass `Some(PromptActions)` for
  host-owned open/export buttons or `None` for the path field alone.
  `rag_references_group(settings, changes)` can be added to a host's Library
  page. `path_setting`,
  `token_slider`, `SettingRow`, `dirty_color`, and `dirty_header` are exported
  for hosts that need the same pickers, 2^x token controls, or unsaved marks.
- `RagSettings::dirty_since(&applied)` returns a `RagChanges` set of
  `RagField`s; `RagSection` groups them for page and section indicators.
  `RagSettings::reset_to_profile_defaults()` reloads the selected profile (or
  the built-in defaults while preserving the workspace and bibliography).

## Checks

```sh
cargo fmt --check
cargo check --all-targets --all-features
cargo test --all-features
cargo clippy --all-targets --all-features -- -D warnings
cargo doc --no-deps --all-features
```

Current suite: 125 library tests (including ten actor tests with a fake
generator and embedder, context-format and budgeting tests, citation-resolution
tests, field-level pending-change tests, retrieval enrichment, catalog and
inline/raster mark checks, agent-bar popover bounds, GPUI number-field typing,
blur validation and warning bounds, and GPUI hover, selection and
bulk-clipboard tests), 2 gallery tests, 5 active spike tests plus one ignored
typing benchmark, and 2 doctests. One GROBID end-to-end test is ignored by
default:

```sh
cargo test --all-features grobid_extracts_the_test_library_end_to_end \
  -- --ignored --nocapture
```

It copies the PDFs from `test_library/` to a temporary corpus, parses them
with the running GROBID server, renames them, merges the bibliography, checks
every row's PDF path, and resolves one key through the real PDF parser. It
needs `http://localhost:8070`.

On a machine without the xkbcommon development packages, linking fails even
though the Rust crates compile. Install the packages above; do not paper over
the link error in CI.

## Notes for integrators

- The examples register `gpui_kit::assets::AllAssets`. The default `Assets`
  bundle contains only 106 icons; using `Database`, `Pilcrow`, `Save`, or
  `Sparkles` without the full catalog leaves those buttons blank.

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
- GROBID extraction runs on the same worker as chat. It uses
  `grobid-bibtex`'s pre-pass, so unchanged PDFs are cached by a
  `.ragrig_grobid.json` manifest. References merge into
  `<workspace>/.ragrig/references.bib`; the demo resolves rows' PDFs against
  the selected corpus. Rename and OpenAlex completion are opt-in because both
  change files or call the network.
- `@key` resolution is capped at 3 papers per question and 100,000 characters
  per paper, mirroring Sciscraper. Citing the same file through two aliases
  counts both keys as resolved but attaches the document once. Misses never
  abort the question; they appear in the transcript note.
- `ReferenceSelection` is keyed by citation key, so a refreshed list keeps the
  selection. `ReferenceRow::on_select` adds the leading **+** and
  `ReferenceRow::on_edit` adds the pencil that opens the host's bibliography at
  that entry; `ReferenceSelectionBar` shows `N selected · @key1, @key2`,
  performs the bulk copy actions, and reports **Insert** / **Attach** / **Clear**
  to the host, which owns the editor, chat, and selection. It is full width, so
  it works as a top bar or a footer.
- The Library → bibliography roundtrip is backend-independent:
  `RoundtripHost::bibliography` names the `.bib`, `open_at` opens or focuses it
  at a character offset, and the default `edit_reference` locates the key on
  disk with `bibtex_entry_offset`. `EditorSurface::reveal` is the editor half,
  so a host only implements tab handling and caret placement.
- The checked-in `Cargo.lock` is intentional: `gpui-kit 0.7.3`'s optional
  `gpui-fast` branch pins `cocoa =0.26.0`, while a pre-existing lockfile may
  hold `0.26.1`. A fresh resolve succeeds; a stale one fails.

## License

MIT
