//! Bibliographic references extracted with GROBID.
//!
//! The heavy lifting lives in [`grobid_bibtex`]: its pre-pass fingerprints the
//! PDFs, extracts headers with GROBID, optionally completes them against
//! OpenAlex, optionally renames the PDFs, and merges fresh records into a
//! BibTeX file. This module adapts the result to the two UI needs of M4:
//!
//! - [`ReferenceEntry`] rows with authors, year, title, venue and a resolved
//!   PDF path;
//! - [`ReferenceRow`], a two-line row whose **Copy @key** and **Copy path**
//!   actions appear while the pointer is over it, plus a leading **+** that
//!   toggles multi-selection;
//! - [`ReferenceSelection`] and [`ReferenceSelectionBar`], the bulk actions for
//!   a set of selected rows;
//! - [`resolve_citations`], which turns the `@key` mentions of a question into
//!   one-shot context documents.
//!
//! Available with the `grobid` feature.

use biblatex::ChunksExt;
use std::{
    collections::{BTreeSet, HashSet},
    path::{Path, PathBuf},
    rc::Rc,
};

use gpui_kit::{
    App, ClipboardItem, ElementId, InteractiveElement as _, IntoElement, ParentElement as _,
    RenderOnce, SharedString, Styled as _, Window,
    assets::IconName,
    component::{
        ActiveTheme as _, Disableable as _, Selectable as _, Sizable as _,
        button::{Button, ButtonVariants as _},
        h_flex, v_flex,
    },
    div,
    prelude::FluentBuilder as _,
    px,
};
use grobid_bibtex::{
    Collection,
    cite::{CiteState, Located},
    files::collect_pdfs,
    prepass::{
        BibtexTarget, PrepassConfig, PrepassError, PrepassObserver, PrepassProgress, PrepassReport,
        PrepassStage, prepass_with,
    },
};
use ragrig::{AttachedDocument, DocumentParsers};

/// Server and pipeline options for one reference extraction run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReferenceOptions {
    /// Base URL of the GROBID server.
    pub grobid_url: String,
    /// Rename PDFs to `Key - Authors - Title - Year` after parsing.
    pub rename: bool,
    /// Complete extracted headers against OpenAlex.
    pub openalex: bool,
    /// Concurrent GROBID/OpenAlex requests.
    pub workers: usize,
}

impl Default for ReferenceOptions {
    fn default() -> Self {
        Self {
            grobid_url: "http://localhost:8070".to_owned(),
            rename: false,
            openalex: false,
            workers: 4,
        }
    }
}

impl ReferenceOptions {
    /// Options with the given server URL and the safe defaults (no rename, no
    /// OpenAlex completion).
    pub fn new(grobid_url: impl Into<String>) -> Self {
        Self {
            grobid_url: grobid_url.into(),
            ..Self::default()
        }
    }
}

/// One row of the reference list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReferenceEntry {
    /// Canonical citation key.
    pub key: String,
    /// Short author string, e.g. `Kano & Smith`.
    pub authors: String,
    /// Publication year, when known.
    pub year: String,
    /// Title, when known.
    pub title: String,
    /// Journal, book, series, or publisher, when known.
    pub source: String,
    /// Resolved PDF path, when one exists.
    pub path: Option<PathBuf>,
}

impl ReferenceEntry {
    /// The text the **Copy @key** action writes.
    pub fn copy_key_text(&self) -> String {
        format!("@{}", self.key)
    }

    /// One-line citation for tooltips, skipping unknown parts.
    pub fn citation_line(&self) -> String {
        let mut parts = Vec::new();
        if !self.authors.is_empty() {
            parts.push(self.authors.clone());
        }
        if !self.year.is_empty() {
            parts.push(format!("({})", self.year));
        }
        let mut line = parts.join(" ");
        for part in [&self.title, &self.source] {
            if !part.is_empty() {
                if !line.is_empty() {
                    line.push_str(". ");
                }
                line.push_str(part);
            }
        }
        line
    }

    /// Authors, year, and venue on one muted line.
    pub fn meta_line(&self) -> String {
        [&self.authors, &self.year, &self.source]
            .into_iter()
            .filter(|part| !part.is_empty())
            .cloned()
            .collect::<Vec<_>>()
            .join(" · ")
    }

    /// A title or a placeholder.
    pub fn display_title(&self) -> &str {
        if self.title.is_empty() {
            "Untitled"
        } else {
            &self.title
        }
    }
}

/// What a row's hover action copied.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReferenceCopy {
    /// The bare citation key. The clipboard already holds `@key`.
    Key(String),
    /// The resolved PDF path.
    Path(PathBuf),
}

/// Progress stage of a reference extraction.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ReferenceStage {
    /// PDFs are parsed by GROBID.
    #[default]
    Parsing,
    /// Records are completed against OpenAlex.
    Completing,
    /// PDFs are renamed.
    Renaming,
    /// Records are merged into the BibTeX file.
    Merging,
}

impl ReferenceStage {
    /// Short human-readable name.
    pub fn label(self) -> &'static str {
        match self {
            Self::Parsing => "parsing",
            Self::Completing => "completing",
            Self::Renaming => "renaming",
            Self::Merging => "merging",
        }
    }
}

/// A snapshot of a running extraction.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReferenceProgress {
    /// Current stage.
    pub stage: ReferenceStage,
    /// 1-based step within the stage.
    pub done: usize,
    /// Steps in the stage.
    pub total: usize,
    /// File the step is about.
    pub path: Option<PathBuf>,
}

impl ReferenceProgress {
    /// One line for a status bar.
    pub fn summary(&self) -> String {
        let file = self
            .path
            .as_ref()
            .and_then(|path| path.file_name())
            .and_then(|name| name.to_str());
        match file {
            Some(file) => format!(
                "GROBID · {} {}/{} · {file}",
                self.stage.label(),
                self.done.min(self.total),
                self.total
            ),
            None => format!(
                "GROBID · {} {}/{}",
                self.stage.label(),
                self.done.min(self.total),
                self.total
            ),
        }
    }
}

