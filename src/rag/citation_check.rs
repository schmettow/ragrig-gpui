//! Check the `@key` citations of a selected passage against their PDFs.
//!
//! The checker is deliberately independent of the chat transcript: it builds
//! its own generator from the configured chat backend and runs up to
//! [`CHECK_CONCURRENCY`] citations at a time. The host receives one row per
//! citation occurrence and writes the report, so the author can keep working.

use std::{
    collections::{HashMap, HashSet},
    ops::Range,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use anyhow::anyhow;
use async_channel::Sender;
use futures_util::{StreamExt as _, stream};
use ragrig::{
    ChatAgentSpec, DocumentParsers, Generator,
    parsers::extract_text,
    progress::{Cancel as _, CancellationToken},
    types::{ChatConfig, ParseConfig},
};
use serde_json::Value;

use crate::references::{ReferenceEntry, load_entries};

use super::{
    CitationCheckRequest, CitationCheckRow, CitationCheckSummary, LibraryDocument, LibrarySnapshot,
    QuoteQuality, RagEvent, RequestId,
};

/// How many citations are checked at the same time.
const CHECK_CONCURRENCY: usize = 4;
/// Sentence window before the cited sentence.
const CONTEXT_BEFORE: usize = 3;
/// Sentence window after the cited sentence.
const CONTEXT_AFTER: usize = 2;
/// Maximum tokens in the claim context sent to the model.
const CONTEXT_MAX_TOKENS: usize = 500;
/// Tokens reserved for the model's JSON answer.
const ANSWER_TOKENS: usize = 1_200;
/// Tokens reserved for instructions and metadata.
const PROMPT_OVERHEAD_TOKENS: usize = 700;

/// Everything the checker needs from the worker configuration.
pub(crate) struct CitationCheckEnv {
    /// Bibliography used for key resolution and metadata.
    pub bibliography: PathBuf,
    /// Workspace root; relative `file` fields resolve against it.
    pub base: PathBuf,
    /// Folder corpora that count as "in the corpus".
    pub corpora: Vec<PathBuf>,
    /// Parser configuration used by ingestion.
    pub parse: ParseConfig,
    /// Chat backend configuration.
    pub chat: ChatConfig,
}

/// One `@key` occurrence in the selection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Mention {
    /// Bare key without the `@`.
    pub key: String,
    /// Byte range of the whole `@key` token in the document.
    pub range: Range<usize>,
}

