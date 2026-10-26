//! Editable Ragrig configuration for the settings page and profiles.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

use ragrig::{
    ChatConfig, EmbedConfig, GenerationParams, MemoryConfig, MemoryStrategyKind, ParseConfig,
    PdfParserBackend, RagrigConfig,
    types::{EmbeddingProvider, Provider},
};

/// The GROBID URL used when no server URL has been configured.
pub const DEFAULT_GROBID_URL: &str = "http://localhost:8070";

/// The default managed GROBID image: all CRF and Deep Learning models.
pub const DEFAULT_GROBID_IMAGE: &str = "grobid/grobid:0.9.1-full";

/// Chat backend offered by the settings page.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ChatProvider {
    /// Local Ollama server.
    #[default]
    Ollama,
    /// DeepSeek cloud API; the key comes from `DEEPSEEK_API_KEY`.
    DeepSeek,
    /// OpenAI API; the key comes from `OPENAI_API_KEY`.
    OpenAi,
    /// Anthropic Claude API; the key comes from `ANTHROPIC_API_KEY`.
    Claude,
    /// Mistral API; the key comes from `MISTRAL_API_KEY`.
    Mistral,
    /// OpenRouter API; the key comes from `OPENROUTER_API_KEY`.
    OpenRouter,
}

impl ChatProvider {
    /// Human-readable name.
    pub fn label(self) -> &'static str {
        match self {
            Self::Ollama => "Ollama",
            Self::DeepSeek => "DeepSeek",
            Self::OpenAi => "OpenAI",
            Self::Claude => "Claude",
            Self::Mistral => "Mistral",
            Self::OpenRouter => "OpenRouter",
        }
    }

    /// The environment variable holding this provider's API key.
    ///
    /// `None` for the local Ollama server.
    pub fn env_var(self) -> Option<&'static str> {
        match self {
            Self::Ollama => None,
            Self::DeepSeek => Some("DEEPSEEK_API_KEY"),
            Self::OpenAi => Some("OPENAI_API_KEY"),
            Self::Claude => Some("ANTHROPIC_API_KEY"),
            Self::Mistral => Some("MISTRAL_API_KEY"),
            Self::OpenRouter => Some("OPENROUTER_API_KEY"),
        }
    }

    /// The model used when the settings hold no explicit choice.
    pub fn default_model(self) -> &'static str {
        match self {
            Self::Ollama => "qwen3.5:9b",
            Self::DeepSeek => "deepseek-v4-pro",
            Self::OpenAi => "gpt-4o",
            Self::Claude => "claude-sonnet-4-6",
            Self::Mistral => "mistral-large-latest",
            Self::OpenRouter => "openrouter/auto",
        }
    }

    /// Options for a dropdown.
    pub fn options() -> &'static [(Self, &'static str)] {
        &[
            (Self::Ollama, "Ollama"),
            (Self::DeepSeek, "DeepSeek"),
            (Self::OpenAi, "OpenAI"),
            (Self::Claude, "Claude"),
            (Self::Mistral, "Mistral"),
            (Self::OpenRouter, "OpenRouter"),
        ]
    }

    /// Find the provider with the given dropdown label.
    pub fn from_label(label: &str) -> Option<Self> {
        Self::options()
            .iter()
            .find(|(_, known)| *known == label)
            .map(|(provider, _)| *provider)
    }
}

impl From<ChatProvider> for Provider {
    fn from(provider: ChatProvider) -> Self {
        match provider {
            ChatProvider::Ollama => Self::Ollama,
            ChatProvider::DeepSeek => Self::Deepseek,
            ChatProvider::OpenAi => Self::OpenAi,
            ChatProvider::Claude => Self::Claude,
            ChatProvider::Mistral => Self::Mistral,
            ChatProvider::OpenRouter => Self::OpenRouter,
        }
    }
}

impl From<Provider> for ChatProvider {
    fn from(provider: Provider) -> Self {
        match provider {
            Provider::Deepseek => Self::DeepSeek,
            Provider::OpenAi => Self::OpenAi,
            Provider::Claude => Self::Claude,
            Provider::Mistral => Self::Mistral,
            Provider::OpenRouter => Self::OpenRouter,
            _ => Self::Ollama,
        }
    }
}

/// PDF backend offered by the settings page.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PdfBackend {
    /// Legacy flat-text extraction.
    #[default]
    Extract,
    /// Markdown-native extraction.
    Unpdf,
    /// Structured, layout-aware extraction.
    Sink,
    /// Binary scavenger that never panics.
    Internal,
}

impl PdfBackend {
    /// Human-readable name.
    pub fn label(self) -> &'static str {
        match self {
            Self::Extract => "pdf-extract",
            Self::Unpdf => "unpdf",
            Self::Sink => "pdfsink",
            Self::Internal => "sloppy-pdf",
        }
    }

    /// Options for a dropdown.
    pub fn options() -> &'static [(Self, &'static str)] {
        &[
            (Self::Extract, "pdf-extract (flat text)"),
            (Self::Unpdf, "unpdf (Markdown)"),
            (Self::Sink, "pdfsink (layout)"),
            (Self::Internal, "sloppy-pdf (scavenger)"),
        ]
    }
}

/// Memory strategy offered by the settings page.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MemoryChoice {
    /// Rewrite the query with a small model before retrieval.
    #[default]
    Rewrite,
    /// Pass the transcript through without rewriting.
    Transcript,
    /// Blend the most recent saved session into the query.
    Log,
    /// Summarise past sessions with an LLM.
    Summary,
    /// No transcript, no rewriting.
    Off,
}

impl MemoryChoice {
    /// Human-readable name.
    pub fn label(self) -> &'static str {
        match self {
            Self::Rewrite => "Rewrite",
            Self::Transcript => "Transcript",
            Self::Log => "Log",
            Self::Summary => "Summary",
            Self::Off => "Off",
        }
    }

    /// Options for a dropdown.
    pub fn options() -> &'static [(Self, &'static str)] {
        &[
            (Self::Rewrite, "Rewrite (small model)"),
            (Self::Transcript, "Transcript only"),
            (Self::Log, "Log (last session)"),
            (Self::Summary, "Summary (LLM)"),
            (Self::Off, "Off"),
        ]
    }

    fn uses_model(self) -> bool {
        matches!(self, Self::Rewrite | Self::Log | Self::Summary)
    }
}

/// Where the GROBID server comes from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GrobidSource {
    /// Use a server that is already running (local or remote).
    #[default]
    Existing,
    /// Let Scico pull and run the official Docker image.
    Managed,
}

impl GrobidSource {
    /// Human-readable name, also used as the persisted value.
    pub fn label(self) -> &'static str {
        match self {
            Self::Existing => "Existing server",
            Self::Managed => "Docker container",
        }
    }

    /// Options for a dropdown.
    pub fn options() -> &'static [(Self, &'static str)] {
        &[
            (Self::Existing, "Existing server"),
            (Self::Managed, "Docker container"),
        ]
    }

    /// Parse the label stored in a settings file.
    pub fn from_label(label: &str) -> Option<Self> {
        Self::options()
            .iter()
            .find(|(_, option)| *option == label || option.eq_ignore_ascii_case(label))
            .map(|(source, _)| *source)
    }
}

/// Everything needed to build one [`ragrig::RagAgent`].
#[derive(Clone, Debug)]
pub struct RagSetup {
    /// Ragrig's own configuration tree.
    pub config: RagrigConfig,
    /// Bibliography the worker reads and writes.
    ///
    /// `None` uses the conventional `<workspace>/.ragrig/references.bib`.
    /// Hosts that keep their library elsewhere set this so `@key`
    /// citations, the reference rows, and GROBID all agree on one file.
    pub bibliography: Option<PathBuf>,
    /// Prompt text applied after building; takes precedence over
    /// `config.chat.system_prompt_path`.
    pub system_prompt: Option<String>,
    /// Whether answers stream token by token.
    pub stream: bool,
    /// Whether `@key` mentions in a question attach the paper as one-shot
    /// context. On by default.
    pub resolve_citations: bool,
    /// Whether hybrid retrieval (BM25 + embeddings) is enabled.
    ///
    /// Off builds a no-op embedder: questions still use explicit attachments
    /// and the bibliographic library, but no vector search runs and no
    /// embedding model is required. On by default.
    pub hybrid: bool,
    /// Where the vector store, sessions, and profiles live.
    ///
    /// `None` uses `<config.workspace>/.ragrig`. A host with a dedicated
    /// project folder (for example `.ragrig` inside the document workspace)
    /// sets this explicitly.
    pub data_dir: Option<PathBuf>,
}

