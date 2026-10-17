//! Editable Ragrig configuration for the settings page and profiles.

use std::{
    fs,
    path::{Path, PathBuf},
};

use ragrig::{
    ChatConfig, EmbedConfig, GenerationParams, MemoryConfig, MemoryStrategyKind, ParseConfig,
    PdfParserBackend, RagrigConfig,
    types::{EmbeddingProvider, Provider},
};

/// Chat backend offered by the settings page.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ChatProvider {
    /// Local Ollama server.
    #[default]
    Ollama,
    /// DeepSeek cloud API; the key comes from `DEEPSEEK_API_KEY`.
    DeepSeek,
}

impl ChatProvider {
    /// Human-readable name.
    pub fn label(self) -> &'static str {
        match self {
            Self::Ollama => "Ollama",
            Self::DeepSeek => "DeepSeek",
        }
    }

    /// Options for a dropdown.
    pub fn options() -> &'static [(Self, &'static str)] {
        &[(Self::Ollama, "Ollama"), (Self::DeepSeek, "DeepSeek")]
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

/// Everything needed to build one [`ragrig::RagAgent`].
#[derive(Clone, Debug)]
pub struct RagSetup {
    /// Ragrig's own configuration tree.
    pub config: RagrigConfig,
    /// Prompt text applied after building; takes precedence over
    /// `config.chat.system_prompt_path`.
    pub system_prompt: Option<String>,
    /// Whether answers stream token by token.
    pub stream: bool,
}

impl RagSetup {
    /// Wrap a config without a prompt override.
    pub fn new(config: RagrigConfig) -> Self {
        Self {
            config,
            system_prompt: None,
            stream: true,
        }
    }

    /// Wrap a config and apply `prompt` before the agent is built.
    pub fn with_system_prompt(config: RagrigConfig, prompt: impl Into<String>) -> Self {
        Self {
            config,
            system_prompt: Some(prompt.into()),
            stream: true,
        }
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
/// this struct: the DeepSeek key is read from the environment by the worker.
#[derive(Clone, Debug, PartialEq)]
pub struct RagSettings {
    /// Workspace folder holding the store and sessions.
    pub workspace: String,
    /// Active chat backend.
    pub provider: ChatProvider,
    /// Ollama chat model.
    pub ollama_model: String,
    /// DeepSeek chat model.
    pub deepseek_model: String,
    /// Sampling temperature.
    pub temperature: f64,
    /// Maximum generated tokens.
    pub max_tokens: usize,
    /// Chat context window in tokens.
    pub context_tokens: usize,
    /// System prompt sent before the conversation.
    pub system_prompt: String,
    /// Stream tokens into the transcript.
    pub stream: bool,
    /// Ollama embedding model.
    pub embed_model: String,
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
    /// Base URL of the GROBID server.
    pub grobid_url: String,
    /// Rename PDFs to their metadata name during reference extraction.
    pub rename_pdfs: bool,
    /// Complete extracted references against OpenAlex.
    pub openalex: bool,
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

impl RagSettings {
    /// Read the editable fields from a Ragrig configuration.
    pub fn from_config(config: &RagrigConfig) -> Self {
        let provider = match config.chat.provider {
            Provider::Deepseek => ChatProvider::DeepSeek,
            _ => ChatProvider::Ollama,
        };
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
        let system_prompt = config
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
            .and_then(|path| fs::read_to_string(path).ok())
            .filter(|text| !text.trim().is_empty())
            .unwrap_or_else(default_prompt);

        Self {
            workspace: config.workspace.to_string_lossy().into_owned(),
            provider,
            ollama_model: config.chat.model.clone(),
            deepseek_model: config.chat.deepseek_model.clone(),
            temperature: config.chat.params.temperature.unwrap_or(0.2),
            max_tokens: config.chat.params.max_tokens.unwrap_or(2048),
            context_tokens: config.chat.context_tokens,
            system_prompt,
            stream: true,
            embed_model: config.embed.model.clone(),
            top_k: config.embed.top_k,
            similarity_threshold: config.embed.similarity_threshold,
            chunk_size: config.parse.chunk_size,
            chunk_overlap: config.parse.chunk_overlap,
            pdf_backend,
            sloppy_pdf: config.parse.sloppy_pdf,
            memory,
            memory_model: config.memory.model.clone(),
            grobid_url: "http://localhost:8070".to_owned(),
            rename_pdfs: false,
            openalex: false,
            corpus_dirs: config.corpus_dirs.clone(),
            profile_name: String::new(),
            profiles: Vec::new(),
            status: None,
        }
    }

    /// Validate and convert to a Ragrig configuration.
    pub fn to_config(&self) -> Result<RagrigConfig, String> {
        self.validate()?;

        let workspace = PathBuf::from(self.workspace.trim());
        let chat = ChatConfig {
            provider: match self.provider {
                ChatProvider::Ollama => Provider::Ollama,
                ChatProvider::DeepSeek => Provider::Deepseek,
            },
            model: self.ollama_model.trim().to_owned(),
            deepseek_model: self.deepseek_model.trim().to_owned(),
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

    /// Validate and produce a build setup, including the prompt override.
    pub fn to_setup(&self) -> Result<RagSetup, String> {
        let config = self.to_config()?;
        let prompt = self.system_prompt.trim();
        let mut setup = if prompt.is_empty() {
            RagSetup::new(config)
        } else {
            RagSetup::with_system_prompt(config, prompt)
        };
        setup.stream = self.stream;
        Ok(setup)
    }

    /// Check the editable values without touching the filesystem.
    pub fn validate(&self) -> Result<(), String> {
        if self.workspace.trim().is_empty() {
            return Err("choose a workspace folder".to_owned());
        }
        match self.provider {
            ChatProvider::Ollama => {
                if self.ollama_model.trim().is_empty() {
                    return Err("choose an Ollama chat model".to_owned());
                }
            }
            ChatProvider::DeepSeek => {
                if self.deepseek_model.trim().is_empty() {
                    return Err("choose a DeepSeek chat model".to_owned());
                }
                if std::env::var_os("DEEPSEEK_API_KEY").is_none() {
                    return Err(
                        "DEEPSEEK_API_KEY is not set; export it before building the agent"
                            .to_owned(),
                    );
                }
            }
        }
        if !self.temperature.is_finite() || !(0.0..=2.0).contains(&self.temperature) {
            return Err("temperature must be between 0 and 2".to_owned());
        }
        if self.max_tokens == 0 {
            return Err("max tokens must be at least 1".to_owned());
        }
        if self.context_tokens < 256 {
            return Err("context tokens must be at least 256".to_owned());
        }
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

    /// Re-read the profile list from the workspace.
    pub fn refresh_profiles(&mut self) {
        let workspace = Path::new(self.workspace.trim());
        match RagrigConfig::list_profiles(workspace) {
            Ok(profiles) => {
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
            Err(error) => {
                self.profiles.clear();
                self.status = Some(format!("cannot list profiles: {error:#}"));
            }
        }
    }

    /// Save these settings as a named profile in the workspace.
    pub fn save_profile(&mut self) -> Result<(), String> {
        let name = self.profile_name.trim().to_owned();
        validate_profile_name(&name)?;
        let mut config = self.to_config()?;
        let workspace = Path::new(self.workspace.trim());
        let prompt = self.system_prompt.trim();
        if prompt.is_empty() {
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

    /// Load a named profile from the workspace into the editable fields.
    pub fn load_profile(&mut self, name: &str) -> Result<(), String> {
        validate_profile_name(name)?;
        let workspace = Path::new(self.workspace.trim());
        let config = RagrigConfig::load_from_profile(workspace, name)
            .map_err(|error| format!("cannot load profile '{name}': {error:#}"))?;
        let profiles = self.profiles.clone();
        let mut loaded = Self::from_config(&config);
        loaded.profiles = profiles;
        loaded.profile_name = name.to_owned();
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

fn default_prompt() -> String {
    "Answer from the indexed corpus and cite the documents you used.".to_owned()
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
        settings.ollama_model = "qwen3.5:9b".to_owned();
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
        assert_eq!(settings.ollama_model, "qwen3.5:9b");
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

    fn unique_temp_dir(prefix: &str) -> PathBuf {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let counter = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("{prefix}-{}-{counter}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }
}