/// Run one citation check and emit rows as they complete.
pub(crate) async fn run(
    id: RequestId,
    request: CitationCheckRequest,
    env: CitationCheckEnv,
    library: LibrarySnapshot,
    events: Sender<RagEvent>,
    cancel: CancellationToken,
) {
    let document_len = request.document_text.len();
    let selection = char_range_to_bytes(&request.document_text, request.selection.clone());
    let selection = selection.start.min(document_len)..selection.end.min(document_len);
    let mentions = citation_mentions(&request.document_text, Some(selection));
    let total = mentions.len();

    if events
        .send(RagEvent::CitationCheckStarted {
            id,
            total,
            document: request.document.clone(),
            selection: request.selection.clone(),
        })
        .await
        .is_err()
    {
        return;
    }
    if total == 0 {
        let _ = events
            .send(RagEvent::CitationCheckFinished {
                id,
                summary: CitationCheckSummary::default(),
            })
            .await;
        return;
    }
    if cancel.is_cancelled() {
        let _ = events.send(RagEvent::CitationCheckStopped { id }).await;
        return;
    }

    let entries = match load_entries(&env.bibliography, &env.base, Some(&env.base)) {
        Ok(entries) => entries,
        Err(message) => {
            let _ = events
                .send(RagEvent::CitationCheckFailed {
                    id,
                    message: format!("cannot read the bibliography: {message}"),
                })
                .await;
            return;
        }
    };
    let by_key = entry_map(&entries);

    let keys: Vec<String> = {
        let mut seen = HashSet::new();
        mentions
            .iter()
            .filter(|mention| seen.insert(mention.key.to_ascii_lowercase()))
            .map(|mention| mention.key.clone())
            .collect()
    };
    let mut state = grobid_bibtex::cite::CiteState::default();
    let corpus_paths: Vec<&Path> = env.corpora.iter().map(PathBuf::as_path).collect();
    let located = state.locate_with_base(
        &keys,
        Some(&env.bibliography),
        Some(&env.base),
        &corpus_paths,
    );
    let mut resolutions = HashMap::new();
    for (requested, item) in keys.iter().zip(located) {
        resolutions.insert(requested.to_lowercase(), item);
    }

    let parsers = Arc::new(DocumentParsers::new(super::actor::filtered_parsers(
        &env.parse,
    )));
    let parsed = parse_sources(&mentions, &resolutions, &library, &parsers, &cancel).await;
    if cancel.is_cancelled() {
        let _ = events.send(RagEvent::CitationCheckStopped { id }).await;
        return;
    }

    let chat = citation_chat_config(&env.chat);
    let generator = match ChatAgentSpec::try_from(&chat)
        .map_err(|error| anyhow!("cannot build the citation-check model: {error:#}"))
        .and_then(|spec| spec.build().map_err(|error| anyhow!("{error:#}")))
    {
        Ok(generator) => generator,
        Err(error) => {
            let _ = events
                .send(RagEvent::CitationCheckFailed {
                    id,
                    message: format!("{error:#}"),
                })
                .await;
            return;
        }
    };

    let mut summary = CitationCheckSummary {
        total,
        ..CitationCheckSummary::default()
    };
    let mut pending = Vec::new();
    for (offset, mention) in mentions.into_iter().enumerate() {
        let index = offset + 1;
        let citation = format!("@{}", mention.key);
        let resolved = resolutions.get(&mention.key.to_ascii_lowercase());
        let mut errors = Vec::new();
        let claim_context = context_window(
            &request.document_text,
            mention.range.clone(),
            CONTEXT_BEFORE,
            CONTEXT_AFTER,
            CONTEXT_MAX_TOKENS,
            &mut errors,
        );
        let claim = citation_sentence(&request.document_text, mention.range.start)
            .unwrap_or_else(|| claim_context.clone());
        let source = match resolved {
            Some(item) => match source_for(item, &parsed, &library) {
                Ok(source) => Some(source),
                Err(error) => {
                    errors.push(error);
                    None
                }
            },
            None => {
                errors.push("citation could not be resolved".to_owned());
                None
            }
        };
        let entry = resolved
            .and_then(|item| match item {
                grobid_bibtex::cite::Located::Found { key, .. } => by_key.get(&key.to_lowercase()),
                _ => None,
            })
            .cloned();
        let source_label = source_label(entry.as_ref(), resolved);
        let prepared = PreparedCitation {
            index,
            citation,
            claim,
            claim_context,
            source,
            source_label,
            errors,
            context_tokens: env.chat.context_tokens,
        };
        if prepared.source.is_none() {
            let row = prepared.failed_row(&prepared.errors);
            summary.errors += 1;
            let _ = events.send(RagEvent::CitationCheckRow { id, row }).await;
        } else {
            pending.push(prepared);
        }
    }

    let mut checks = stream::iter(
        pending
            .into_iter()
            .map(|prepared| check_one(prepared, generator.as_ref(), &cancel)),
    )
    .buffer_unordered(CHECK_CONCURRENCY);
    while let Some(result) = checks.next().await {
        if cancel.is_cancelled() {
            drop(checks);
            let _ = events.send(RagEvent::CitationCheckStopped { id }).await;
            return;
        }
        match result {
            Ok(row) => {
                if row.verdict.is_some() {
                    summary.checked += 1;
                } else {
                    summary.errors += 1;
                }
                for quote in &row.quotes {
                    match quote.quality {
                        QuoteQuality::Accurate => summary.accurate += 1,
                        QuoteQuality::Approximate => summary.approximate += 1,
                        QuoteQuality::NotFound => summary.not_found += 1,
                    }
                }
                let _ = events.send(RagEvent::CitationCheckRow { id, row }).await;
            }
            Err(error) => {
                let _ = events
                    .send(RagEvent::CitationCheckFailed {
                        id,
                        message: format!("{error:#}"),
                    })
                    .await;
                return;
            }
        }
    }

    let _ = events
        .send(RagEvent::CitationCheckFinished { id, summary })
        .await;
}

/// A citation that passed resolution and parsing.
struct PreparedCitation {
    index: usize,
    citation: String,
    claim: String,
    claim_context: String,
    source: Option<Arc<String>>,
    source_label: String,
    errors: Vec<String>,
    context_tokens: usize,
}

impl PreparedCitation {
    fn failed_row(&self, errors: &[String]) -> CitationCheckRow {
        CitationCheckRow {
            index: self.index,
            citation: self.citation.clone(),
            claim: self.claim.clone(),
            verdict: None,
            verdict_label: None,
            explanation: String::new(),
            quotes: Vec::new(),
            source: self.source_label.clone(),
            error: (!errors.is_empty()).then(|| errors.join("; ")),
        }
    }
}

async fn check_one(
    prepared: PreparedCitation,
    generator: &dyn Generator,
    cancel: &CancellationToken,
) -> anyhow::Result<CitationCheckRow> {
    if cancel.is_cancelled() {
        return Err(anyhow!(ragrig::Cancelled));
    }
    let Some(source) = &prepared.source else {
        return Ok(prepared.failed_row(&prepared.errors));
    };
    let mut errors = prepared.errors.clone();
    let claim_tokens = token_count(&prepared.claim_context);
    let available = source_budget(prepared.context_tokens, claim_tokens);
    let material = source_material(source, &prepared.claim_context, available, &mut errors);
    if material.is_empty() {
        errors.push("the cited PDF produced no text".to_owned());
        return Ok(prepared.failed_row(&errors));
    }
    let prompt = build_prompt(
        &prepared.claim_context,
        &prepared.citation,
        &prepared.source_label,
        &material,
    );
    let raw = match generate(generator, &prompt, cancel).await {
        Ok(raw) => raw,
        Err(error) if error.downcast_ref::<ragrig::Cancelled>().is_some() => {
            return Err(error);
        }
        Err(error) => {
            errors.push(format!("check failed: {error:#}"));
            return Ok(prepared.failed_row(&errors));
        }
    };
    let answer = match parse_answer(&raw) {
        Ok(answer) => answer,
        Err(error) => {
            errors.push(error);
            return Ok(prepared.failed_row(&errors));
        }
    };

    let mut quotes = Vec::new();
    for text in answer.quotes.into_iter().take(3) {
        let quality = verify_quote(source, &text);
        quotes.push(super::CitationQuote { text, quality });
    }
    if quotes.is_empty() {
        errors.push("model returned no evidence quotes".to_owned());
    }
    if !quotes.is_empty()
        && quotes
            .iter()
            .all(|quote| quote.quality == QuoteQuality::NotFound)
    {
        errors.push("no quote could be verified against the parsed PDF".to_owned());
    }

    Ok(CitationCheckRow {
        index: prepared.index,
        citation: prepared.citation,
        claim: prepared.claim,
        verdict: Some(answer.verdict),
        verdict_label: Some(verdict_label(answer.verdict).to_owned()),
        explanation: answer.explanation,
        quotes,
        source: prepared.source_label,
        error: (!errors.is_empty()).then(|| errors.join("; ")),
    })
}