impl RagSetup {
    /// Wrap a config without a prompt override.
    pub fn new(config: RagrigConfig) -> Self {
        Self {
            config,
            bibliography: None,
            system_prompt: None,
            stream: true,
            resolve_citations: true,
            hybrid: true,
            data_dir: None,
        }
    }

    /// Wrap a config and apply `prompt` before the agent is built.
    pub fn with_system_prompt(config: RagrigConfig, prompt: impl Into<String>) -> Self {
        Self {
            config,
            bibliography: None,
            system_prompt: Some(prompt.into()),
            stream: true,
            resolve_citations: true,
            hybrid: true,
            data_dir: None,
        }
    }

    /// Use `path` for the bibliography instead of the conventional file.
    pub fn with_bibliography(mut self, path: impl Into<PathBuf>) -> Self {
        self.bibliography = Some(path.into());
        self
    }

    /// Turn hybrid retrieval (embeddings) on or off.
    pub fn with_hybrid(mut self, hybrid: bool) -> Self {
        self.hybrid = hybrid;
        self
    }

    /// Use `dir` for the vector store, sessions, and profiles.
    pub fn with_data_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.data_dir = Some(dir.into());
        self
    }

    /// Whether conversation memory is on (`false` for the `Off` strategy).
    pub fn memory_enabled(&self) -> bool {
        !matches!(self.config.memory.strategy, MemoryStrategyKind::Off)
    }
}

impl From<RagrigConfig> for RagSetup {
    fn from(config: RagrigConfig) -> Self {
        Self::new(config)
    }
}

/// Editable configuration shown by the settings page.
///
/// This mirrors the Ragrig fields a chat surface exposes. Secrets never enter
/// this struct: cloud API keys are read from the environment by the worker.
#[derive(Clone, Debug, PartialEq)]
pub struct RagSettings {
    /// Workspace folder holding the store and sessions.
    pub workspace: String,
    /// Bibliography the library and `@key` resolution use.
    ///
    /// Empty means the conventional `<workspace>/.ragrig/references.bib`.
    pub bibliography: String,
    /// Active chat backend.
    pub provider: ChatProvider,
    /// Chat model per provider.
    ///
    /// Every provider keeps its own model, so switching backends does not
    /// carry a model name into an incompatible API. Missing entries use
    /// [`ChatProvider::default_model`] when the configuration is built.
    pub models: BTreeMap<ChatProvider, String>,
    /// Sampling temperature.
    pub temperature: f64,
    /// Maximum generated tokens.
    pub max_tokens: usize,
    /// Chat context window in tokens.
    pub context_tokens: usize,
    /// Inline system prompt, used when `system_prompt_path` is empty.
    pub system_prompt: String,
    /// Markdown file that overrides the inline prompt; empty uses
    /// `system_prompt`.
    pub system_prompt_path: String,
    /// The prompt the host compiles in.
    ///
    /// Hosts set this after loading their settings; **Use built-in** and
    /// profile defaults restore it. It is not written to profiles and is
    /// ignored by dirty tracking.
    pub built_in_prompt: String,
    /// Stream tokens into the transcript.
    pub stream: bool,
    /// Ollama embedding model.
    pub embed_model: String,
    /// Hybrid retrieval (BM25 + embeddings). Off runs without an embedding
    /// model and skips vector search.
    pub hybrid: bool,
    /// Chunks retrieved per query.
    pub top_k: usize,
    /// Minimum similarity for a chunk to enter the prompt.
    pub similarity_threshold: f64,
    /// Target chunk size in tokens.
    pub chunk_size: usize,
    /// Token overlap between chunks.
    pub chunk_overlap: usize,
    /// PDF parser selected as primary.
    pub pdf_backend: PdfBackend,
    /// Keep the sloppy PDF fallback registered.
    pub sloppy_pdf: bool,
    /// Conversation memory strategy.
    pub memory: MemoryChoice,
    /// Small model used for rewriting and summaries.
    pub memory_model: String,
    /// Where the GROBID server comes from.
    pub grobid_source: GrobidSource,
    /// Base URL of the GROBID server.
    pub grobid_url: String,
    /// Docker image used when [`Self::grobid_source`] is [`GrobidSource::Managed`].
    pub grobid_image: String,
    /// Start the managed GROBID container when the app opens.
    pub grobid_auto_start: bool,
    /// Rename PDFs to their metadata name during reference extraction.
    pub rename_pdfs: bool,
    /// Complete extracted references against OpenAlex.
    pub openalex: bool,
    /// Attach `@key` papers mentioned in a question as one-shot context.
    pub resolve_citations: bool,
    /// Named folder corpora in `name=path` form.
    pub corpus_dirs: Vec<String>,
    /// Profile name typed by the user.
    pub profile_name: String,
    /// Profiles found in the workspace.
    pub profiles: Vec<String>,
    /// Last settings action result, shown under the Apply row.
    pub status: Option<String>,
}

impl Default for RagSettings {
    fn default() -> Self {
        let config = RagrigConfig::default();
        let mut settings = Self::from_config(&config);
        settings.workspace = default_workspace().to_string_lossy().into_owned();
        settings
    }
}

/// One editable RAG setting.
///
/// The settings page uses this to mark exactly the fields that differ from the
/// configuration last applied to the worker.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RagField {
    /// Workspace folder.
    Workspace,
    /// Bibliography path.
    Bibliography,
    /// Chat backend.
    Provider,
    /// Model of the active chat backend.
    Model,
    /// Sampling temperature.
    Temperature,
    /// Maximum generated tokens.
    MaxTokens,
    /// Chat context window.
    ContextTokens,
    /// Streaming responses.
    Stream,
    /// System prompt.
    SystemPrompt,
    /// Embedding model.
    EmbedModel,
    /// Hybrid retrieval.
    Hybrid,
    /// Retrieved chunks.
    TopK,
    /// Minimum similarity.
    SimilarityThreshold,
    /// Named folder corpora.
    CorpusDirs,
    /// PDF parser backend.
    PdfBackend,
    /// Sloppy PDF fallback.
    SloppyPdf,
    /// Chunk size.
    ChunkSize,
    /// Chunk overlap.
    ChunkOverlap,
    /// Memory strategy.
    Memory,
    /// Memory model.
    MemoryModel,
    /// Where the GROBID server comes from.
    GrobidSource,
    /// GROBID server URL.
    GrobidUrl,
    /// Docker image for the managed GROBID container.
    GrobidImage,
    /// Start the managed GROBID container when the app opens.
    GrobidAutoStart,
    /// Rename processed PDFs.
    RenamePdfs,
    /// OpenAlex completion.
    OpenAlex,
    /// `@key` citation resolution.
    ResolveCitations,
}

impl RagField {
    /// The group this field is shown in.
    pub fn section(self) -> RagSection {
        match self {
            Self::Workspace | Self::Bibliography => RagSection::Workspace,
            Self::Provider
            | Self::Model
            | Self::Temperature
            | Self::MaxTokens
            | Self::ContextTokens
            | Self::Stream
            | Self::SystemPrompt => RagSection::Chat,
            Self::EmbedModel
            | Self::Hybrid
            | Self::TopK
            | Self::SimilarityThreshold
            | Self::CorpusDirs => RagSection::Retrieval,
            Self::PdfBackend | Self::SloppyPdf | Self::ChunkSize | Self::ChunkOverlap => {
                RagSection::Parsing
            }
            Self::Memory | Self::MemoryModel => RagSection::Memory,
            Self::GrobidSource | Self::GrobidUrl | Self::GrobidImage | Self::GrobidAutoStart => {
                RagSection::Services
            }
            Self::RenamePdfs | Self::OpenAlex | Self::ResolveCitations => RagSection::References,
        }
    }
}