impl From<PrepassProgress> for ReferenceProgress {
    fn from(progress: PrepassProgress) -> Self {
        Self {
            stage: match progress.stage {
                PrepassStage::Parsing => ReferenceStage::Parsing,
                PrepassStage::Completing => ReferenceStage::Completing,
                PrepassStage::Renaming => ReferenceStage::Renaming,
                PrepassStage::Merging => ReferenceStage::Merging,
            },
            done: progress.done,
            total: progress.total,
            path: progress.path,
        }
    }
}

/// What one extraction run did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReferenceReport {
    /// Files parsed by GROBID in this run.
    pub parsed: usize,
    /// Files that came from the manifest cache.
    pub cached: usize,
    /// Files renamed.
    pub renamed: usize,
    /// Entries newly written to the bibliography.
    pub merged: usize,
    /// Records skipped as duplicates.
    pub duplicates: usize,
    /// Per-file failures.
    pub failed: Vec<String>,
}

impl ReferenceReport {
    /// One line for a status bar.
    pub fn summary(&self) -> String {
        let mut summary = format!(
            "{} parsed · {} cached · {} merged",
            self.parsed, self.cached, self.merged
        );
        if self.duplicates > 0 {
            summary.push_str(&format!(" · {} duplicates", self.duplicates));
        }
        if self.renamed > 0 {
            summary.push_str(&format!(" · {} renamed", self.renamed));
        }
        if !self.failed.is_empty() {
            summary.push_str(&format!(" · {} failed", self.failed.len()));
        }
        summary
    }

    fn from_prepass(report: PrepassReport) -> Self {
        Self {
            parsed: report.parsed,
            cached: report.cached,
            renamed: report.renamed,
            merged: report.merged,
            duplicates: report.duplicates,
            failed: report
                .failed
                .into_iter()
                .map(|failure| format!("{}: {}", failure.path.display(), failure.reason))
                .collect(),
        }
    }
}

/// Maximum number of `@key` papers attached to one query.
pub const MAX_CITATIONS: usize = 3;
/// Maximum characters attached per `@key` paper.
pub const MAX_CITATION_CHARS: usize = 100_000;

/// One `@key` paper that was attached to a query.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CitationAttachment {
    /// Canonical citation key.
    pub key: String,
    /// PDF or document that supplied the context.
    pub path: PathBuf,
}

/// Result of resolving the `@key` mentions of one query.
#[derive(Default)]
pub(crate) struct CitationResolution {
    /// Documents ready for Ragrig's attach strategy.
    pub attachments: Vec<AttachedDocument>,
    /// Canonical keys that were attached (aliases count).
    pub resolved: Vec<String>,
    /// One attachment per distinct file, for source chips.
    pub attached: Vec<CitationAttachment>,
    /// One readable line per key that could not be attached.
    pub missed: Vec<String>,
}

/// Resolve `@key` mentions against the bibliography and the active corpora.
///
/// Mirrors the host-side resolution in Sciscraper: the recorded `file` field
/// first, then a corpus file named after the key; up to [`MAX_CITATIONS`]
/// papers, each truncated to [`MAX_CITATION_CHARS`] characters. Misses never
/// abort the query; they are returned for the transcript note.
pub(crate) fn resolve_citations(
    state: &mut CiteState,
    bibliography: &Path,
    corpora: &[PathBuf],
    keys: &[String],
    parsers: &DocumentParsers,
) -> CitationResolution {
    let mut resolution = CitationResolution::default();
    if keys.is_empty() {
        return resolution;
    }
    if !bibliography.is_file() {
        for key in keys {
            resolution.missed.push(format!(
                "@{key} — no bibliography at {}",
                bibliography.display()
            ));
        }
        return resolution;
    }

    let corpus_paths: Vec<&Path> = corpora.iter().map(PathBuf::as_path).collect();
    let located = state.locate(keys, Some(bibliography), &corpus_paths);
    let mut attached_paths: HashSet<PathBuf> = HashSet::new();
    for item in located {
        match item {
            Located::Found { key, path, .. } => {
                if !attached_paths.insert(path.clone()) {
                    // Aliases of an already attached document count as
                    // resolved, but do not produce a second context block.
                    resolution.resolved.push(key);
                    continue;
                }
                if resolution.attachments.len() >= MAX_CITATIONS {
                    resolution.missed.push(format!(
                        "@{key} — attachment limit ({MAX_CITATIONS}) reached"
                    ));
                    continue;
                }
                match parsers.parse(&path) {
                    Ok(text) => {
                        resolution.attachments.push(AttachedDocument {
                            name: format!("@{key}"),
                            content: truncate_text(text, MAX_CITATION_CHARS),
                        });
                        resolution.resolved.push(key.clone());
                        resolution.attached.push(CitationAttachment { key, path });
                    }
                    Err(error) => resolution.missed.push(format!(
                        "@{key} — cannot read {}: {error:#}",
                        path.display()
                    )),
                }
            }
            other => resolution.missed.push(other.describe()),
        }
    }
    resolution
}

/// Build the `@key @key` text for a set of selected rows.
pub fn keys_text(entries: &[ReferenceEntry]) -> String {
    entries
        .iter()
        .map(|entry| entry.copy_key_text())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Build the newline-separated absolute paths of a set of rows.
pub fn paths_text(entries: &[ReferenceEntry]) -> String {
    entries
        .iter()
        .filter_map(|entry| entry.path.as_ref())
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Keys selected in a reference list, keyed by citation key.
///
/// Selection survives list refreshes: [`retain_existing`](Self::retain_existing)
/// drops keys that disappeared, and rows are always returned in list order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReferenceSelection {
    keys: BTreeSet<String>,
}

impl ReferenceSelection {
    /// Number of selected keys.
    pub fn len(&self) -> usize {
        self.keys.len()
    }

    /// Whether nothing is selected.
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    /// Whether `key` is selected.
    pub fn contains(&self, key: &str) -> bool {
        self.keys.contains(key)
    }

    /// Select `key`; returns whether it was newly added.
    pub fn insert(&mut self, key: impl Into<String>) -> bool {
        self.keys.insert(key.into())
    }

    /// Deselect `key`; returns whether it was selected.
    pub fn remove(&mut self, key: &str) -> bool {
        self.keys.remove(key)
    }

    /// Toggle `key`; returns the new selection state.
    pub fn toggle(&mut self, key: impl Into<String>) -> bool {
        let key = key.into();
        if self.keys.remove(&key) {
            false
        } else {
            self.keys.insert(key);
            true
        }
    }

    /// Clear every selection.
    pub fn clear(&mut self) {
        self.keys.clear();
    }

    /// Drop selected keys that no longer exist in `entries`.
    pub fn retain_existing(&mut self, entries: &[ReferenceEntry]) {
        let available: HashSet<&str> = entries.iter().map(|entry| entry.key.as_str()).collect();
        self.keys.retain(|key| available.contains(key.as_str()));
    }

    /// The selected entries, in the order they appear in `entries`.
    pub fn entries<'a>(&self, entries: &'a [ReferenceEntry]) -> Vec<&'a ReferenceEntry> {
        entries
            .iter()
            .filter(|entry| self.keys.contains(&entry.key))
            .collect()
    }
}