async fn generate(
    generator: &dyn Generator,
    prompt: &str,
    cancel: &CancellationToken,
) -> anyhow::Result<String> {
    let answer = Mutex::new(String::new());
    generator
        .generate_stream_with_cancel(
            prompt,
            &|token| answer.lock().expect("citation answer").push_str(&token),
            cancel,
        )
        .await?;
    Ok(answer.into_inner().expect("citation answer"))
}

fn answer_budget(context_tokens: usize) -> usize {
    ANSWER_TOKENS
        .min(context_tokens.saturating_sub(PROMPT_OVERHEAD_TOKENS + CONTEXT_MAX_TOKENS + 200))
        .max(256)
}

fn source_budget(context_tokens: usize, claim_tokens: usize) -> usize {
    let reserved = PROMPT_OVERHEAD_TOKENS + claim_tokens + 200 + answer_budget(context_tokens);
    context_tokens.saturating_sub(reserved).max(256)
}

fn citation_chat_config(chat: &ChatConfig) -> ChatConfig {
    let mut chat = chat.clone();
    chat.params.max_tokens = Some(answer_budget(chat.context_tokens));
    chat.params.temperature = Some(0.0);
    chat.system_prompt_path = None;
    chat
}

fn entry_map(entries: &[ReferenceEntry]) -> HashMap<String, ReferenceEntry> {
    entries
        .iter()
        .map(|entry| (entry.key.to_lowercase(), entry.clone()))
        .collect()
}

async fn parse_sources(
    mentions: &[Mention],
    resolutions: &HashMap<String, grobid_bibtex::cite::Located>,
    library: &LibrarySnapshot,
    parsers: &Arc<DocumentParsers>,
    cancel: &CancellationToken,
) -> HashMap<PathBuf, Result<Arc<String>, String>> {
    let mut paths = HashSet::new();
    for mention in mentions {
        if let Some(grobid_bibtex::cite::Located::Found { path, .. }) =
            resolutions.get(&mention.key.to_lowercase())
            && ingested(library, path)
        {
            paths.insert(path.clone());
        }
    }
    let mut parsed = HashMap::new();
    for path in paths {
        if cancel.is_cancelled() {
            break;
        }
        let parsers = Arc::clone(parsers);
        let parse_path = path.clone();
        let result = tokio::task::spawn_blocking(move || extract_text(&parsers, &parse_path))
            .await
            .unwrap_or_else(|error| Err(anyhow!("parser task failed: {error}")));
        parsed.insert(
            path,
            result.map(Arc::new).map_err(|error| format!("{error:#}")),
        );
    }
    parsed
}