/// A group of settings shown on one page.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RagSection {
    /// Workspace folder and bibliography.
    Workspace,
    /// Provider, models, generation, and prompt.
    Chat,
    /// Embedding and retrieval.
    Retrieval,
    /// Parser and chunking.
    Parsing,
    /// Memory strategy and model.
    Memory,
    /// Renaming, OpenAlex, and citations.
    References,
    /// External services such as the GROBID server.
    Services,
}

impl RagSection {
    /// Every section, in page order.
    pub const ALL: [Self; 7] = [
        Self::Workspace,
        Self::Chat,
        Self::Retrieval,
        Self::Parsing,
        Self::Memory,
        Self::References,
        Self::Services,
    ];

    /// The human-readable section name.
    pub fn label(self) -> &'static str {
        match self {
            Self::Workspace => "Workspace",
            Self::Chat => "Chat",
            Self::Retrieval => "Retrieval",
            Self::Parsing => "Parsing and chunking",
            Self::Memory => "Memory",
            Self::References => "References",
            Self::Services => "External services",
        }
    }
}

/// The editable fields that differ from the applied configuration.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RagChanges {
    fields: BTreeSet<RagField>,
}

impl RagChanges {
    /// Whether no setting changed.
    pub fn is_empty(&self) -> bool {
        self.fields.is_empty()
    }

    /// Whether any setting changed.
    pub fn any(&self) -> bool {
        !self.is_empty()
    }

    /// Whether one field changed.
    pub fn contains(&self, field: RagField) -> bool {
        self.fields.contains(&field)
    }

    /// Whether any field in a section changed.
    pub fn section(&self, section: RagSection) -> bool {
        self.fields.iter().any(|field| field.section() == section)
    }

    /// How many fields in a section changed.
    pub fn count_in(&self, section: RagSection) -> usize {
        self.fields
            .iter()
            .filter(|field| field.section() == section)
            .count()
    }

    /// The number of changed fields.
    pub fn len(&self) -> usize {
        self.fields.len()
    }

    /// Iterate over the changed fields in declaration order.
    pub fn fields(&self) -> impl Iterator<Item = RagField> + '_ {
        self.fields.iter().copied()
    }

    fn mark_if(&mut self, changed: bool, field: RagField) {
        if changed {
            self.fields.insert(field);
        }
    }
}

impl RagSettings {
    /// The model configured for the active provider.
    ///
    /// The value is returned as stored, so an edited field can be empty while
    /// the user types. [`Self::effective_model`] applies the provider default.
    pub fn model(&self) -> &str {
        self.model_for(self.provider)
    }

    /// The model configured for `provider`, as stored.
    pub fn model_for(&self, provider: ChatProvider) -> &str {
        self.models
            .get(&provider)
            .map(String::as_str)
            .unwrap_or_default()
    }

    /// The model configured for the active provider, or its default.
    pub fn effective_model(&self) -> &str {
        self.effective_model_for(self.provider)
    }

    /// The model configured for `provider`, or its default when unset.
    pub fn effective_model_for(&self, provider: ChatProvider) -> &str {
        let model = self.model_for(provider).trim();
        if model.is_empty() {
            provider.default_model()
        } else {
            model
        }
    }

    /// Set the model of the active provider.
    pub fn set_model(&mut self, model: impl Into<String>) {
        self.set_model_for(self.provider, model);
    }

    /// Set the model of `provider`.
    pub fn set_model_for(&mut self, provider: ChatProvider, model: impl Into<String>) {
        self.models.insert(provider, model.into());
    }

    fn default_models() -> BTreeMap<ChatProvider, String> {
        ChatProvider::options()
            .iter()
            .map(|(provider, _)| (*provider, provider.default_model().to_owned()))
            .collect()
    }

    /// Compare the editable configuration fields with the configuration that is
    /// currently running. Profile bookkeeping is ignored.
    pub fn dirty_since(&self, applied: &Self) -> RagChanges {
        let mut changes = RagChanges::default();
        changes.mark_if(self.workspace != applied.workspace, RagField::Workspace);
        changes.mark_if(
            self.bibliography != applied.bibliography,
            RagField::Bibliography,
        );
        changes.mark_if(self.provider != applied.provider, RagField::Provider);
        changes.mark_if(self.models != applied.models, RagField::Model);
        changes.mark_if(
            self.temperature != applied.temperature,
            RagField::Temperature,
        );
        changes.mark_if(self.max_tokens != applied.max_tokens, RagField::MaxTokens);
        changes.mark_if(
            self.context_tokens != applied.context_tokens,
            RagField::ContextTokens,
        );
        changes.mark_if(self.stream != applied.stream, RagField::Stream);
        changes.mark_if(
            self.system_prompt != applied.system_prompt
                || self.system_prompt_path != applied.system_prompt_path,
            RagField::SystemPrompt,
        );
        changes.mark_if(
            self.embed_model != applied.embed_model,
            RagField::EmbedModel,
        );
        changes.mark_if(self.hybrid != applied.hybrid, RagField::Hybrid);
        changes.mark_if(self.top_k != applied.top_k, RagField::TopK);
        changes.mark_if(
            self.similarity_threshold != applied.similarity_threshold,
            RagField::SimilarityThreshold,
        );
        changes.mark_if(
            self.corpus_dirs != applied.corpus_dirs,
            RagField::CorpusDirs,
        );
        changes.mark_if(
            self.pdf_backend != applied.pdf_backend,
            RagField::PdfBackend,
        );
        changes.mark_if(self.sloppy_pdf != applied.sloppy_pdf, RagField::SloppyPdf);
        changes.mark_if(self.chunk_size != applied.chunk_size, RagField::ChunkSize);
        changes.mark_if(
            self.chunk_overlap != applied.chunk_overlap,
            RagField::ChunkOverlap,
        );
        changes.mark_if(self.memory != applied.memory, RagField::Memory);
        changes.mark_if(
            self.memory_model != applied.memory_model,
            RagField::MemoryModel,
        );
        changes.mark_if(
            self.grobid_source != applied.grobid_source,
            RagField::GrobidSource,
        );
        changes.mark_if(self.grobid_url != applied.grobid_url, RagField::GrobidUrl);
        changes.mark_if(
            self.grobid_image != applied.grobid_image,
            RagField::GrobidImage,
        );
        changes.mark_if(
            self.grobid_auto_start != applied.grobid_auto_start,
            RagField::GrobidAutoStart,
        );
        changes.mark_if(
            self.rename_pdfs != applied.rename_pdfs,
            RagField::RenamePdfs,
        );
        changes.mark_if(self.openalex != applied.openalex, RagField::OpenAlex);
        changes.mark_if(
            self.resolve_citations != applied.resolve_citations,
            RagField::ResolveCitations,
        );
        changes
    }

    /// Reset the editable fields to the defaults of the last chosen profile.
    ///
    /// With no profile selected, the compiled defaults are used while keeping
    /// the workspace, bibliography, and profile bookkeeping so the library
    /// location is not lost.
    pub fn reset_to_profile_defaults(&mut self) -> Result<(), String> {
        let name = self.profile_name.trim().to_owned();
        if !name.is_empty() && self.profiles.iter().any(|profile| profile == &name) {
            return self.load_profile(&name);
        }

        let workspace = self.workspace.clone();
        let bibliography = self.bibliography.clone();
        let built_in_prompt = self.built_in_prompt.clone();
        let grobid_source = self.grobid_source;
        let grobid_url = self.grobid_url.clone();
        let grobid_image = self.grobid_image.clone();
        let grobid_auto_start = self.grobid_auto_start;
        let mut defaults = Self {
            workspace,
            bibliography,
            grobid_source,
            grobid_url,
            grobid_image,
            grobid_auto_start,
            profile_name: self.profile_name.clone(),
            built_in_prompt: built_in_prompt.clone(),
            system_prompt: built_in_prompt,
            system_prompt_path: String::new(),
            ..Self::default()
        };
        defaults.refresh_profiles();
        defaults.status = Some("Reset to built-in defaults — press Apply".to_owned());
        *self = defaults;
        Ok(())
    }
}