fn truncate_text(text: String, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        text
    } else {
        text.chars().take(max_chars).collect()
    }
}

pub(crate) enum ExtractionError {
    Cancelled,
    Failed(String),
}

#[derive(Default)]
pub(crate) struct ExtractionObserver<'a> {
    pub progress: Option<&'a mut (dyn FnMut(ReferenceProgress) + Send)>,
    pub cancel: Option<&'a (dyn Fn() -> bool + Send + Sync)>,
}

impl PrepassObserver for ExtractionObserver<'_> {
    fn progress(&mut self, progress: PrepassProgress) {
        if let Some(callback) = self.progress.as_mut() {
            callback(progress.into());
        }
    }

    fn cancelled(&self) -> bool {
        self.cancel.is_some_and(|cancel| cancel())
    }
}

/// Run the GROBID pre-pass and return the merged bibliography as UI rows.
pub(crate) async fn extract(
    folder: &Path,
    bibliography: &Path,
    options: &ReferenceOptions,
    observer: &mut ExtractionObserver<'_>,
) -> Result<(Vec<ReferenceEntry>, ReferenceReport), ExtractionError> {
    if let Some(parent) = bibliography.parent() {
        std::fs::create_dir_all(parent).map_err(|error| {
            ExtractionError::Failed(format!("cannot create '{}': {error}", parent.display()))
        })?;
    }

    let mut config = PrepassConfig::new(folder);
    config.bibtex = Some(BibtexTarget {
        path: bibliography.to_path_buf(),
        link: true,
    });
    config.grobid_url.clone_from(&options.grobid_url);
    config.openalex = options.openalex;
    config.rename = options.rename;
    config.workers = options.workers.max(1);

    let report = match prepass_with(&config, observer).await {
        Ok(report) => report,
        Err(PrepassError::Cancelled) => return Err(ExtractionError::Cancelled),
        Err(error) => return Err(ExtractionError::Failed(format!("{error:#}"))),
    };
    let entries = load_entries(bibliography, folder).map_err(ExtractionError::Failed)?;
    Ok((entries, ReferenceReport::from_prepass(report)))
}

/// Load reference rows from a merged bibliography.
pub fn load_entries(bibliography: &Path, folder: &Path) -> Result<Vec<ReferenceEntry>, String> {
    let collection = Collection::load_or_new(bibliography)
        .map_err(|error| format!("cannot read '{}': {error}", bibliography.display()))?;
    let pdfs = collect_pdfs(folder).unwrap_or_default();
    let mut entries: Vec<ReferenceEntry> = collection
        .iter()
        .map(|entry| {
            let field = |names: &[&str]| -> String {
                for name in names {
                    if let Some(chunks) = entry.get(name) {
                        let text = normalize_text(&chunks.format_verbatim());
                        if !text.is_empty() {
                            return text;
                        }
                    }
                }
                String::new()
            };
            let key = entry.key.clone();
            let raw_file = field(&["file", "pdf"]);
            let file = (!raw_file.is_empty()).then(|| PathBuf::from(&raw_file));
            let year = {
                let year = field(&["year"]);
                if year.is_empty() {
                    first_year(&field(&["date"]))
                } else {
                    year
                }
            };
            ReferenceEntry {
                key: key.clone(),
                authors: short_authors(&field(&["author", "editor"])),
                year,
                title: field(&["title"]),
                source: field(&[
                    "journaltitle",
                    "journal",
                    "booktitle",
                    "series",
                    "publisher",
                    "institution",
                ]),
                path: resolve_document(&key, file.as_deref(), folder, &pdfs),
            }
        })
        .collect();

    entries.sort_by_cached_key(|entry| {
        (
            entry.authors.is_empty(),
            entry.authors.to_lowercase(),
            entry.year.clone(),
            entry.title.to_lowercase(),
        )
    });
    Ok(entries)
}

fn resolve_document(
    key: &str,
    file: Option<&Path>,
    folder: &Path,
    pdfs: &[PathBuf],
) -> Option<PathBuf> {
    if let Some(file) = file {
        let candidate = if file.is_absolute() {
            file.to_path_buf()
        } else {
            folder.join(file)
        };
        if candidate.is_file() {
            return Some(candidate);
        }
        // The bibliography may have been written from a different working
        // directory; try the recorded path as-is before falling back.
        if file.is_file() {
            return Some(file.to_path_buf());
        }
    }
    let key = key.to_ascii_lowercase();
    let prefix = format!("{key} - ");
    pdfs.iter()
        .find(|path| {
            path.file_stem()
                .and_then(|stem| stem.to_str())
                .map(|stem| {
                    let stem = stem.to_ascii_lowercase();
                    stem == key || stem.starts_with(&prefix)
                })
                .unwrap_or(false)
        })
        .cloned()
}

fn short_authors(raw: &str) -> String {
    let names: Vec<String> = raw
        .split(" and ")
        .map(|part| normalize_text(part.split(',').next().unwrap_or(part)))
        .filter(|name| !name.is_empty())
        .collect();
    match names.as_slice() {
        [] => String::new(),
        [one] => one.clone(),
        [first, second] => format!("{first} & {second}"),
        [first, ..] => format!("{first} et al."),
    }
}