fn source_for(
    located: &grobid_bibtex::cite::Located,
    parsed: &HashMap<PathBuf, Result<Arc<String>, String>>,
    library: &LibrarySnapshot,
) -> Result<Arc<String>, String> {
    match located {
        grobid_bibtex::cite::Located::Found { path, .. } => {
            if !ingested(library, path) {
                return Err(format!(
                    "'{}' is not embedded in the workspace index",
                    path.display()
                ));
            }
            match parsed.get(path) {
                Some(Ok(text)) => Ok(Arc::clone(text)),
                Some(Err(error)) => Err(error.clone()),
                None => Err(format!("'{}' could not be parsed", path.display())),
            }
        }
        grobid_bibtex::cite::Located::UnknownKey { key } => {
            Err(format!("@{key} is not in the bibliography or the corpus"))
        }
        grobid_bibtex::cite::Located::NoDocument { key, .. } => {
            Err(format!("@{key} has no PDF in the corpus"))
        }
        grobid_bibtex::cite::Located::OutsideCorpora { key, path } => Err(format!(
            "@{key} links '{}', which is outside the corpus",
            path.display()
        )),
        grobid_bibtex::cite::Located::Ambiguous { key, candidates } => Err(format!(
            "@{key} matches several corpus files: {}",
            candidates
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

fn ingested(library: &LibrarySnapshot, path: &Path) -> bool {
    let Some(document) = library_document(library, path) else {
        return false;
    };
    document.path.is_some()
}

fn library_document<'a>(library: &'a LibrarySnapshot, path: &Path) -> Option<&'a LibraryDocument> {
    let target = normalized_path(path);
    library.documents.iter().find(|document| {
        document
            .path
            .as_deref()
            .is_some_and(|stored| normalized_path(stored) == target)
    })
}

fn normalized_path(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

fn source_label(
    entry: Option<&ReferenceEntry>,
    located: Option<&grobid_bibtex::cite::Located>,
) -> String {
    if let Some(entry) = entry {
        let mut parts = Vec::new();
        if !entry.authors.is_empty() {
            parts.push(entry.authors.clone());
        }
        if !entry.year.is_empty() {
            parts.push(format!("({})", entry.year));
        }
        if !entry.title.is_empty() {
            parts.push(entry.title.clone());
        }
        if !parts.is_empty() {
            return parts.join(" ");
        }
    }
    match located {
        Some(grobid_bibtex::cite::Located::Found { path, .. }) => path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("PDF")
            .to_owned(),
        _ => "unknown source".to_owned(),
    }
}

/// Return every `@key` mention in `text`, optionally restricted to a byte
/// range. Markdown code spans and fences are ignored.
pub(crate) fn citation_mentions(text: &str, within: Option<Range<usize>>) -> Vec<Mention> {
    let code = code_ranges(text);
    let mut mentions = Vec::new();
    let bytes = text.as_bytes();
    let mut index = 0;
    while let Some(relative) = text[index..].find('@') {
        let at = index + relative;
        index = at + 1;
        if inside_ranges(&code, at) || escaped(text, at) {
            continue;
        }
        if at > 0 && bytes[at - 1].is_ascii_alphanumeric() {
            continue;
        }
        if let Some(range) = &within
            && (at < range.start || at >= range.end)
        {
            continue;
        }
        let start = at + 1;
        let mut end = start;
        for (offset, character) in text[start..].char_indices() {
            if !is_key_char(character) {
                break;
            }
            end = start + offset + character.len_utf8();
        }
        let token = text[start..end].trim_end_matches(['.', ':', '+', '-']);
        if token.is_empty()
            || !token
                .chars()
                .next()
                .is_some_and(|character| character.is_ascii_alphanumeric())
        {
            continue;
        }
        mentions.push(Mention {
            key: token.to_owned(),
            range: at..at + 1 + token.len(),
        });
    }
    mentions
}

fn is_key_char(character: char) -> bool {
    character.is_ascii_alphanumeric() || matches!(character, '_' | '.' | ':' | '+' | '-')
}

fn escaped(text: &str, at: usize) -> bool {
    let before = &text[..at];
    before.chars().rev().take_while(|&c| c == '\\').count() % 2 == 1
}

fn inside_ranges(ranges: &[Range<usize>], at: usize) -> bool {
    ranges
        .iter()
        .any(|range| at >= range.start && at < range.end)
}

/// Byte ranges of fenced and inline code.
fn code_ranges(text: &str) -> Vec<Range<usize>> {
    let mut ranges = Vec::new();
    let mut fence: Option<(usize, char, usize)> = None;
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        let line_start = offset;
        offset += line.len();
        let trimmed = line.trim_start();
        if let Some((kind, count)) = fence_kind(trimmed) {
            match &fence {
                Some((_, open, length)) if *open == kind && count >= *length => {
                    ranges.push(line_start..offset);
                    fence = None;
                }
                None => {
                    ranges.push(line_start..offset);
                    fence = Some((line_start, kind, count));
                }
                _ => {}
            }
            continue;
        }
        if fence.is_some() {
            ranges.push(line_start..offset);
            continue;
        }
        let mut local = 0;
        while local < line.len() {
            let Some(relative) = line[local..].find('`') else {
                break;
            };
            let start = local + relative;
            let count = line[start..].chars().take_while(|&c| c == '`').count();
            let after = start + count;
            let needle = "`".repeat(count);
            let Some(close) = line[after..].find(needle.as_str()) else {
                break;
            };
            let end = after + close + count;
            ranges.push(line_start + start..line_start + end);
            local = end;
        }
    }
    ranges
}

fn fence_kind(line: &str) -> Option<(char, usize)> {
    let marker = line.chars().next()?;
    if marker != '`' && marker != '~' {
        return None;
    }
    let count = line.chars().take_while(|&c| c == marker).count();
    (count >= 3).then_some((marker, count))
}

fn char_range_to_bytes(text: &str, range: Range<usize>) -> Range<usize> {
    let mut chars = text.char_indices();
    let mut start = text.len();
    let mut end = text.len();
    for (char_index, (byte, character)) in chars.by_ref().enumerate() {
        if char_index == range.start {
            start = byte;
        }
        if char_index == range.end {
            end = byte;
            break;
        }
        let _ = character;
    }
    start..end
}

/// The sentence containing `at`, or the paragraph when no punctuation splits.
fn citation_sentence(text: &str, at: usize) -> Option<String> {
    let sentences = sentence_ranges(text);
    let range = sentences
        .iter()
        .find(|range| at >= range.start && at < range.end)
        .or_else(|| sentences.last())?;
    Some(text[range.clone()].trim().to_owned())
}

/// A sentence window around `at`, capped at `max_tokens`.
fn context_window(
    text: &str,
    at: Range<usize>,
    before: usize,
    after: usize,
    max_tokens: usize,
    notes: &mut Vec<String>,
) -> String {
    let sentences = sentence_ranges(text);
    let Some(index) = sentences
        .iter()
        .position(|range| at.start >= range.start && at.start < range.end)
        .or_else(|| (!sentences.is_empty()).then_some(sentences.len() - 1))
    else {
        return section_around(text, at, max_tokens, notes);
    };
    let mut first = index.saturating_sub(before);
    let mut last = (index + after).min(sentences.len().saturating_sub(1));
    loop {
        let context = text[sentences[first].start..sentences[last].end].trim();
        if token_count(context) <= max_tokens || first == last {
            let mut context = context.to_owned();
            if token_count(&context) > max_tokens {
                context = truncate_tokens(&context, max_tokens);
                notes.push(format!("claim context truncated to {max_tokens} tokens"));
            }
            return context;
        }
        if first < index {
            first += 1;
        } else if last > index {
            last -= 1;
        } else {
            break;
        }
    }
    let context = text[sentences[index].start..sentences[index].end].trim();
    if token_count(context) > max_tokens {
        notes.push(format!("claim context truncated to {max_tokens} tokens"));
        truncate_tokens(context, max_tokens)
    } else {
        context.to_owned()
    }
}

fn section_around(
    text: &str,
    at: Range<usize>,
    max_tokens: usize,
    notes: &mut Vec<String>,
) -> String {
    let mut start = at.start.saturating_sub(max_tokens * 2);
    while !text.is_char_boundary(start) {
        start -= 1;
    }
    let mut end = (at.end + max_tokens * 2).min(text.len());
    while !text.is_char_boundary(end) {
        end += 1;
    }
    let context = text[start..end].trim();
    if token_count(context) > max_tokens {
        notes.push(format!("claim context truncated to {max_tokens} tokens"));
        truncate_tokens(context, max_tokens)
    } else {
        context.to_owned()
    }
}

/// Sentence byte ranges, punctuation included.
pub(crate) fn sentence_ranges(text: &str) -> Vec<Range<usize>> {
    let mut ranges = Vec::new();
    let mut start = 0;
    let bytes = text.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        let character = text[index..].chars().next().expect("char");
        let is_end = matches!(character, '.' | '!' | '?');
        if is_end {
            let mut end = index + character.len_utf8();
            // Include closing quotes/brackets.
            while let Some(next) = text[end..].chars().next() {
                if matches!(next, '"' | '\'' | ')' | ']' | '”' | '’' | '}' | '》') {
                    end += next.len_utf8();
                } else {
                    break;
                }
            }
            let next = text[end..].chars().next();
            let boundary = next.is_none_or(|next| next.is_whitespace());
            let abandon = character == '.'
                && (is_decimal(text, index)
                    || is_abbreviation(&text[start..end])
                    || text[index..].starts_with("..."));
            if boundary && !abandon {
                let range = trim_range(text, start..end);
                if range.start < range.end {
                    ranges.push(range);
                }
                start = end;
                while start < text.len()
                    && text[start..]
                        .chars()
                        .next()
                        .is_some_and(char::is_whitespace)
                {
                    start += text[start..]
                        .chars()
                        .next()
                        .map(char::len_utf8)
                        .unwrap_or(1);
                }
            }
            index = end;
            continue;
        }
        index += character.len_utf8();
    }
    if start < text.len() {
        let range = trim_range(text, start..text.len());
        if range.start < range.end {
            ranges.push(range);
        }
    }
    ranges
}

fn trim_range(text: &str, mut range: Range<usize>) -> Range<usize> {
    while range.start < range.end
        && text[range.start..]
            .chars()
            .next()
            .is_some_and(char::is_whitespace)
    {
        range.start += text[range.start..]
            .chars()
            .next()
            .map(char::len_utf8)
            .unwrap_or(1);
    }
    while range.end > range.start
        && text[..range.end]
            .chars()
            .next_back()
            .is_some_and(char::is_whitespace)
    {
        range.end -= text[..range.end]
            .chars()
            .next_back()
            .map(char::len_utf8)
            .unwrap_or(1);
    }
    range
}

fn is_decimal(text: &str, index: usize) -> bool {
    let before = text[..index].chars().next_back();
    let after = text[index + 1..].chars().next();
    before.is_some_and(|c| c.is_ascii_digit()) && after.is_some_and(|c| c.is_ascii_digit())
}

fn is_abbreviation(sentence: &str) -> bool {
    let word = sentence
        .trim_end_matches(['.', '!', '?', '"', '\'', ')', ']', '”', '’', '}'])
        .split_whitespace()
        .next_back()
        .unwrap_or_default()
        .to_lowercase();
    matches!(
        word.as_str(),
        "e.g"
            | "i.e"
            | "al"
            | "vs"
            | "fig"
            | "figs"
            | "eq"
            | "eqs"
            | "dr"
            | "prof"
            | "no"
            | "nr"
            | "cf"
            | "etc"
    )
}

/// Number of tokens according to the chunker used by Ragrig.
pub(crate) fn token_count(text: &str) -> usize {
    if text.trim().is_empty() {
        return 0;
    }
    chunkedrs::chunk(text)
        .max_tokens(100_000)
        .overlap(0)
        .split()
        .iter()
        .map(|chunk| chunk.token_count)
        .sum()
}

fn truncate_tokens(text: &str, max_tokens: usize) -> String {
    chunkedrs::chunk(text)
        .max_tokens(max_tokens.max(1))
        .overlap(0)
        .split()
        .first()
        .map(|chunk| chunk.content.clone())
        .unwrap_or_default()
}

fn source_material(source: &str, claim: &str, available: usize, notes: &mut Vec<String>) -> String {
    if source.trim().is_empty() {
        return String::new();
    }
    let total = token_count(source);
    if total <= available {
        return source.to_owned();
    }
    let chunks = chunkedrs::chunk(source).max_tokens(400).overlap(80).split();
    if chunks.is_empty() {
        return String::new();
    }
    let claim_tokens = word_set(claim);
    let mut ranked: Vec<(usize, f64, &chunkedrs::Chunk)> = chunks
        .iter()
        .enumerate()
        .map(|(index, chunk)| {
            let score = chunk_score(&chunk.content, &claim_tokens);
            (index, score, chunk)
        })
        .collect();
    ranked.sort_by(|left, right| {
        right
            .1
            .partial_cmp(&left.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| left.0.cmp(&right.0))
    });
    let mut selected = Vec::new();
    let mut used = 0usize;
    for (index, _, chunk) in &ranked {
        if used + chunk.token_count > available && !selected.is_empty() {
            continue;
        }
        selected.push((*index, chunk.content.as_str()));
        used += chunk.token_count;
        if used >= available {
            break;
        }
    }
    if selected.is_empty() {
        selected.push((ranked[0].0, ranked[0].2.content.as_str()));
    }
    selected.sort_by_key(|(index, _)| *index);
    let text = selected
        .iter()
        .map(|(_, content)| content.trim())
        .collect::<Vec<_>>()
        .join("\n\n[...]\n\n");
    let text = if token_count(&text) > available {
        truncate_tokens(&text, available)
    } else {
        text
    };
    notes.push(format!(
        "full PDF ({total} tokens) exceeded the {available}-token source budget; used {} excerpt(s)",
        selected.len()
    ));
    text
}

fn word_set(text: &str) -> HashSet<String> {
    text.split(|character: char| !character.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(|word| word.to_lowercase())
        .collect()
}

fn chunk_score(text: &str, claim: &HashSet<String>) -> f64 {
    if claim.is_empty() {
        return 0.0;
    }
    let words: Vec<String> = text
        .split(|character: char| !character.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(|word| word.to_lowercase())
        .collect();
    if words.is_empty() {
        return 0.0;
    }
    let matches = words.iter().filter(|word| claim.contains(*word)).count();
    matches as f64 / (words.len() as f64).sqrt()
}

fn build_prompt(claim: &str, citation: &str, source: &str, material: &str) -> String {
    format!(
        "You are checking one citation in a scientific manuscript.\n\n\
         The author's claim and its surrounding context:\n\
         <claim>\n{claim}\n</claim>\n\n\
         The cited source ({citation}, {source}):\n\
         <source>\n{material}\n</source>\n\n\
         Decide how well the cited source supports the claim. Use exactly one of these qualifiers:\n\
         1 = not at all\n\
         2 = indirectly\n\
         3 = repeating a third party's opinion\n\
         4 = theoretically justified\n\
         5 = empirically justified\n\n\
         Extract 1 to 3 direct quotes from the source as evidence. Copy each quote verbatim; do not paraphrase inside a quote.\n\
         Return only a JSON object with this shape:\n\
         {{\"verdict\": 1, \"explanation\": \"...\", \"quotes\": [\"...\", \"...\"]}}\n"
    )
}

#[derive(Debug)]
struct ModelAnswer {
    verdict: u8,
    explanation: String,
    quotes: Vec<String>,
}

fn parse_answer(raw: &str) -> Result<ModelAnswer, String> {
    let trimmed = raw.trim();
    let body = trimmed
        .strip_prefix("```json")
        .or_else(|| trimmed.strip_prefix("```"))
        .and_then(|body| body.strip_suffix("```"))
        .unwrap_or(trimmed)
        .trim();
    let start = body.find('{').ok_or_else(|| {
        format!(
            "unstructured answer (no JSON object): {}",
            truncate_chars(body, 200)
        )
    })?;
    let end = body.rfind('}').ok_or_else(|| {
        format!(
            "unstructured answer (no closing brace): {}",
            truncate_chars(body, 200)
        )
    })?;
    let value: Value = serde_json::from_str(&body[start..=end]).map_err(|error| {
        format!(
            "unstructured answer ({error}): {}",
            truncate_chars(body, 200)
        )
    })?;
    let verdict = value
        .get("verdict")
        .and_then(|value| {
            value
                .as_u64()
                .or_else(|| value.as_str().and_then(|text| text.trim().parse().ok()))
        })
        .filter(|value| (1..=5).contains(value))
        .ok_or_else(|| "model returned no valid verdict (1-5)".to_owned())? as u8;
    let explanation = value
        .get("explanation")
        .or_else(|| value.get("assessment"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_owned();
    let quotes = value
        .get("quotes")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    item.as_str().map(str::to_owned).or_else(|| {
                        item.get("text")
                            .or_else(|| item.get("quote"))
                            .and_then(Value::as_str)
                            .map(str::to_owned)
                    })
                })
                .map(|quote| quote.trim().to_owned())
                .filter(|quote| !quote.is_empty())
                .collect()
        })
        .unwrap_or_default();
    Ok(ModelAnswer {
        verdict,
        explanation,
        quotes,
    })
}

fn truncate_chars(text: &str, limit: usize) -> String {
    let mut out: String = text.chars().take(limit).collect();
    if text.chars().count() > limit {
        out.push('…');
    }
    out
}

fn verdict_label(verdict: u8) -> &'static str {
    match verdict {
        1 => "not at all",
        2 => "indirectly",
        3 => "repeating a third party's opinion",
        4 => "theoretically justified",
        5 => "empirically justified",
        _ => "unknown",
    }
}