impl RagSettings {
    /// Read the editable fields from a Ragrig configuration.
    pub fn from_config(config: &RagrigConfig) -> Self {
        let provider = ChatProvider::from(config.chat.provider.clone());
        let mut models = Self::default_models();
        if !config.chat.deepseek_model.trim().is_empty() {
            models.insert(ChatProvider::DeepSeek, config.chat.deepseek_model.clone());
        }
        if !config.chat.model.trim().is_empty() {
            match provider {
                // `model` is ignored for DeepSeek; it carries the Ollama
                // choice so switching back keeps it.
                ChatProvider::DeepSeek => {
                    models.insert(ChatProvider::Ollama, config.chat.model.clone());
                }
                ChatProvider::Ollama => {
                    models.insert(ChatProvider::Ollama, config.chat.model.clone());
                }
                // Upstream treats the Ollama default as "provider default"
                // when another backend is selected.
                _ if config.chat.model == ChatProvider::Ollama.default_model() => {}
                _ => {
                    models.insert(provider, config.chat.model.clone());
                }
            }
        }
        let pdf_backend = match config.parse.pdf_parser {
            PdfParserBackend::Unpdf => PdfBackend::Unpdf,
            PdfParserBackend::Sink => PdfBackend::Sink,
            PdfParserBackend::Internal => PdfBackend::Internal,
            _ => PdfBackend::Extract,
        };
        let memory = match config.memory.strategy {
            MemoryStrategyKind::Transcript => MemoryChoice::Transcript,
            MemoryStrategyKind::Log => MemoryChoice::Log,
            MemoryStrategyKind::Summary => MemoryChoice::Summary,
            MemoryStrategyKind::Off => MemoryChoice::Off,
            _ => MemoryChoice::Rewrite,
        };
        let system_prompt_path = config
            .chat
            .system_prompt_path
            .as_ref()
            .map(|path| {
                if path.is_absolute() {
                    path.clone()
                } else {
                    config.workspace.join(path)
                }
            })
            .map(|path| path.display().to_string())
            .unwrap_or_default();
        let system_prompt = if system_prompt_path.is_empty() {
            default_prompt()
        } else {
            fs::read_to_string(&system_prompt_path)
                .ok()
                .filter(|text| !text.trim().is_empty())
                .unwrap_or_else(default_prompt)
        };

        Self {
            workspace: config.workspace.to_string_lossy().into_owned(),
            bibliography: String::new(),
            provider,
            models,
            temperature: config.chat.params.temperature.unwrap_or(0.2),
            max_tokens: config.chat.params.max_tokens.unwrap_or(2048),
            context_tokens: config.chat.context_tokens,
            system_prompt,
            system_prompt_path,
            built_in_prompt: default_prompt(),
            stream: true,
            embed_model: config.embed.model.clone(),
            hybrid: true,
            top_k: config.embed.top_k,
            similarity_threshold: config.embed.similarity_threshold,
            chunk_size: config.parse.chunk_size,
            chunk_overlap: config.parse.chunk_overlap,
            pdf_backend,
            sloppy_pdf: config.parse.sloppy_pdf,
            memory,
            memory_model: config.memory.model.clone(),
            grobid_source: GrobidSource::Existing,
            grobid_url: DEFAULT_GROBID_URL.to_owned(),
            grobid_image: DEFAULT_GROBID_IMAGE.to_owned(),
            grobid_auto_start: false,
            rename_pdfs: false,
            openalex: false,
            resolve_citations: true,
            corpus_dirs: config.corpus_dirs.clone(),
            profile_name: String::new(),
            profiles: Vec::new(),
            status: None,
        }
    }

    /// Read `~/.ragrig/config.json` and `<workspace>/.ragrig/config.json` as
    /// partial configurations and merge them over the built-in defaults.
    ///
    /// The merge is per setting: a key missing from a higher layer keeps the
    /// value from the layer below, so a workspace only states what it
    /// changes. Returns `None` when neither file exists.
    pub fn from_layers(workspace: &Path) -> Option<Self> {
        let global = home_ragrig_dir().map(|dir| dir.join("config.json"));
        Self::from_layer_paths(
            global.as_deref(),
            &workspace.join(".ragrig/config.json"),
            None,
        )
    }

    /// Like [`Self::from_layers`], but keeps the runtime memory and hybrid
    /// switches from `base` when no layer mentions them.
    ///
    /// The status-bar switches live in `settings.json` as well as the
    /// workspace layer; this prevents a layer that only sets, say, the chat
    /// model from resetting them to their defaults.
    pub fn from_layers_over(workspace: &Path, base: &Self) -> Option<Self> {
        let global = home_ragrig_dir().map(|dir| dir.join("config.json"));
        Self::from_layer_paths(
            global.as_deref(),
            &workspace.join(".ragrig/config.json"),
            Some(base),
        )
    }