fn first_year(date: &str) -> String {
    let digits: String = date
        .chars()
        .skip_while(|character| !character.is_ascii_digit())
        .take(4)
        .collect();
    if digits.len() == 4 {
        digits
    } else {
        String::new()
    }
}

fn normalize_text(text: &str) -> String {
    let mut without_braces = String::with_capacity(text.len());
    let mut depth = 0usize;
    for character in text.chars() {
        match character {
            '{' => depth += 1,
            '}' => depth = depth.saturating_sub(1),
            _ if depth == 0 => without_braces.push(character),
            _ => {}
        }
    }
    without_braces
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

type CopyHandler = Rc<dyn Fn(ReferenceCopy, &mut Window, &mut App)>;
type SelectHandler = Rc<dyn Fn(&mut Window, &mut App)>;

/// A two-line reference row with hover-revealed copy and selection actions.
///
/// While the pointer is over the row, a **+** appears at its leading edge and
/// **Copy @key** / **Copy path** at its trailing edge. Every control keeps its
/// slot in every frame, so the text never jumps when the actions become
/// visible. A row without a resolved PDF keeps the path button disabled and
/// explains why in its tooltip.
#[derive(IntoElement)]
pub struct ReferenceRow {
    index: usize,
    entry: ReferenceEntry,
    on_copy: Option<CopyHandler>,
    on_select: Option<SelectHandler>,
    selectable: bool,
    selected: bool,
}

impl ReferenceRow {
    /// A row for the entry at `index` in the current list.
    pub fn new(index: usize, entry: ReferenceEntry) -> Self {
        Self {
            index,
            entry,
            on_copy: None,
            on_select: None,
            selectable: false,
            selected: false,
        }
    }

    /// Receive the copied value, e.g. to update a status line.
    ///
    /// The row itself writes the clipboard text: `@key` for the key action and
    /// the absolute PDF path for the path action.
    pub fn on_copy(
        mut self,
        handler: impl Fn(ReferenceCopy, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_copy = Some(Rc::new(handler));
        self
    }

    /// Show the leading **+** toggle and report clicks.
    ///
    /// Implies [`selectable`](Self::selectable).
    pub fn on_select(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_select = Some(Rc::new(handler));
        self.selectable = true;
        self
    }

    /// Reserve the leading selection slot without a click handler.
    ///
    /// Use [`on_select`](Self::on_select) for an interactive toggle.
    pub fn selectable(mut self, selectable: bool) -> Self {
        self.selectable = selectable;
        self
    }

    /// Paint the row as selected and keep the leading toggle visible.
    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }
}

impl RenderOnce for ReferenceRow {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let group: SharedString = format!("reference-row-{}", self.index).into();
        let key = self.entry.key.clone();
        let key_label = self.entry.copy_key_text();
        let meta = self.entry.meta_line();
        let path = self.entry.path.clone();
        let handler = self.on_copy;
        let selected = self.selected;
        let hover_bg = if selected {
            theme.primary.opacity(0.16)
        } else {
            theme.muted.opacity(0.28)
        };

        let toggle = (self.selectable || selected).then(|| {
            let on_select = self.on_select;
            let button = Button::new(ElementId::Name(
                format!("reference-{}-select", self.index).into(),
            ))
            .icon(if selected {
                IconName::Check
            } else {
                IconName::Plus
            })
            .small()
            .ghost()
            .selected(selected)
            .toggled(selected)
            .tooltip(if selected {
                "Deselect reference"
            } else {
                "Select reference"
            })
            .when_some(on_select, |button, on_select| {
                button.on_click(move |_, window, cx| on_select(window, cx))
            });
            div()
                .debug_selector(|| format!("reference-select-{}", self.index))
                .flex_none()
                .w(px(24.))
                .h(px(24.))
                .flex()
                .items_center()
                .justify_center()
                .when(!selected, |slot| {
                    slot.invisible()
                        .group_hover(group.clone(), |slot| slot.visible())
                })
                .child(button)
        });

        let copy_key = {
            let key = key.clone();
            let handler = handler.clone();
            Button::new(ElementId::Name(
                format!("reference-{}-copy-key", self.index).into(),
            ))
            .icon(IconName::AtSign)
            .xsmall()
            .ghost()
            .tooltip("Copy @key")
            .on_click(move |_, window, cx| {
                cx.write_to_clipboard(ClipboardItem::new_string(format!("@{key}")));
                if let Some(handler) = &handler {
                    handler(ReferenceCopy::Key(key.clone()), window, cx);
                }
            })
        };

        let copy_path = match path {
            Some(path) => {
                let handler = handler.clone();
                Button::new(ElementId::Name(
                    format!("reference-{}-copy-path", self.index).into(),
                ))
                .icon(IconName::FileText)
                .xsmall()
                .ghost()
                .tooltip("Copy path")
                .on_click(move |_, window, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(path.display().to_string()));
                    if let Some(handler) = &handler {
                        handler(ReferenceCopy::Path(path.clone()), window, cx);
                    }
                })
            }
            None => Button::new(ElementId::Name(
                format!("reference-{}-copy-path", self.index).into(),
            ))
            .icon(IconName::FileText)
            .xsmall()
            .ghost()
            .disabled(true)
            .tooltip("No PDF path"),
        };

        h_flex()
            .id(ElementId::Name(
                format!("reference-row-{}", self.index).into(),
            ))
            .group(group.clone())
            .items_center()
            .gap_2()
            .p_2()
            .rounded(theme.radius)
            .border_1()
            .border_color(if selected {
                theme.primary.opacity(0.55)
            } else {
                theme.border
            })
            .when(selected, |this| this.bg(theme.primary.opacity(0.10)))
            .hover(move |this| this.bg(hover_bg))
            .when_some(toggle, |this, toggle| this.child(toggle))
            .child(
                div()
                    .w(px(3.))
                    .h(px(28.))
                    .flex_none()
                    .rounded_full()
                    .bg(theme.primary.opacity(if selected { 1.0 } else { 0.65 })),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_1()
                    .child(
                        h_flex()
                            .min_w_0()
                            .gap_2()
                            .child(
                                div()
                                    .flex_none()
                                    .text_xs()
                                    .text_color(theme.primary)
                                    .child(key_label),
                            )
                            .when(!meta.is_empty(), |this| {
                                this.child(
                                    div()
                                        .min_w_0()
                                        .truncate()
                                        .text_xs()
                                        .text_color(theme.muted_foreground)
                                        .child(meta),
                                )
                            }),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_sm()
                            .child(self.entry.display_title().to_owned()),
                    ),
            )
            .child(
                h_flex()
                    .flex_none()
                    .gap_1()
                    .invisible()
                    .group_hover(group, |this| this.visible())
                    .child(
                        div()
                            .debug_selector(|| "reference-copy-key".to_owned())
                            .child(copy_key),
                    )
                    .child(
                        div()
                            .debug_selector(|| "reference-copy-path".to_owned())
                            .child(copy_path),
                    ),
            )
    }
}