/// Verify a quote with a precise match first, then a fuzzy token window.
pub(crate) fn verify_quote(source: &str, quote: &str) -> QuoteQuality {
    let source_norm = normalize_text(source);
    let quote_norm = normalize_text(quote);
    if quote_norm.is_empty() {
        return QuoteQuality::NotFound;
    }
    if source_norm.contains(&quote_norm) {
        return QuoteQuality::Accurate;
    }
    // A PDF may hyphenate a word across a line while the model writes it
    // with the hyphen (or without it). Compare both without hyphens before
    // falling back to the fuzzy window.
    if source_norm
        .replace('-', "")
        .contains(&quote_norm.replace('-', ""))
    {
        return QuoteQuality::Accurate;
    }
    if source_norm
        .to_lowercase()
        .contains(&quote_norm.to_lowercase())
        || source_norm
            .replace('-', "")
            .to_lowercase()
            .contains(&quote_norm.replace('-', "").to_lowercase())
    {
        return QuoteQuality::Approximate;
    }
    let source_tokens = words(&source_norm);
    let quote_tokens = words(&quote_norm);
    if quote_tokens.len() < 4 || source_tokens.is_empty() {
        return QuoteQuality::NotFound;
    }
    let mut best: f64 = 0.0;
    for length in quote_tokens.len().saturating_sub(2)..=quote_tokens.len() + 2 {
        if length == 0 || length > source_tokens.len() {
            continue;
        }
        for window in source_tokens.windows(length) {
            let score = overlap_score(window, &quote_tokens);
            if score > best {
                best = score;
                if best >= 1.0 {
                    return QuoteQuality::Approximate;
                }
            }
        }
    }
    if best >= 0.8 {
        QuoteQuality::Approximate
    } else {
        QuoteQuality::NotFound
    }
}