    fn from_layer_paths(
        global: Option<&Path>,
        workspace: &Path,
        base: Option<&Self>,
    ) -> Option<Self> {
        let mut layers = serde_json::json!({});
        let mut found = false;
        for path in global.into_iter().chain(std::iter::once(workspace)) {
            let Ok(text) = fs::read_to_string(path) else {
                continue;
            };
            let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
                continue;
            };
            merge_json(&mut layers, value);
            found = true;
        }
        if !found {
            return None;
        }
        let memory_present = layers.pointer("/memory/strategy").is_some();
        let hybrid_present = layers.get("hybrid").is_some();
        let merged_hybrid = layers.get("hybrid").and_then(|value| value.as_bool());
        let mut merged = serde_json::to_value(RagrigConfig::default()).ok()?;
        merge_json(&mut merged, layers);
        let config: RagrigConfig = serde_json::from_value(merged).ok()?;
        let mut config = config;
        // The layer file lives in `<root>/.ragrig/`; relative prompts and
        // bibliographies resolve against the workspace root, not the cwd.
        if let Some(root) = workspace.parent().and_then(Path::parent) {
            config.workspace = root.to_path_buf();
        }
        let mut settings = Self::from_config(&config);
        if let Some(hybrid) = merged_hybrid {
            settings.hybrid = hybrid;
        }
        if let Some(base) = base {
            if !memory_present {
                settings.memory = base.memory;
            }
            if !hybrid_present {
                settings.hybrid = base.hybrid;
            }
        }
        Some(settings)
    }

    /// Validate and convert to a Ragrig configuration.
    pub fn to_config(&self) -> Result<RagrigConfig, String> {
        self.validate()?;

        let workspace = PathBuf::from(self.workspace.trim());
        let model = match self.provider {
            // `model` is ignored for DeepSeek; keep the Ollama choice so a
            // profile switched back to Ollama retains it.
            ChatProvider::DeepSeek => self.effective_model_for(ChatProvider::Ollama),
            _ => self.effective_model(),
        };
        let chat = ChatConfig {
            provider: self.provider.into(),
            model: model.to_owned(),
            deepseek_model: self.effective_model_for(ChatProvider::DeepSeek).to_owned(),
            // Secrets only ever come from the environment.
            deepseek_api_key: None,
            params: GenerationParams {
                temperature: Some(self.temperature),
                top_p: None,
                max_tokens: Some(self.max_tokens),
                seed: None,
            },
            context_tokens: self.context_tokens,
            system_prompt_path: None,
            ..ChatConfig::default()
        };

        let embed = EmbedConfig {
            provider: EmbeddingProvider::Ollama,
            model: self.embed_model.trim().to_owned(),
            top_k: self.top_k,
            similarity_threshold: self.similarity_threshold,
            ..EmbedConfig::default()
        };

        let parse = ParseConfig {
            pdf_parser: match self.pdf_backend {
                PdfBackend::Extract => PdfParserBackend::Extract,
                PdfBackend::Unpdf => PdfParserBackend::Unpdf,
                PdfBackend::Sink => PdfParserBackend::Sink,
                PdfBackend::Internal => PdfParserBackend::Internal,
            },
            sloppy_pdf: self.sloppy_pdf,
            chunk_size: self.chunk_size,
            chunk_overlap: self.chunk_overlap,
        };

        let memory = MemoryConfig {
            model: self.memory_model.trim().to_owned(),
            rewrite_prompt_path: None,
            strategy: match self.memory {
                MemoryChoice::Rewrite => MemoryStrategyKind::Rewrite,
                MemoryChoice::Transcript => MemoryStrategyKind::Transcript,
                MemoryChoice::Log => MemoryStrategyKind::Log,
                MemoryChoice::Summary => MemoryStrategyKind::Summary,
                MemoryChoice::Off => MemoryStrategyKind::Off,
            },
        };

        Ok(RagrigConfig {
            workspace,
            chat,
            embed,
            parse,
            memory,
            corpus_dirs: self
                .corpus_dirs
                .iter()
                .map(|entry| entry.trim().to_owned())
                .filter(|entry| !entry.is_empty())
                .collect(),
            corpus_urls: Vec::new(),
            semantic_scholar_api_key: None,
        })
    }

    /// The custom prompt file, resolved against the workspace.
    pub fn prompt_path(&self) -> Option<PathBuf> {
        let path = self.system_prompt_path.trim();
        if path.is_empty() {
            return None;
        }
        let path = Path::new(path);
        Some(if path.is_absolute() {
            path.to_path_buf()
        } else {
            Path::new(self.workspace.trim()).join(path)
        })
    }

    /// The prompt in effect: the selected file, or the inline prompt.
    pub fn effective_prompt(&self) -> Result<String, String> {
        match self.prompt_path() {
            Some(path) => {
                let text = fs::read_to_string(&path).map_err(|error| {
                    format!("cannot read system prompt '{}': {error}", path.display())
                })?;
                if text.trim().is_empty() {
                    return Err(format!("system prompt '{}' is empty", path.display()));
                }
                Ok(text)
            }
            None => Ok(self.system_prompt.clone()),
        }
    }

    /// Clear the prompt file and restore the host's compiled-in prompt.
    pub fn use_built_in_prompt(&mut self) {
        self.system_prompt_path.clear();
        self.system_prompt = self.built_in_prompt.clone();
    }

    /// Validate and produce a build setup, including the prompt override.
    pub fn to_setup(&self) -> Result<RagSetup, String> {
        let config = self.to_config()?;
        let prompt = self.effective_prompt()?;
        let prompt = prompt.trim();
        let mut setup = if prompt.is_empty() {
            RagSetup::new(config)
        } else {
            RagSetup::with_system_prompt(config, prompt)
        };
        setup.stream = self.stream;
        setup.resolve_citations = self.resolve_citations;
        setup.hybrid = self.hybrid;
        if !self.bibliography.trim().is_empty() {
            setup.bibliography = Some(PathBuf::from(self.bibliography.trim()));
        }
        Ok(setup)
    }

    /// Check the editable values without touching the filesystem.
    pub fn validate(&self) -> Result<(), String> {
        if self.workspace.trim().is_empty() {
            return Err("choose a workspace folder".to_owned());
        }
        if self.model().trim().is_empty() {
            return Err(format!("choose a {} chat model", self.provider.label()));
        }
        // API keys are read from the environment when the worker builds the
        // agent. A missing key must not block editing settings or saving a
        // profile; the worker reports it through `AgentFailed`.
        if !self.temperature.is_finite() || !(0.0..=2.0).contains(&self.temperature) {
            return Err("temperature must be between 0 and 2".to_owned());
        }
        if self.max_tokens == 0 {
            return Err("max tokens must be at least 1".to_owned());
        }
        if self.context_tokens < 256 {
            return Err("context tokens must be at least 256".to_owned());
        }
        if self.hybrid {
            if self.embed_model.trim().is_empty() {
                return Err("choose an embedding model".to_owned());
            }
            if self.top_k == 0 {
                return Err("top-k must be at least 1".to_owned());
            }
            if !self.similarity_threshold.is_finite()
                || !(0.0..=1.0).contains(&self.similarity_threshold)
            {
                return Err("similarity threshold must be between 0 and 1".to_owned());
            }
        }
        if self.chunk_size == 0 {
            return Err("chunk size must be at least 1".to_owned());
        }
        if self.chunk_overlap >= self.chunk_size {
            return Err("chunk overlap must be smaller than the chunk size".to_owned());
        }
        if self.memory.uses_model() && self.memory_model.trim().is_empty() {
            return Err("choose a memory model or turn memory off".to_owned());
        }
        if self.grobid_url.trim().is_empty() {
            return Err("enter the GROBID server URL".to_owned());
        }
        if self.grobid_source == GrobidSource::Managed && self.grobid_image.trim().is_empty() {
            return Err("enter the Docker image for the managed GROBID container".to_owned());
        }
        for entry in &self.corpus_dirs {
            let entry = entry.trim();
            if entry.is_empty() {
                continue;
            }
            let Some((name, path)) = entry.split_once('=') else {
                return Err(format!("corpus '{entry}' must use name=path"));
            };
            if name.trim().is_empty() || path.trim().is_empty() {
                return Err(format!("corpus '{entry}' needs a name and a path"));
            }
        }
        Ok(())
    }

    /// Re-read the profile list from the workspace and `~/.ragrig`.
    ///
    /// Workspace profiles shadow global profiles with the same name.
    pub fn refresh_profiles(&mut self) {
        let workspace = Path::new(self.workspace.trim());
        let mut workspace_failed = None;
        let mut profiles = match RagrigConfig::list_profiles(workspace) {
            Ok(profiles) => profiles,
            Err(error) => {
                workspace_failed = Some(error);
                Vec::new()
            }
        };
        if let Some(home) = home_ragrig_dir() {
            for name in RagrigConfig::list_profiles(&home).unwrap_or_default() {
                if !profiles.contains(&name) {
                    profiles.push(name);
                }
            }
        }
        profiles.sort();
        if let Some(error) = workspace_failed {
            self.status = Some(format!("cannot list workspace profiles: {error:#}"));
        }
        self.profiles = profiles;
        if !self.profile_name.trim().is_empty()
            && !self
                .profiles
                .iter()
                .any(|name| name == self.profile_name.trim())
        {
            self.profiles.insert(0, self.profile_name.trim().to_owned());
        }
    }

    /// Save these settings as a named profile in the workspace.
    pub fn save_profile(&mut self) -> Result<(), String> {
        let name = self.profile_name.trim().to_owned();
        validate_profile_name(&name)?;
        let mut config = self.to_config()?;
        let workspace = Path::new(self.workspace.trim());
        let prompt = self.effective_prompt()?;
        let prompt = prompt.trim();
        if prompt.is_empty()
            || (self.system_prompt_path.trim().is_empty() && prompt == self.built_in_prompt.trim())
        {
            config.chat.system_prompt_path = None;
        } else {
            let dir = RagrigConfig::profiles_dir(workspace);
            fs::create_dir_all(&dir)
                .map_err(|error| format!("cannot create profiles folder: {error}"))?;
            let relative = PathBuf::from(format!(".ragrig/profiles/{name}.prompt.md"));
            fs::write(workspace.join(&relative), prompt).map_err(|error| {
                format!("cannot write the prompt for profile '{name}': {error}")
            })?;
            config.chat.system_prompt_path = Some(relative);
        }
        config
            .save_to_profile(workspace, &name)
            .map_err(|error| format!("cannot save profile '{name}': {error:#}"))?;
        self.refresh_profiles();
        self.status = Some(format!("Saved profile '{name}'"));
        Ok(())
    }

    /// Load a named profile from the workspace, then from `~/.ragrig`.
    pub fn load_profile(&mut self, name: &str) -> Result<(), String> {
        validate_profile_name(name)?;
        let workspace = Path::new(self.workspace.trim());
        let mut config = match RagrigConfig::load_from_profile(workspace, name) {
            Ok(config) => config,
            Err(workspace_error) => match home_ragrig_dir() {
                Some(home) => RagrigConfig::load_from_profile(&home, name)
                    .map_err(|_| format!("cannot load profile '{name}': {workspace_error:#}"))?,
                None => {
                    return Err(format!("cannot load profile '{name}': {workspace_error:#}"));
                }
            },
        };
        // A global profile resolves relative prompt paths against the actual
        // workspace, never against the home directory.
        config.workspace = workspace.to_path_buf();
        let profiles = self.profiles.clone();
        let built_in_prompt = self.built_in_prompt.clone();
        // Hybrid retrieval and the service endpoint are runtime choices, not
        // part of a profile, so keep them across profile switches.
        let hybrid = self.hybrid;
        let grobid_source = self.grobid_source;
        let grobid_url = self.grobid_url.clone();
        let grobid_image = self.grobid_image.clone();
        let grobid_auto_start = self.grobid_auto_start;
        let mut loaded = Self::from_config(&config);
        loaded.hybrid = hybrid;
        loaded.grobid_source = grobid_source;
        loaded.grobid_url = grobid_url;
        loaded.grobid_image = grobid_image;
        loaded.grobid_auto_start = grobid_auto_start;
        loaded.profiles = profiles;
        loaded.profile_name = name.to_owned();
        loaded.built_in_prompt = built_in_prompt.clone();
        if loaded.system_prompt_path.is_empty() {
            loaded.system_prompt = built_in_prompt;
        }
        loaded.status = Some(format!("Loaded profile '{name}' — press Apply"));
        *self = loaded;
        self.refresh_profiles();
        Ok(())
    }

    /// Remember a folder corpus so later applies keep it.
    pub fn add_corpus(&mut self, name: &str, folder: &Path) {
        let entry = format!("{name}={}", folder.display());
        self.corpus_dirs.retain(|existing| {
            existing
                .split_once('=')
                .is_none_or(|(existing_name, _)| existing_name != name)
        });
        self.corpus_dirs.push(entry);
    }
}