/// A bulk action offered by [`ReferenceSelectionBar`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReferenceAction {
    /// Write the selected `@key` mentions to the clipboard.
    CopyKeys,
    /// Write the selected PDF paths to the clipboard.
    CopyPaths,
    /// Attach the selected papers to the next question. The host does this.
    Attach,
    /// Drop the selection. The host does this.
    Clear,
}

type ActionHandler = Rc<dyn Fn(ReferenceAction, &[ReferenceEntry], &mut Window, &mut App)>;

/// One floating bar for a multi-selection of references.
///
/// The bar writes **Copy @keys** and **Copy paths** itself. **Attach** and
/// **Clear** are shown only when a host action handler is installed; the host
/// owns the selection and the chat, so it performs those two actions. Build the
/// bar from the selected entries in list order and give it a stable parent.
#[derive(IntoElement)]
pub struct ReferenceSelectionBar {
    entries: Vec<ReferenceEntry>,
    on_action: Option<ActionHandler>,
}

impl ReferenceSelectionBar {
    /// A bar for `entries`, which should be the selected rows in list order.
    pub fn new(entries: impl IntoIterator<Item = ReferenceEntry>) -> Self {
        Self {
            entries: entries.into_iter().collect(),
            on_action: None,
        }
    }

    /// Receive every action after its built-in effect.
    ///
    /// Copy actions have already updated the clipboard when the handler runs.
    pub fn on_action(
        mut self,
        handler: impl Fn(ReferenceAction, &[ReferenceEntry], &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_action = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for ReferenceSelectionBar {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let count = self.entries.len();
        let path_text = paths_text(&self.entries);
        let paths_available = !path_text.is_empty();
        let handler = self.on_action;

        let copy_keys = {
            let handler = handler.clone();
            let entries = self.entries.clone();
            Button::new("reference-bulk-copy-keys")
                .icon(IconName::AtSign)
                .xsmall()
                .ghost()
                .tooltip("Copy @keys")
                .on_click(move |_, window, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(keys_text(&entries)));
                    if let Some(handler) = &handler {
                        handler(ReferenceAction::CopyKeys, &entries, window, cx);
                    }
                })
        };
        let copy_paths = {
            let captured = path_text;
            let handler = handler.clone();
            let entries = self.entries.clone();
            Button::new("reference-bulk-copy-paths")
                .icon(IconName::FileText)
                .xsmall()
                .ghost()
                .disabled(!paths_available)
                .tooltip("Copy paths")
                .on_click(move |_, window, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(captured.clone()));
                    if let Some(handler) = &handler {
                        handler(ReferenceAction::CopyPaths, &entries, window, cx);
                    }
                })
        };

        let mut bar = h_flex()
            .debug_selector(|| "reference-selection-bar".to_owned())
            .items_center()
            .gap_1()
            .px_2()
            .py_1()
            .rounded_full()
            .border_1()
            .border_color(theme.primary.opacity(0.55))
            .bg(theme.background)
            .shadow_md()
            .child(
                div()
                    .flex_none()
                    .px_1()
                    .text_xs()
                    .text_color(theme.primary)
                    .child(format!("{count} selected")),
            )
            .child(
                div()
                    .debug_selector(|| "reference-bulk-copy-keys-button".to_owned())
                    .child(copy_keys),
            )
            .child(
                div()
                    .debug_selector(|| "reference-bulk-copy-paths-button".to_owned())
                    .child(copy_paths),
            );

        if handler.is_some() {
            let attach_handler = handler.clone();
            let attach_entries = self.entries.clone();
            bar = bar.child(
                Button::new("reference-bulk-attach")
                    .icon(IconName::Paperclip)
                    .xsmall()
                    .ghost()
                    .disabled(!paths_available)
                    .tooltip("Attach to next question")
                    .on_click(move |_, window, cx| {
                        if let Some(handler) = &attach_handler {
                            handler(ReferenceAction::Attach, &attach_entries, window, cx);
                        }
                    }),
            );
            let clear_handler = handler.clone();
            let clear_entries = self.entries.clone();
            bar = bar.child(
                Button::new("reference-bulk-clear")
                    .icon(IconName::X)
                    .xsmall()
                    .ghost()
                    .tooltip("Clear selection")
                    .on_click(move |_, window, cx| {
                        if let Some(handler) = &clear_handler {
                            handler(ReferenceAction::Clear, &clear_entries, window, cx);
                        }
                    }),
            );
        }

        bar
    }
}

