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
//!   actions appear while the pointer is over it.
//!
//! Available with the `grobid` feature.

use biblatex::ChunksExt;
use std::{
    path::{Path, PathBuf},
    rc::Rc,
};

use gpui_kit::{
    App, ClipboardItem, ElementId, InteractiveElement as _, IntoElement, ParentElement as _,
    RenderOnce, SharedString, Styled as _, Window,
    assets::IconName,
    component::{
        ActiveTheme as _, Disableable as _, Sizable as _,
        button::{Button, ButtonVariants as _},
        h_flex, v_flex,
    },
    div,
    prelude::FluentBuilder as _,
    px,
};
use grobid_bibtex::{
    Collection,
    files::collect_pdfs,
    prepass::{
        BibtexTarget, PrepassConfig, PrepassError, PrepassObserver, PrepassProgress, PrepassReport,
        PrepassStage, prepass_with,
    },
};

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

/// A two-line reference row with hover-revealed copy actions.
///
/// While the pointer is over the row, **Copy @key** and **Copy path** appear
/// at its trailing edge. Both buttons are laid out in every frame, so the text
/// never jumps when they become visible. A row without a resolved PDF keeps
/// the path button disabled and explains why in its tooltip.
#[derive(IntoElement)]
pub struct ReferenceRow {
    index: usize,
    entry: ReferenceEntry,
    on_copy: Option<CopyHandler>,
}

impl ReferenceRow {
    /// A row for the entry at `index` in the current list.
    pub fn new(index: usize, entry: ReferenceEntry) -> Self {
        Self {
            index,
            entry,
            on_copy: None,
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
            .border_color(theme.border)
            .hover(|this| this.bg(theme.muted.opacity(0.28)))
            .child(
                div()
                    .w(px(3.))
                    .h(px(28.))
                    .flex_none()
                    .rounded_full()
                    .bg(theme.primary.opacity(0.65)),
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

        let _ = std::fs::remove_dir_all(dir);
    }
}