fn overlap_score(window: &[String], quote: &[String]) -> f64 {
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for word in quote {
        *counts.entry(word.as_str()).or_default() += 1;
    }
    let mut matches = 0usize;
    for word in window {
        if let Some(count) = counts.get_mut(word.as_str())
            && *count > 0
        {
            *count -= 1;
            matches += 1;
        }
    }
    matches as f64 / window.len().max(quote.len()) as f64
}

fn words(text: &str) -> Vec<String> {
    text.split(|character: char| !character.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(|word| word.to_lowercase())
        .collect()
}

fn normalize_text(text: &str) -> String {
    let mut joined = String::with_capacity(text.len());
    let mut characters = text.chars().peekable();
    while let Some(character) = characters.next() {
        if character == '-' && matches!(characters.peek(), Some('\n' | '\r')) {
            characters.next();
            if characters.peek() == Some(&'\n') {
                characters.next();
            }
            continue;
        }
        let normalized = match character {
            '“' | '”' | '„' | '‟' => '"',
            '‘' | '’' | '‚' => '\'',
            '–' | '—' => '-',
            '\u{00a0}' => ' ',
            other => other,
        };
        joined.push(normalized);
    }
    joined.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Debug)]
    struct FakeGenerator {
        answer: String,
        fail: bool,
    }

    #[async_trait::async_trait]
    impl Generator for FakeGenerator {
        async fn generate_stream(
            &self,
            _prompt: &str,
            on_token: &(dyn Fn(String) + Sync),
        ) -> anyhow::Result<()> {
            if self.fail {
                return Err(anyhow!("model unavailable"));
            }
            on_token(self.answer.clone());
            Ok(())
        }

        fn backend_name(&self) -> &'static str {
            "fake"
        }

        fn model_name(&self) -> &str {
            "fake"
        }
    }

    fn prepared(source: &str) -> PreparedCitation {
        PreparedCitation {
            index: 1,
            citation: "@Key".to_owned(),
            claim: "The treatment reduces response times.".to_owned(),
            claim_context: "The treatment reduces response times in the trial.".to_owned(),
            source: Some(Arc::new(source.to_owned())),
            source_label: "Author (2020) Title".to_owned(),
            errors: Vec::new(),
            context_tokens: 8_192,
        }
    }

    #[tokio::test]
    async fn a_fake_model_produces_a_checked_row() {
        let generator = FakeGenerator {
            answer: "{\"verdict\":5,\"explanation\":\"measured\",\"quotes\":[\"The treatment reduced response times\"]}"
                .to_owned(),
            fail: false,
        };
        let cancel = CancellationToken::new();
        let row = check_one(
            prepared("The treatment reduced response times in the second trial."),
            &generator,
            &cancel,
        )
        .await
        .expect("row");
        assert_eq!(row.verdict, Some(5));
        assert_eq!(row.verdict_label.as_deref(), Some("empirically justified"));
        assert_eq!(row.quotes[0].quality, QuoteQuality::Accurate);
        assert!(row.error.is_none());
    }

    #[tokio::test]
    async fn a_generation_error_becomes_a_row_error() {
        let generator = FakeGenerator {
            answer: String::new(),
            fail: true,
        };
        let cancel = CancellationToken::new();
        let row = check_one(prepared("Source text."), &generator, &cancel)
            .await
            .expect("row");
        assert_eq!(row.verdict, None);
        assert!(
            row.error
                .as_deref()
                .is_some_and(|error| error.contains("model unavailable"))
        );
    }

    #[test]
    fn mentions_ignore_code_emails_and_escapes() {
        let text = "See @Kano2025 and [@Smith2020, p. 4].\\\n\
                    Write `@Code2020` and `@Other2020`.\n\
                    ```\n@Fenced2020\n```\n\
                    mail a@b.com and \\@Escaped2020.";
        let mentions = citation_mentions(text, None);
        let keys: Vec<&str> = mentions
            .iter()
            .map(|mention| mention.key.as_str())
            .collect();
        assert_eq!(keys, ["Kano2025", "Smith2020"]);
    }

    #[test]
    fn mentions_are_restricted_to_the_selection() {
        let text = "@Before starts. Then @Inside here.";
        let start = text.find("@Inside").expect("inside");
        let mentions = citation_mentions(text, Some(start..text.len()));
        assert_eq!(mentions.len(), 1);
        assert_eq!(mentions[0].key, "Inside");
    }

    #[test]
    fn sentences_keep_abbreviations_and_decimals_together() {
        let text = "This is e.g. a test. It costs 3.14 euro! Is it? Yes.";
        let ranges = sentence_ranges(text);
        assert_eq!(ranges.len(), 4);
        assert_eq!(&text[ranges[0].clone()], "This is e.g. a test.");
        assert_eq!(&text[ranges[1].clone()], "It costs 3.14 euro!");
        assert_eq!(&text[ranges[2].clone()], "Is it?");
        assert_eq!(&text[ranges[3].clone()], "Yes.");
    }

    #[test]
    fn context_window_takes_three_before_and_two_after() {
        let text = "One. Two. Three. Four @Key here. Five. Six. Seven.";
        let at = text.find("@Key").expect("mention");
        let mut notes = Vec::new();
        let context = context_window(text, at..at + 4, 3, 2, 500, &mut notes);
        assert_eq!(context, "One. Two. Three. Four @Key here. Five. Six.");
        assert!(notes.is_empty());
    }

    #[test]
    fn quote_verification_cascades_precise_fuzzy_missing() {
        let source = "The treatment reduced response times substantially in the second experiment.";
        assert_eq!(
            verify_quote(source, "reduced response times substantially"),
            QuoteQuality::Accurate
        );
        assert_eq!(
            verify_quote(source, "reduced the response times substantially"),
            QuoteQuality::Approximate
        );
        assert_eq!(
            verify_quote(source, "the treatment increased costs dramatically"),
            QuoteQuality::NotFound
        );
    }

    #[test]
    fn quote_verification_normalises_hyphenation_and_quotes() {
        let source = "A well-\nknown “result” appears here.";
        assert_eq!(
            verify_quote(source, "a well-known \"result\""),
            QuoteQuality::Approximate
        );
        assert_eq!(
            verify_quote(source, "A well-known “result” appears here."),
            QuoteQuality::Accurate
        );
    }

    #[test]
    fn answer_parsing_accepts_fences_and_quote_objects() {
        let raw = "```json\n{\"verdict\": \"4\", \"explanation\": \"because\", \"quotes\": [\"one\", {\"text\": \"two\"}]}\n```";
        let answer = parse_answer(raw).expect("answer");
        assert_eq!(answer.verdict, 4);
        assert_eq!(answer.explanation, "because");
        assert_eq!(answer.quotes, ["one", "two"]);
    }

    #[test]
    fn answer_parsing_rejects_an_out_of_range_verdict() {
        let error = parse_answer("{\"verdict\": 9}").expect_err("invalid verdict");
        assert!(error.contains("valid verdict"));
    }

    #[test]
    fn the_prompt_contains_the_exact_qualifiers() {
        let prompt = build_prompt("claim", "@Key", "Source", "text");
        for label in [
            "not at all",
            "indirectly",
            "repeating a third party's opinion",
            "theoretically justified",
            "empirically justified",
        ] {
            assert!(prompt.contains(label), "missing {label}");
        }
    }

    #[test]
    fn long_sources_fall_back_to_excerpts() {
        let source = (0..200)
            .map(|index| format!("Paragraph {index} about response times and results."))
            .collect::<Vec<_>>()
            .join(" ");
        let mut notes = Vec::new();
        let material = source_material(&source, "response times", 300, &mut notes);
        assert!(token_count(&material) <= 300);
        assert!(notes.iter().any(|note| note.contains("excerpt")));
    }
}