impl crate::rag::RagSettings {
    /// The reference options described by these settings.
    pub fn reference_options(&self) -> ReferenceOptions {
        ReferenceOptions {
            grobid_url: self.grobid_url.clone(),
            rename: self.rename_pdfs,
            openalex: self.openalex,
            workers: 4,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, rc::Rc};

    use gpui_kit::{Context, Modifiers, Render, TestAppContext, point};

    use super::*;

    fn temp_dir(label: &str) -> PathBuf {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let counter = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "ragrig-gpui-references-{label}-{}-{counter}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    const SAMPLE: &str = r#"
@article{Kano2025,
  author = {Kano, Fumihiro and Smith, John},
  year = {2025},
  title = {Holding a steady look at the human eye},
  journaltitle = {Journal of Vision},
  file = {PDF_PATH}
}
"#;

    #[test]
    fn entries_carry_metadata_and_resolve_the_linked_pdf() {
        let dir = temp_dir("read");
        let pdf = dir.join("paper.pdf");
        std::fs::write(&pdf, b"%PDF-1.4").expect("pdf");
        let bib = dir.join("references.bib");
        std::fs::write(&bib, SAMPLE.replace("PDF_PATH", &pdf.display().to_string())).expect("bib");

        let entries = load_entries(&bib, &dir).expect("entries");
        assert_eq!(entries.len(), 1);
        let entry = &entries[0];
        assert_eq!(entry.key, "Kano2025");
        assert_eq!(entry.authors, "Kano & Smith");
        assert_eq!(entry.year, "2025");
        assert_eq!(entry.title, "Holding a steady look at the human eye");
        assert_eq!(entry.source, "Journal of Vision");
        assert_eq!(entry.path.as_deref(), Some(pdf.as_path()));
        assert_eq!(entry.copy_key_text(), "@Kano2025");

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_keyed_file_name_resolves_without_a_file_field() {
        let dir = temp_dir("keyed");
        let pdf = dir.join("Kano2025 - Holding a steady look.pdf");
        std::fs::write(&pdf, b"%PDF-1.4").expect("pdf");
        let bib = dir.join("references.bib");
        std::fs::write(
            &bib,
            r#"@article{Kano2025,
  author = {Kano, Fumihiro},
  date = {2024-03},
  title = {Gaze},
}"#,
        )
        .expect("bib");

        let entries = load_entries(&bib, &dir).expect("entries");
        assert_eq!(entries[0].path.as_deref(), Some(pdf.as_path()));
        assert_eq!(entries[0].year, "2024");

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_missing_bibliography_is_empty() {
        let dir = temp_dir("missing");
        let entries = load_entries(&dir.join("missing.bib"), &dir).expect("entries");
        assert!(entries.is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    fn cite_parsers() -> DocumentParsers {
        DocumentParsers::new(ragrig::build_parsers())
    }

    fn entry(key: &str, path: Option<PathBuf>) -> ReferenceEntry {
        ReferenceEntry {
            key: key.to_owned(),
            authors: "Kano".to_owned(),
            year: "2025".to_owned(),
            title: format!("Title of {key}"),
            source: String::new(),
            path,
        }
    }

    #[test]
    fn citations_resolve_recorded_files_and_explain_misses() {
        let dir = temp_dir("cite");
        let corpus = dir.join("corpus");
        std::fs::create_dir_all(&corpus).expect("corpus");
        let pdf = corpus.join("paper.md");
        std::fs::write(&pdf, "RECORDED PAPER BODY").expect("paper");
        let bib = dir.join("references.bib");
        std::fs::write(
            &bib,
            format!(
                "@article{{Kano2025,\n  author = {{Kano, Fumihiro}},\n  year = {{2025}},\n  title = {{Gaze}},\n  file = {{{}}}\n}}\n",
                pdf.display()
            ),
        )
        .expect("bib");

        let keys = vec!["kano2025".to_owned(), "Missing2020".to_owned()];
        let mut state = CiteState::default();
        let resolution = resolve_citations(
            &mut state,
            &bib,
            std::slice::from_ref(&corpus),
            &keys,
            &cite_parsers(),
        );

        assert_eq!(resolution.resolved, ["Kano2025"]);
        assert_eq!(resolution.attachments.len(), 1);
        assert_eq!(resolution.attachments[0].name, "@Kano2025");
        assert_eq!(resolution.attachments[0].content, "RECORDED PAPER BODY");
        assert_eq!(resolution.attached[0].path, pdf);
        assert_eq!(resolution.missed.len(), 1);
        assert!(resolution.missed[0].contains("Missing2020"));

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn citation_aliases_share_one_attachment() {
        let dir = temp_dir("cite-alias");
        let corpus = dir.join("corpus");
        std::fs::create_dir_all(&corpus).expect("corpus");
        let paper = corpus.join("paper.md");
        std::fs::write(&paper, "SHARED BODY").expect("paper");
        let bib = dir.join("references.bib");
        std::fs::write(
            &bib,
            format!(
                "@article{{KeyA, title = {{A}}, file = {{{path}}}}}\n\
                 @article{{KeyB, title = {{B}}, file = {{{path}}}}}\n",
                path = paper.display()
            ),
        )
        .expect("bib");

        let keys = vec!["KeyA".to_owned(), "KeyB".to_owned()];
        let mut state = CiteState::default();
        let resolution = resolve_citations(
            &mut state,
            &bib,
            std::slice::from_ref(&corpus),
            &keys,
            &cite_parsers(),
        );

        assert_eq!(resolution.resolved, ["KeyA", "KeyB"]);
        assert_eq!(resolution.attachments.len(), 1);
        assert_eq!(resolution.attached.len(), 1);
        assert!(resolution.missed.is_empty());

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn citations_cap_at_three_and_truncate_one_document() {
        let dir = temp_dir("cite-limit");
        let corpus = dir.join("corpus");
        std::fs::create_dir_all(&corpus).expect("corpus");
        let mut bib = String::new();
        for index in 1..=4 {
            let path = corpus.join(format!("paper{index}.md"));
            let body = if index == 1 {
                "x".repeat(MAX_CITATION_CHARS + 5)
            } else {
                format!("body {index}")
            };
            std::fs::write(&path, body).expect("paper");
            bib.push_str(&format!(
                "@article{{Key{index}, title = {{T{index}}}, file = {{{}}}}}\n",
                path.display()
            ));
        }
        let bib_path = dir.join("references.bib");
        std::fs::write(&bib_path, bib).expect("bib");

        let keys = (1..=4)
            .map(|index| format!("Key{index}"))
            .collect::<Vec<_>>();
        let mut state = CiteState::default();
        let resolution = resolve_citations(
            &mut state,
            &bib_path,
            std::slice::from_ref(&corpus),
            &keys,
            &cite_parsers(),
        );

        assert_eq!(resolution.attachments.len(), MAX_CITATIONS);
        assert_eq!(
            resolution.attachments[0].content.chars().count(),
            MAX_CITATION_CHARS
        );
        assert_eq!(resolution.resolved.len(), MAX_CITATIONS);
        assert_eq!(resolution.missed.len(), 1);
        assert!(resolution.missed[0].contains("limit"));

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_keyed_corpus_file_is_citable_without_a_file_field() {
        let dir = temp_dir("cite-keyed");
        let corpus = dir.join("corpus");
        std::fs::create_dir_all(&corpus).expect("corpus");
        let paper = corpus.join("Kano2025 - Gaze.md");
        std::fs::write(&paper, "KEYED BODY").expect("paper");
        let bib = dir.join("references.bib");
        std::fs::write(&bib, "@article{Kano2025, title = {Gaze}}\n").expect("bib");

        let keys = vec!["kano2025".to_owned()];
        let mut state = CiteState::default();
        let resolution = resolve_citations(
            &mut state,
            &bib,
            std::slice::from_ref(&corpus),
            &keys,
            &cite_parsers(),
        );

        assert_eq!(resolution.resolved, ["Kano2025"]);
        assert_eq!(resolution.attached[0].path, paper);
        assert_eq!(resolution.attachments[0].content, "KEYED BODY");

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_missing_bibliography_reports_every_key() {
        let dir = temp_dir("cite-missing-bib");
        let keys = vec!["One".to_owned(), "Two".to_owned()];
        let mut state = CiteState::default();
        let resolution = resolve_citations(
            &mut state,
            &dir.join("missing.bib"),
            std::slice::from_ref(&dir),
            &keys,
            &cite_parsers(),
        );

        assert!(resolution.attachments.is_empty());
        assert_eq!(resolution.missed.len(), 2);
        assert!(
            resolution
                .missed
                .iter()
                .all(|line| line.contains("bibliography"))
        );

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn selection_is_keyed_and_follows_the_list_order() {
        let entries = vec![entry("B", None), entry("A", None)];
        let mut selection = ReferenceSelection::default();
        assert!(selection.insert("A"));
        assert!(selection.insert("B"));
        assert_eq!(selection.len(), 2);
        assert_eq!(
            selection
                .entries(&entries)
                .iter()
                .map(|entry| entry.key.as_str())
                .collect::<Vec<_>>(),
            ["B", "A"]
        );

        assert!(!selection.toggle("A"));
        assert_eq!(selection.len(), 1);
        assert!(selection.toggle("A"));

        selection.retain_existing(&entries[..1]);
        assert!(selection.contains("B"));
        assert!(!selection.contains("A"));
        selection.clear();
        assert!(selection.is_empty());
    }

    #[test]
    fn bulk_text_uses_citation_keys_and_skips_pathless_rows() {
        let entries = vec![
            entry("B", Some(PathBuf::from("/tmp/b.pdf"))),
            entry("A", None),
        ];
        assert_eq!(keys_text(&entries), "@B @A");
        assert_eq!(paths_text(&entries), "/tmp/b.pdf");
    }

    #[test]
    fn authors_and_citations_skip_unknown_parts() {
        assert_eq!(short_authors("Kano, Fumihiro"), "Kano");
        assert_eq!(
            short_authors("Kano, Fumihiro and Smith, John"),
            "Kano & Smith"
        );
        assert_eq!(
            short_authors("Kano, F. and Smith, J. and Doe, A."),
            "Kano et al."
        );
        assert_eq!(short_authors(""), "");

        let entry = ReferenceEntry {
            key: "Key".to_owned(),
            authors: "Kano".to_owned(),
            year: String::new(),
            title: "Title".to_owned(),
            source: String::new(),
            path: None,
        };
        assert_eq!(entry.citation_line(), "Kano. Title");
        assert_eq!(entry.meta_line(), "Kano");
        assert_eq!(
            ReferenceEntry {
                title: String::new(),
                ..entry
            }
            .display_title(),
            "Untitled"
        );
    }

    #[test]
    fn the_report_summarises_counts_and_failures() {
        let report = ReferenceReport {
            parsed: 2,
            cached: 1,
            renamed: 1,
            merged: 2,
            duplicates: 3,
            failed: vec!["a.pdf: unreadable".to_owned()],
        };
        assert_eq!(
            report.summary(),
            "2 parsed · 1 cached · 2 merged · 3 duplicates · 1 renamed · 1 failed"
        );
    }

    struct RowHost {
        entry: ReferenceEntry,
        copies: Rc<RefCell<Vec<ReferenceCopy>>>,
    }

    impl Render for RowHost {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let copies = Rc::clone(&self.copies);
            div().w(px(420.)).h(px(80.)).child(
                ReferenceRow::new(0, self.entry.clone())
                    .on_copy(move |copy, _, _| copies.borrow_mut().push(copy)),
            )
        }
    }

    #[gpui_kit::test]
    fn hovering_reveals_copy_key_and_copy_path(cx: &mut TestAppContext) {
        cx.update(crate::init);
        let dir = temp_dir("hover");
        let pdf = dir.join("paper.pdf");
        std::fs::write(&pdf, b"%PDF-1.4").expect("pdf");
        let entry = ReferenceEntry {
            key: "Kano2025".to_owned(),
            authors: "Kano".to_owned(),
            year: "2025".to_owned(),
            title: "Gaze".to_owned(),
            source: String::new(),
            path: Some(pdf.clone()),
        };
        let copies = Rc::new(RefCell::new(Vec::new()));
        let copies_for_host = Rc::clone(&copies);
        let (_, cx) = cx.add_window_view(move |_, _| RowHost {
            entry: entry.clone(),
            copies: Rc::clone(&copies_for_host),
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));

        // The actions are hidden until the pointer enters the row.
        assert!(cx.debug_bounds("reference-copy-key").is_some());
        cx.simulate_mouse_move(point(px(200.), px(30.)), None, Modifiers::default());
        cx.update(|window, cx| window.draw(cx).clear(cx));

        let key = cx
            .debug_bounds("reference-copy-key")
            .expect("key button bounds");
        cx.simulate_click(key.center(), Modifiers::default());
        assert_eq!(
            cx.read_from_clipboard()
                .expect("clipboard")
                .text()
                .as_deref(),
            Some("@Kano2025")
        );

        let path = cx
            .debug_bounds("reference-copy-path")
            .expect("path button bounds");
        cx.simulate_click(path.center(), Modifiers::default());
        assert_eq!(
            cx.read_from_clipboard()
                .expect("clipboard")
                .text()
                .as_deref(),
            Some(pdf.display().to_string().as_str())
        );
        assert_eq!(
            copies.borrow().as_slice(),
            &[
                ReferenceCopy::Key("Kano2025".to_owned()),
                ReferenceCopy::Path(pdf),
            ]
        );

        let _ = std::fs::remove_dir_all(dir);
    }

    struct SelectHost {
        entry: ReferenceEntry,
        state: Rc<RefCell<(bool, usize)>>,
    }

    impl Render for SelectHost {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let state = Rc::clone(&self.state);
            let selected = state.borrow().0;
            div().w(px(420.)).h(px(80.)).child(
                ReferenceRow::new(0, self.entry.clone())
                    .selected(selected)
                    .on_select(move |_, _| {
                        let mut state = state.borrow_mut();
                        state.0 = !state.0;
                        state.1 += 1;
                    }),
            )
        }
    }

    #[gpui_kit::test]
    fn hovering_reveals_the_plus_and_clicking_toggles_selection(cx: &mut TestAppContext) {
        cx.update(crate::init);
        let state = Rc::new(RefCell::new((false, 0)));
        let host_state = Rc::clone(&state);
        let (_, cx) = cx.add_window_view(move |_, _| SelectHost {
            entry: entry("Kano2025", None),
            state: host_state,
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));

        // The slot is laid out in every frame; the pointer makes it visible.
        assert!(cx.debug_bounds("reference-select-0").is_some());
        cx.simulate_mouse_move(point(px(200.), px(30.)), None, Modifiers::default());
        cx.update(|window, cx| window.draw(cx).clear(cx));

        let plus = cx
            .debug_bounds("reference-select-0")
            .expect("selection toggle bounds");
        cx.simulate_click(plus.center(), Modifiers::default());
        assert_eq!(*state.borrow(), (true, 1));

        cx.simulate_click(plus.center(), Modifiers::default());
        assert_eq!(*state.borrow(), (false, 2));
    }

    struct BarHost {
        entries: Vec<ReferenceEntry>,
        actions: Rc<RefCell<Vec<ReferenceAction>>>,
    }

    impl Render for BarHost {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let actions = Rc::clone(&self.actions);
            div().w(px(420.)).h(px(60.)).child(
                ReferenceSelectionBar::new(self.entries.clone()).on_action(
                    move |action, entries, _, _| {
                        actions.borrow_mut().push(action);
                        assert!(!entries.is_empty());
                    },
                ),
            )
        }
    }

    #[gpui_kit::test]
    fn the_selection_bar_copies_keys_and_paths(cx: &mut TestAppContext) {
        cx.update(crate::init);
        let dir = temp_dir("bar");
        let pdf = dir.join("paper.pdf");
        std::fs::write(&pdf, b"%PDF-1.4").expect("pdf");
        let expected_path = pdf.display().to_string();
        let actions = Rc::new(RefCell::new(Vec::new()));
        let host_actions = Rc::clone(&actions);
        let (_, cx) = cx.add_window_view(move |_, _| BarHost {
            entries: vec![entry("B", Some(pdf.clone())), entry("A", None)],
            actions: host_actions,
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));

        let keys = cx
            .debug_bounds("reference-bulk-copy-keys-button")
            .expect("copy keys bounds");
        cx.simulate_click(keys.center(), Modifiers::default());
        assert_eq!(
            cx.read_from_clipboard()
                .expect("clipboard")
                .text()
                .as_deref(),
            Some("@B @A")
        );

        let paths = cx
            .debug_bounds("reference-bulk-copy-paths-button")
            .expect("copy paths bounds");
        cx.simulate_click(paths.center(), Modifiers::default());
        assert_eq!(
            cx.read_from_clipboard()
                .expect("clipboard")
                .text()
                .as_deref(),
            Some(expected_path.as_str())
        );
        assert_eq!(
            actions.borrow().as_slice(),
            [ReferenceAction::CopyKeys, ReferenceAction::CopyPaths]
        );

        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    #[ignore = "requires a running GROBID server and test_library PDFs"]
    async fn grobid_extracts_the_test_library_end_to_end() {
        let source = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("test_library");
        let dir = temp_dir("grobid-e2e");
        let corpus = dir.join("corpus");
        std::fs::create_dir_all(&corpus).expect("corpus");
        let mut copied = 0;
        for entry in std::fs::read_dir(&source).expect("test library") {
            let path = entry.expect("entry").path();
            if path
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("pdf"))
            {
                let target = corpus.join(path.file_name().expect("file name"));
                std::fs::copy(&path, &target).expect("copy pdf");
                copied += 1;
            }
        }
        assert!(
            copied >= 3,
            "expected the supplied PDFs in {}",
            source.display()
        );

        let bibliography = dir.join("references.bib");
        let options = ReferenceOptions {
            grobid_url: "http://localhost:8070".to_owned(),
            rename: true,
            openalex: false,
            workers: 4,
        };
        let mut observer = ExtractionObserver::default();
        let (entries, report) = extract(&corpus, &bibliography, &options, &mut observer)
            .await
            .ok()
            .expect("GROBID extraction");

        assert_eq!(entries.len(), copied);
        assert_eq!(report.merged, copied);
        assert_eq!(report.renamed, copied);
        assert!(entries.iter().all(|entry| !entry.key.is_empty()));
        assert!(
            entries
                .iter()
                .all(|entry| entry.path.as_ref().is_some_and(|path| path.is_file()))
        );

        // The rows must also be citable: the `file` links written by the
        // pre-pass resolve, and the real PDF parser returns text.
        let keys = vec![entries[0].key.clone()];
        let mut state = CiteState::default();
        let parsers = DocumentParsers::new(ragrig::build_parsers());
        let resolution = resolve_citations(
            &mut state,
            &bibliography,
            std::slice::from_ref(&corpus),
            &keys,
            &parsers,
        );
        assert_eq!(resolution.resolved, keys);
        assert_eq!(resolution.attachments.len(), 1);
        assert!(!resolution.attachments[0].content.trim().is_empty());

        let _ = std::fs::remove_dir_all(dir);
    }
}