fn default_workspace() -> PathBuf {
    std::env::var_os("RAGRIG_WORKSPACE")
        .map(PathBuf::from)
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."))
}

/// The user-wide Ragrig directory.
///
/// `RAGRIG_HOME` overrides the default `~/.ragrig` (useful for tests and for
/// keeping one profile library on a shared drive). Profiles here are visible
/// from every workspace; a workspace profile with the same name wins. A
/// `config.json` in this directory is the global layer between the built-in
/// defaults and a workspace's `.ragrig/config.json`.
pub fn home_ragrig_dir() -> Option<PathBuf> {
    std::env::var_os("RAGRIG_HOME")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|home| home.join(".ragrig")))
}

/// Merge `overlay` into `base` per key; objects recurse, everything else
/// replaces.
fn merge_json(base: &mut serde_json::Value, overlay: serde_json::Value) {
    match (base, overlay) {
        (serde_json::Value::Object(base), serde_json::Value::Object(overlay)) => {
            for (key, value) in overlay {
                match base.get_mut(&key) {
                    Some(existing) => merge_json(existing, value),
                    None => {
                        base.insert(key, value);
                    }
                }
            }
        }
        (base, overlay) => *base = overlay,
    }
}

fn default_prompt() -> String {
    "Answer from the attached context and the indexed corpus. Cite each paper by its @Key. \
     If the context does not support an answer, say what is missing instead of guessing."
        .to_owned()
}

fn validate_profile_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("enter a profile name first".to_owned());
    }
    if !name
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || character == '-' || character == '_')
    {
        return Err("profile names may use letters, digits, '-' and '_' only".to_owned());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> RagSettings {
        RagSettings {
            workspace: std::env::temp_dir().display().to_string(),
            ..RagSettings::default()
        }
    }

    #[test]
    fn defaults_convert_to_a_valid_setup() {
        let setup = fixture().to_setup().expect("valid defaults");
        assert_eq!(setup.config.parse.chunk_size, 1024);
        assert_eq!(setup.config.parse.chunk_overlap, 128);
        assert!(setup.system_prompt.is_some());
        assert!(setup.resolve_citations);
        assert!(setup.hybrid);
        assert!(setup.memory_enabled());

        let mut settings = fixture();
        settings.resolve_citations = false;
        assert!(!settings.to_setup().expect("valid setup").resolve_citations);
    }

    #[test]
    fn every_provider_has_a_default_model() {
        let chat = ChatConfig::default();
        assert_eq!(ChatProvider::Ollama.default_model(), chat.model);
        assert_eq!(ChatProvider::DeepSeek.default_model(), chat.deepseek_model);
        for (provider, _) in ChatProvider::options() {
            assert!(
                !provider.default_model().trim().is_empty(),
                "{provider:?} needs a default model"
            );
        }
    }

    #[test]
    fn cloud_defaults_match_the_upstream_specs() {
        use ragrig::ChatAgentSpec;

        for (backend, provider) in [
            ("openai", ChatProvider::OpenAi),
            ("claude", ChatProvider::Claude),
            ("mistral", ChatProvider::Mistral),
            ("openrouter", ChatProvider::OpenRouter),
        ] {
            let spec = ChatAgentSpec::parse(backend, None, None, None).expect("known backend");
            let model = match spec {
                ChatAgentSpec::OpenAi { model, .. }
                | ChatAgentSpec::Claude { model, .. }
                | ChatAgentSpec::Mistral { model, .. }
                | ChatAgentSpec::OpenRouter { model, .. } => model,
                _ => panic!("unexpected spec for {backend}"),
            };
            assert_eq!(provider.default_model(), model, "{backend} default drifted");
        }
    }

    #[test]
    fn each_provider_keeps_its_own_model() {
        let mut settings = fixture();
        settings.set_model_for(ChatProvider::Ollama, "llama3");
        settings.set_model_for(ChatProvider::OpenAi, "gpt-4o-mini");

        settings.provider = ChatProvider::OpenAi;
        assert_eq!(settings.model(), "gpt-4o-mini");

        settings.provider = ChatProvider::Claude;
        assert_eq!(settings.model(), ChatProvider::Claude.default_model());

        settings.provider = ChatProvider::Ollama;
        assert_eq!(settings.model(), "llama3");
    }

    #[test]
    fn cloud_providers_map_to_upstream_backends_and_key_variables() {
        let providers = [
            (
                ChatProvider::DeepSeek,
                Provider::Deepseek,
                "DEEPSEEK_API_KEY",
            ),
            (ChatProvider::OpenAi, Provider::OpenAi, "OPENAI_API_KEY"),
            (ChatProvider::Claude, Provider::Claude, "ANTHROPIC_API_KEY"),
            (ChatProvider::Mistral, Provider::Mistral, "MISTRAL_API_KEY"),
            (
                ChatProvider::OpenRouter,
                Provider::OpenRouter,
                "OPENROUTER_API_KEY",
            ),
        ];
        for (provider, backend, key) in providers {
            assert_eq!(Provider::from(provider), backend);
            assert_eq!(ChatProvider::from(backend), provider);
            assert_eq!(provider.env_var(), Some(key));
            assert_eq!(ChatProvider::from_label(provider.label()), Some(provider));
        }
        assert_eq!(Provider::from(ChatProvider::Ollama), Provider::Ollama);
        assert_eq!(ChatProvider::Ollama.env_var(), None);
    }

    #[test]
    fn a_missing_api_key_does_not_block_the_settings_form() {
        let mut settings = fixture();
        // The worker reports a missing key when it builds the agent. The form
        // must still validate and convert so Apply and profile saves work.
        for provider in [
            ChatProvider::DeepSeek,
            ChatProvider::OpenAi,
            ChatProvider::Claude,
            ChatProvider::Mistral,
            ChatProvider::OpenRouter,
        ] {
            settings.provider = provider;
            settings.set_model(provider.default_model());
            assert!(
                settings.to_config().is_ok(),
                "{provider:?} must validate without its key"
            );
        }

        let workspace = unique_temp_dir("ragrig-gpui-cloud-profile");
        settings.workspace = workspace.display().to_string();
        settings.profile_name = "cloud".to_owned();
        settings
            .save_profile()
            .expect("a profile saves without a key in the environment");
        assert!(workspace.join(".ragrig/profiles/cloud.json").is_file());

        let _ = std::fs::remove_dir_all(workspace);
    }

    #[test]
    fn the_selected_model_reaches_the_chat_config() {
        let mut settings = fixture();
        settings.set_model_for(ChatProvider::Ollama, "llama3");
        let config = settings.to_config().expect("valid configuration");
        assert_eq!(config.chat.provider, Provider::Ollama);
        assert_eq!(config.chat.model, "llama3");

        let mut config = RagrigConfig::default();
        config.chat.provider = Provider::OpenAi;
        config.chat.model = "gpt-4o-mini".to_owned();
        let settings = RagSettings::from_config(&config);
        assert_eq!(settings.provider, ChatProvider::OpenAi);
        assert_eq!(settings.model(), "gpt-4o-mini");
        assert_eq!(
            settings.model_for(ChatProvider::Ollama),
            ChatProvider::Ollama.default_model(),
            "inactive providers keep their own default"
        );

        let mut config = RagrigConfig::default();
        config.chat.provider = Provider::OpenAi;
        let settings = RagSettings::from_config(&config);
        assert_eq!(
            settings.model(),
            ChatProvider::OpenAi.default_model(),
            "the Ollama default means the selected provider's default"
        );

        let mut config = RagrigConfig::default();
        config.chat.provider = Provider::Deepseek;
        config.chat.model = "llama3".to_owned();
        config.chat.deepseek_model = "deepseek-chat".to_owned();
        let settings = RagSettings::from_config(&config);
        assert_eq!(settings.model_for(ChatProvider::Ollama), "llama3");
        assert_eq!(settings.model_for(ChatProvider::DeepSeek), "deepseek-chat");
    }

    #[test]
    fn memory_and_retrieval_switches_reach_the_setup() {
        let mut settings = fixture();
        settings.memory = MemoryChoice::Off;
        settings.hybrid = false;

        let setup = settings.to_setup().expect("valid setup");
        assert!(!setup.memory_enabled());
        assert!(!setup.hybrid);
    }

    #[test]
    fn hybrid_off_does_not_require_an_embedding_model() {
        let mut settings = fixture();
        settings.hybrid = false;
        settings.embed_model.clear();

        assert!(settings.to_setup().is_ok());

        settings.hybrid = true;
        assert!(settings.to_setup().is_err());
    }

    #[test]
    fn layered_config_merges_one_setting_at_a_time() {
        let root = unique_temp_dir("ragrig-gpui-layers");
        let global = root.join("home/.ragrig/config.json");
        let workspace = root.join("paper/.ragrig/config.json");
        std::fs::create_dir_all(global.parent().expect("global parent")).expect("global dir");
        std::fs::create_dir_all(workspace.parent().expect("workspace parent"))
            .expect("workspace dir");
        std::fs::write(
            &global,
            r#"{"chat":{"model":"global-chat"},"embed":{"top_k":12},"parse":{"chunk_size":512}}"#,
        )
        .expect("global layer");
        std::fs::write(
            &workspace,
            r#"{"chat":{"model":"local-chat"},"hybrid":false,"parse":{"chunk_overlap":32}}"#,
        )
        .expect("workspace layer");

        let settings = RagSettings::from_layer_paths(Some(&global), &workspace, None)
            .expect("layered settings");
        assert_eq!(
            settings.model_for(ChatProvider::Ollama),
            "local-chat",
            "workspace wins"
        );
        assert_eq!(settings.top_k, 12, "global wins over built-in");
        assert_eq!(
            settings.chunk_size, 512,
            "global survives the workspace layer"
        );
        assert_eq!(settings.chunk_overlap, 32, "workspace fills a missing key");
        assert!(!settings.hybrid, "the workspace hybrid extension is read");
        assert_eq!(
            settings.provider,
            RagSettings::default().provider,
            "untouched keys keep the built-in default"
        );

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn runtime_switches_survive_a_layer_without_them() {
        let root = unique_temp_dir("ragrig-gpui-layer-switches");
        let global = root.join("home/.ragrig/config.json");
        let workspace = root.join("paper/.ragrig/config.json");
        std::fs::create_dir_all(global.parent().expect("global parent")).expect("global dir");
        std::fs::create_dir_all(workspace.parent().expect("workspace parent"))
            .expect("workspace dir");
        std::fs::write(&global, r#"{"chat":{"model":"layer-chat"}}"#).expect("layer");
        std::fs::write(&workspace, r#"{"parse":{"chunk_size":256}}"#).expect("layer");

        let mut base = fixture();
        base.memory = MemoryChoice::Off;
        base.hybrid = false;
        let settings = RagSettings::from_layer_paths(Some(&global), &workspace, Some(&base))
            .expect("layered settings");
        assert_eq!(settings.model_for(ChatProvider::Ollama), "layer-chat");
        assert_eq!(settings.chunk_size, 256);
        assert_eq!(
            settings.memory,
            MemoryChoice::Off,
            "runtime memory survives"
        );
        assert!(!settings.hybrid, "runtime retrieval switch survives");

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_prompt_file_overrides_the_inline_prompt() {
        let workspace = unique_temp_dir("ragrig-gpui-prompt-file");
        std::fs::write(workspace.join("Chat.md"), "# Chat\n\nUse the corpus.")
            .expect("prompt file");
        let mut settings = fixture();
        settings.workspace = workspace.display().to_string();
        settings.system_prompt = "inline".to_owned();
        settings.system_prompt_path = "Chat.md".to_owned();

        assert_eq!(
            settings.to_setup().expect("setup").system_prompt.as_deref(),
            Some("# Chat\n\nUse the corpus.")
        );
        let applied = RagSettings {
            system_prompt_path: String::new(),
            system_prompt: "inline".to_owned(),
            ..settings.clone()
        };
        assert!(
            settings
                .dirty_since(&applied)
                .contains(RagField::SystemPrompt)
        );

        let _ = std::fs::remove_dir_all(workspace);
    }

    #[test]
    fn a_missing_prompt_file_is_reported() {
        let mut settings = fixture();
        settings.system_prompt_path = "/definitely/not/here/Chat.md".to_owned();
        let error = settings.to_setup().expect_err("missing prompt");
        assert!(error.contains("cannot read system prompt"), "{error}");
    }

    #[test]
    fn using_the_built_in_prompt_clears_the_file() {
        let mut settings = fixture();
        settings.built_in_prompt = "Host prompt".to_owned();
        settings.system_prompt = "File prompt".to_owned();
        settings.system_prompt_path = "Chat.md".to_owned();

        settings.use_built_in_prompt();

        assert_eq!(settings.system_prompt, "Host prompt");
        assert!(settings.system_prompt_path.is_empty());
    }

    #[test]
    fn profile_defaults_restore_the_built_in_prompt() {
        let mut settings = fixture();
        settings.built_in_prompt = "Host prompt".to_owned();
        settings.system_prompt = "Custom".to_owned();
        settings.system_prompt_path = "custom.md".to_owned();

        settings.reset_to_profile_defaults().expect("reset");

        assert_eq!(settings.system_prompt, "Host prompt");
        assert!(settings.system_prompt_path.is_empty());
    }

    #[test]
    fn a_configured_bibliography_reaches_the_setup() {
        let mut settings = fixture();
        assert!(
            settings
                .to_setup()
                .expect("valid defaults")
                .bibliography
                .is_none()
        );

        settings.bibliography = "  /tmp/library.bib ".to_owned();
        assert_eq!(
            settings.to_setup().expect("valid setup").bibliography,
            Some(std::path::PathBuf::from("/tmp/library.bib"))
        );
    }

    #[test]
    fn invalid_values_are_rejected() {
        let mut settings = fixture();
        settings.chunk_overlap = settings.chunk_size;
        assert_eq!(
            settings.validate(),
            Err("chunk overlap must be smaller than the chunk size".to_owned())
        );

        settings = fixture();
        settings.top_k = 0;
        assert_eq!(
            settings.validate(),
            Err("top-k must be at least 1".to_owned())
        );

        settings = fixture();
        settings.similarity_threshold = 1.5;
        assert_eq!(
            settings.validate(),
            Err("similarity threshold must be between 0 and 1".to_owned())
        );

        settings = fixture();
        settings.corpus_dirs = vec!["broken".to_owned()];
        assert_eq!(
            settings.validate(),
            Err("corpus 'broken' must use name=path".to_owned())
        );
    }

    #[test]
    fn profiles_round_trip_prompt_and_api_key_slot() {
        let workspace = unique_temp_dir("ragrig-gpui-profile");
        let mut settings = fixture();
        settings.workspace = workspace.display().to_string();
        settings.set_model("qwen3.5:9b");
        settings.system_prompt = "Cite primary sources only.".to_owned();
        settings.profile_name = "lab".to_owned();
        settings.status = None;

        settings.save_profile().expect("save profile");
        let path = workspace.join(".ragrig/profiles/lab.json");
        let json = std::fs::read_to_string(&path).expect("profile file");
        assert!(!json.contains("sk-"));
        assert!(json.contains("\"deepseek_api_key\": null"));
        assert!(json.contains("lab.prompt.md"));
        assert_eq!(
            std::fs::read_to_string(workspace.join(".ragrig/profiles/lab.prompt.md"))
                .expect("prompt file"),
            "Cite primary sources only."
        );

        settings.workspace = workspace.display().to_string();
        settings.refresh_profiles();
        settings.profile_name = "lab".to_owned();
        settings.system_prompt = "overwritten".to_owned();
        settings.load_profile("lab").expect("load profile");
        assert_eq!(settings.model_for(ChatProvider::Ollama), "qwen3.5:9b");
        assert_eq!(settings.system_prompt, "Cite primary sources only.");

        settings.profile_name = "../escape".to_owned();
        assert!(settings.save_profile().is_err());

        let _ = std::fs::remove_dir_all(workspace);
    }

    #[test]
    fn adding_a_corpus_replaces_the_same_name() {
        let mut settings = fixture();
        settings.add_corpus("papers", Path::new("/docs/a"));
        settings.add_corpus("papers", Path::new("/docs/b"));
        assert_eq!(settings.corpus_dirs, vec!["papers=/docs/b".to_owned()]);
    }

    #[test]
    fn grobid_defaults_to_an_existing_server_and_the_full_image() {
        let settings = RagSettings::default();
        assert_eq!(settings.grobid_source, GrobidSource::Existing);
        assert_eq!(settings.grobid_url, DEFAULT_GROBID_URL.to_owned());
        assert_eq!(settings.grobid_image, DEFAULT_GROBID_IMAGE.to_owned());
        assert!(!settings.grobid_auto_start);
        assert_eq!(
            GrobidSource::from_label("Docker container"),
            Some(GrobidSource::Managed)
        );
        assert_eq!(GrobidSource::from_label("not a source"), None);
    }

    #[test]
    fn changes_name_changed_fields_and_their_sections() {
        let applied = fixture();
        let mut settings = applied.clone();
        assert!(settings.dirty_since(&applied).is_empty());

        settings.max_tokens = applied.max_tokens * 2;
        settings.chunk_size = applied.chunk_size * 2;
        settings.openalex = !applied.openalex;
        let changes = settings.dirty_since(&applied);

        assert!(changes.contains(RagField::MaxTokens));
        assert!(changes.contains(RagField::ChunkSize));
        assert!(changes.contains(RagField::OpenAlex));
        assert!(!changes.contains(RagField::Provider));
        assert_eq!(changes.len(), 3);
        assert!(changes.section(RagSection::Chat));
        assert!(changes.section(RagSection::Parsing));
        assert!(changes.section(RagSection::References));
        assert!(!changes.section(RagSection::Memory));
        assert_eq!(changes.count_in(RagSection::Chat), 1);
        assert_eq!(changes.count_in(RagSection::Parsing), 1);
        assert_eq!(changes.count_in(RagSection::Memory), 0);
    }

    #[test]
    fn every_editable_field_is_tracked() {
        type FieldMutation = (RagField, fn(&mut RagSettings));
        let applied = fixture();
        let mutators: &[FieldMutation] = &[
            (RagField::Workspace, |settings| {
                settings.workspace.push_str("/elsewhere")
            }),
            (RagField::Bibliography, |settings| {
                settings.bibliography.push_str("/other.bib")
            }),
            (RagField::Provider, |settings| {
                settings.provider = ChatProvider::DeepSeek
            }),
            (RagField::Model, |settings| {
                let model = settings.model().to_owned();
                settings.set_model(format!("{model}-alt"));
            }),
            (RagField::Temperature, |settings| {
                settings.temperature += 0.1
            }),
            (RagField::MaxTokens, |settings| settings.max_tokens *= 2),
            (RagField::ContextTokens, |settings| {
                settings.context_tokens *= 2
            }),
            (RagField::Stream, |settings| {
                settings.stream = !settings.stream
            }),
            (RagField::SystemPrompt, |settings| {
                settings.system_prompt.push_str(" Be terse.")
            }),
            (RagField::EmbedModel, |settings| {
                settings.embed_model.push_str("-alt")
            }),
            (RagField::Hybrid, |settings| {
                settings.hybrid = !settings.hybrid
            }),
            (RagField::TopK, |settings| settings.top_k += 1),
            (RagField::SimilarityThreshold, |settings| {
                settings.similarity_threshold += 0.01
            }),
            (RagField::CorpusDirs, |settings| {
                settings.corpus_dirs.push("lab=/docs/lab".to_owned())
            }),
            (RagField::PdfBackend, |settings| {
                settings.pdf_backend = PdfBackend::Sink
            }),
            (RagField::SloppyPdf, |settings| {
                settings.sloppy_pdf = !settings.sloppy_pdf
            }),
            (RagField::ChunkSize, |settings| settings.chunk_size += 16),
            (RagField::ChunkOverlap, |settings| {
                settings.chunk_overlap += 8
            }),
            (RagField::Memory, |settings| {
                settings.memory = MemoryChoice::Off
            }),
            (RagField::MemoryModel, |settings| {
                settings.memory_model.push_str("-alt")
            }),
            (RagField::GrobidSource, |settings| {
                settings.grobid_source = GrobidSource::Managed
            }),
            (RagField::GrobidUrl, |settings| {
                settings.grobid_url.push_str("/v2")
            }),
            (RagField::GrobidImage, |settings| {
                settings.grobid_image.push_str("-crf")
            }),
            (RagField::GrobidAutoStart, |settings| {
                settings.grobid_auto_start = !settings.grobid_auto_start
            }),
            (RagField::RenamePdfs, |settings| {
                settings.rename_pdfs = !settings.rename_pdfs
            }),
            (RagField::OpenAlex, |settings| {
                settings.openalex = !settings.openalex
            }),
            (RagField::ResolveCitations, |settings| {
                settings.resolve_citations = !settings.resolve_citations
            }),
        ];

        for (field, mutate) in mutators {
            let mut settings = applied.clone();
            mutate(&mut settings);
            let changes = settings.dirty_since(&applied);
            assert!(
                changes.contains(*field),
                "{field:?} is not part of the change set"
            );
            assert_eq!(changes.len(), 1, "{field:?} changed more than one field");
            assert!(
                changes.section(field.section()),
                "{field:?} did not mark its section"
            );
        }
    }

    #[test]
    fn profile_bookkeeping_is_not_a_change() {
        let applied = fixture();
        let mut settings = applied.clone();
        settings.profile_name = "lab".to_owned();
        settings.profiles = vec!["lab".to_owned()];
        settings.status = Some("Saved profile 'lab'".to_owned());

        assert!(settings.dirty_since(&applied).is_empty());
    }

    #[test]
    fn profile_defaults_reload_the_selected_profile() {
        let workspace = unique_temp_dir("ragrig-gpui-defaults");
        let mut settings = fixture();
        settings.workspace = workspace.display().to_string();
        settings.profile_name = "lab".to_owned();
        settings.set_model("qwen3.5:9b");
        settings.top_k = 42;
        settings.save_profile().expect("save profile");

        settings.set_model("scratch");
        settings.top_k = 3;
        settings.reset_to_profile_defaults().expect("reset");

        assert_eq!(settings.model_for(ChatProvider::Ollama), "qwen3.5:9b");
        assert_eq!(settings.top_k, 42);
        assert_eq!(settings.workspace, workspace.display().to_string());

        let _ = std::fs::remove_dir_all(workspace);
    }

    #[test]
    fn profile_defaults_without_a_profile_keep_the_library_location() {
        let mut settings = fixture();
        settings.workspace = "/tmp/scico-workspace".to_owned();
        settings.bibliography = "/tmp/scico.bib".to_owned();
        settings.profile_name.clear();
        settings.top_k = 3;
        settings.temperature = 1.7;

        settings.reset_to_profile_defaults().expect("reset");

        let defaults = RagSettings::default();
        assert_eq!(settings.top_k, defaults.top_k);
        assert_eq!(settings.temperature, defaults.temperature);
        assert_eq!(settings.workspace, "/tmp/scico-workspace");
        assert_eq!(settings.bibliography, "/tmp/scico.bib");
    }

    fn unique_temp_dir(prefix: &str) -> PathBuf {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let counter = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("{prefix}-{}-{counter}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }
}
