//! First-run setup wizard.
//!
//! Scico needs a local embedding model, a chat model, and token and retrieval
//! limits before the first question. [`onboarding_page`] walks through those
//! choices in a few steps; the host decides when to show it and what to do when
//! it finishes.
//!
//! The page edits the live [`RagSettings`] entity, so every control is the same
//! one the settings page uses. [`RagOnboarding::original`] holds the snapshot
//! taken when the wizard started, which lets a host restore it when the user
//! skips.

use std::{fs, path::Path, rc::Rc};

use gpui_kit::{
    App, Entity, FontWeight, IntoElement, ParentElement as _, SharedString, Styled as _, Window,
    component::{
        ActiveTheme as _, Disableable as _, Sizable as _,
        button::{Button, ButtonVariants as _},
        h_flex,
        setting::{NumberFieldOptions, SettingGroup, SettingItem, SettingPage},
        v_flex,
    },
    div, px,
};
use ragrig::agents::DEFAULT_LLAMACPP_BASE_URL;

use crate::{
    SettingRow, dropdown_control, number_control, switch_control, text_control, token_control,
};

#[cfg(feature = "grobid")]
use crate::services::grobid_group;
use crate::services::{ExternalServices, ServiceKind, docker_group, llamacpp_group, ollama_group};

use super::{ChatProvider, RagSettings, ServiceSource, no_changes};

/// Marker written into `<workspace>/.ragrig` after setup finishes or is skipped.
pub const SETUP_MARKER: &str = ".onboarded";

/// One step of the setup guide.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OnboardingStep {
    /// The local embedding model and its Ollama server.
    Embeddings,
    /// The chat provider and model.
    Chat,
    /// Context window and answer limit.
    Tokens,
    /// Top-k and similarity threshold.
    Retrieval,
    /// Optional GROBID configuration.
    References,
    /// Summary and finish.
    Review,
}

impl OnboardingStep {
    /// The steps in order.
    ///
    /// The references step only exists when the crate is built with the
    /// `grobid` feature.
    pub fn steps() -> &'static [Self] {
        #[cfg(feature = "grobid")]
        {
            &[
                Self::Embeddings,
                Self::Chat,
                Self::Tokens,
                Self::Retrieval,
                Self::References,
                Self::Review,
            ]
        }
        #[cfg(not(feature = "grobid"))]
        {
            &[
                Self::Embeddings,
                Self::Chat,
                Self::Tokens,
                Self::Retrieval,
                Self::Review,
            ]
        }
    }

    fn position(self) -> Option<usize> {
        Self::steps().iter().position(|step| *step == self)
    }

    /// Position in [`Self::steps`], zero-based.
    pub fn index(self) -> usize {
        self.position().unwrap_or(0)
    }

    /// The next step, when the guide has one.
    pub fn next_step(self) -> Option<Self> {
        let index = self.position()?;
        Self::steps().get(index + 1).copied()
    }

    /// The previous step, when the guide has one.
    pub fn previous_step(self) -> Option<Self> {
        let index = self.position()?;
        index
            .checked_sub(1)
            .and_then(|index| Self::steps().get(index).copied())
    }

    /// Human-readable step name.
    pub fn title(self) -> &'static str {
        match self {
            Self::Embeddings => "Local embeddings",
            Self::Chat => "Chat model",
            Self::Tokens => "Context and answer size",
            Self::Retrieval => "Retrieval",
            Self::References => "References",
            Self::Review => "Review",
        }
    }

    /// One sentence explaining the step.
    pub fn description(self) -> &'static str {
        match self {
            Self::Embeddings => {
                "Scico embeds your library with a small local model so it can find the passages that answer a question."
            }
            Self::Chat => {
                "Choose the model that writes with you. A frontier model gives the best results; a local model keeps everything on this machine."
            }
            Self::Tokens => {
                "Set how much the model sees and how long one answer may be. The suggestion is based on the selected model."
            }
            Self::Retrieval => {
                "Choose how many passages Scico retrieves and how similar they must be."
            }
            Self::References => {
                "GROBID reads authors, title, year, and DOI from PDFs. It is optional."
            }
            Self::Review => "Check the choices. Finish applies them and starts the agent.",
        }
    }
}

/// Suggested token limits for a chat model.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TokenSuggestion {
    /// Suggested `RagSettings::context_tokens`.
    pub context_tokens: usize,
    /// Suggested `RagSettings::max_tokens`.
    pub max_tokens: usize,
    /// Why these values fit this model.
    pub note: &'static str,
}

/// Suggest a context window and answer limit for `provider`/`model`.
///
/// Frontier models get a generous window because their context is large and
/// they are billed per token; local models get a budget that matches their
/// parameter count, so a small model does not drown in retrieval.
pub fn token_suggestion(provider: ChatProvider, model: &str) -> TokenSuggestion {
    match provider {
        ChatProvider::Ollama | ChatProvider::LlamaCpp => local_suggestion(model),
        ChatProvider::DeepSeek => TokenSuggestion {
            context_tokens: 32_768,
            max_tokens: 4_096,
            note: "DeepSeek models handle long prompts; 32k leaves room for several papers.",
        },
        ChatProvider::OpenAi
        | ChatProvider::Claude
        | ChatProvider::Mistral
        | ChatProvider::OpenRouter => TokenSuggestion {
            context_tokens: 32_768,
            max_tokens: 4_096,
            note: "Frontier models handle long prompts. Raise the answer limit only for long drafts.",
        },
    }
}

fn local_suggestion(model: &str) -> TokenSuggestion {
    match parameter_billions(model) {
        Some(size) if size >= 30.0 => TokenSuggestion {
            context_tokens: 16_384,
            max_tokens: 4_096,
            note: "A large local model: 16k context leaves room for retrieval; 4k answers keep generation time reasonable.",
        },
        Some(size) if size >= 7.0 => TokenSuggestion {
            context_tokens: 8_192,
            max_tokens: 2_048,
            note: "A mid-sized local model: 8k context and 2k answers balance quality and speed.",
        },
        Some(_) => TokenSuggestion {
            context_tokens: 4_096,
            max_tokens: 1_024,
            note: "A small local model: a tight window keeps answers fast.",
        },
        None => TokenSuggestion {
            context_tokens: 8_192,
            max_tokens: 2_048,
            note: "The model size is not in its name; 8k context and 2k answers are a safe start.",
        },
    }
}

/// Read a parameter count such as `7b`, `9b`, `30b`, or `3.8b` from a model name.
fn parameter_billions(model: &str) -> Option<f64> {
    model
        .to_ascii_lowercase()
        .split(|character: char| !(character.is_ascii_alphanumeric() || character == '.'))
        .find_map(|token| token.strip_suffix('b')?.parse::<f64>().ok())
}

/// Whether a workspace already holds Ragrig or Scico data.
///
/// The directories Scico scaffolds on every open do not count; a user-wide
/// profile or a saved chat session in the workspace does.
pub fn workspace_has_history(workspace: &Path) -> bool {
    let ragrig = workspace.join(".ragrig");
    ragrig.join("config.json").is_file()
        || ragrig.join(SETUP_MARKER).is_file()
        || ragrig.join(".ragrig_store").exists()
        || directory_has_entries(&ragrig.join("profiles"))
        || directory_has_entries(&ragrig.join("sessions"))
}

fn directory_has_entries(path: &Path) -> bool {
    fs::read_dir(path).is_ok_and(|mut entries| entries.next().is_some())
}

/// Record that the setup guide ran for `workspace`.
pub fn mark_workspace_setup_done(workspace: &Path) -> Result<(), String> {
    let dir = workspace.join(".ragrig");
    fs::create_dir_all(&dir)
        .map_err(|error| format!("cannot create '{}': {error}", dir.display()))?;
    let path = dir.join(SETUP_MARKER);
    fs::write(&path, "1\n").map_err(|error| format!("cannot write '{}': {error}", path.display()))
}

/// State of one run through the setup guide.
pub struct RagOnboarding {
    original: RagSettings,
    step: OnboardingStep,
    active: bool,
    error: Option<SharedString>,
    run: u64,
}

impl RagOnboarding {
    /// Start the guide from `settings`; `active` is false when the host only
    /// keeps the entity for a later **Run setup** action.
    pub fn new(settings: RagSettings, active: bool) -> Self {
        Self {
            original: settings,
            step: OnboardingStep::steps()
                .first()
                .copied()
                .unwrap_or(OnboardingStep::Embeddings),
            active,
            error: None,
            run: 0,
        }
    }

    /// Whether the guide is waiting for the user.
    pub fn is_active(&self) -> bool {
        self.active
    }

    /// The current step.
    pub fn step(&self) -> OnboardingStep {
        self.step
    }

    /// Identifies this run of the guide.
    ///
    /// Hosts can use it in the settings element id so re-opening the guide
    /// starts on its first page instead of a page selected during an earlier
    /// run.
    pub fn run(&self) -> u64 {
        self.run
    }

    /// The settings captured when the guide started.
    pub fn original(&self) -> &RagSettings {
        &self.original
    }

    /// The current validation message.
    pub fn error(&self) -> Option<&SharedString> {
        self.error.as_ref()
    }

    /// Show a validation message.
    pub fn set_error(&mut self, message: impl Into<SharedString>) {
        self.error = Some(message.into());
    }

    /// Clear the validation message.
    pub fn clear_error(&mut self) {
        self.error = None;
    }

    /// Advance; returns false on the last step.
    pub fn advance(&mut self) -> bool {
        let Some(step) = self.step.next_step() else {
            return false;
        };
        self.step = step;
        self.error = None;
        true
    }

    /// Go back; returns false on the first step.
    pub fn back(&mut self) -> bool {
        let Some(step) = self.step.previous_step() else {
            return false;
        };
        self.step = step;
        self.error = None;
        true
    }

    /// Mark the guide finished.
    pub fn finish(&mut self) {
        self.active = false;
        self.error = None;
    }

    /// Mark the guide skipped.
    pub fn skip(&mut self) {
        self.finish();
    }

    /// Re-open the guide from the current settings.
    pub fn restart(&mut self, settings: &RagSettings) {
        self.original = settings.clone();
        self.step = OnboardingStep::steps()
            .first()
            .copied()
            .unwrap_or(OnboardingStep::Embeddings);
        self.active = true;
        self.error = None;
        self.run = self.run.wrapping_add(1);
    }
}

/// A host action invoked by the guide.
pub type OnboardingAction = Rc<dyn Fn(&mut Window, &mut App)>;

/// Host actions at the end of the guide.
#[derive(Clone)]
pub struct OnboardingActions {
    /// Apply and persist the settings.
    pub finish: OnboardingAction,
    /// Restore the snapshot and persist it.
    pub skip: OnboardingAction,
}

impl OnboardingActions {
    /// Build the action pair from two callbacks.
    pub fn new(
        finish: impl Fn(&mut Window, &mut App) + 'static,
        skip: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        Self {
            finish: Rc::new(finish),
            skip: Rc::new(skip),
        }
    }
}

/// Build the setup page for the guide's current step.
///
/// The host should observe `onboarding` and re-render on its notifications:
/// a step change swaps whole groups, not just values inside them.
pub fn onboarding_page(
    onboarding: Entity<RagOnboarding>,
    settings: Entity<RagSettings>,
    services: Entity<ExternalServices>,
    actions: OnboardingActions,
    cx: &App,
) -> SettingPage {
    let step = onboarding.read(cx).step();
    let mut page = SettingPage::new("Setup")
        .description("A short guided setup. You can change every value later in Settings.")
        .default_open(true)
        .resettable(false)
        .group(progress_group(step, onboarding.clone()));
    for group in step_groups(step, &settings, &services, cx) {
        page = page.group(group);
    }
    page.group(navigation_group(step, onboarding, settings, actions))
}

fn progress_group(step: OnboardingStep, onboarding: Entity<RagOnboarding>) -> SettingGroup {
    let title = if step.index() == 0 {
        "Welcome"
    } else {
        "Getting started"
    };
    SettingGroup::new()
        .title(title)
        .item(SettingItem::render(move |_, _, cx| {
            let onboarding = onboarding.read(cx);
            let step = onboarding.step();
            let count = OnboardingStep::steps().len();
            v_flex()
                .gap_1()
                .child(div().text_sm().font_weight(FontWeight::BOLD).child(format!(
                    "Step {} of {count} · {}",
                    step.index() + 1,
                    step.title()
                )))
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(step.description()),
                )
        }))
}

fn step_groups(
    step: OnboardingStep,
    settings: &Entity<RagSettings>,
    services: &Entity<ExternalServices>,
    cx: &App,
) -> Vec<SettingGroup> {
    match step {
        OnboardingStep::Embeddings => embeddings_groups(settings, services, cx),
        OnboardingStep::Chat => chat_groups(settings, services, cx),
        OnboardingStep::Tokens => vec![tokens_group(settings)],
        OnboardingStep::Retrieval => vec![retrieval_group(settings)],
        OnboardingStep::References => references_groups(settings, services, cx),
        OnboardingStep::Review => vec![review_group(settings)],
    }
}

fn embeddings_groups(
    settings: &Entity<RagSettings>,
    services: &Entity<ExternalServices>,
    cx: &App,
) -> Vec<SettingGroup> {
    let mut groups = Vec::new();
    if uses_docker(settings, &[ServiceKind::Ollama], cx) {
        groups.push(docker_group(settings.clone(), services.clone()));
    }
    groups.push(ollama_group(
        settings.clone(),
        services.clone(),
        no_changes(),
        cx,
    ));
    groups.push(embedding_model_group(settings));
    groups
}

fn embedding_model_group(settings: &Entity<RagSettings>) -> SettingGroup {
    let reader = settings.clone();
    let writer = settings.clone();
    let embed_model = text_control(
        "onboarding-embed-model",
        move |cx| reader.read(cx).embed_model.clone(),
        move |value, cx| {
            writer.update(cx, |settings, cx| {
                settings.embed_model = value;
                cx.notify();
            });
        },
    );
    let reader = settings.clone();
    let writer = settings.clone();
    let hybrid = switch_control(
        "onboarding-hybrid",
        move |cx| reader.read(cx).hybrid,
        move |value, cx| {
            writer.update(cx, |settings, cx| {
                settings.hybrid = value;
                cx.notify();
            });
        },
    );

    SettingGroup::new()
        .title("Embedding model")
        .description(
            "A small local model turns your library into searchable vectors. nomic-embed-text:latest is the recommended default; mxbai-embed-large is stronger but larger; all-minilm is the fastest.",
        )
        .item(
            SettingRow::new("Embedding model", embed_model)
                .description("Served by the Ollama instance above.")
                .into_item(),
        )
        .item(
            SettingRow::new("Hybrid retrieval", hybrid)
                .description(
                    "BM25 plus embeddings. Off skips vector search and needs no embedding model.",
                )
                .into_item(),
        )
}

fn chat_groups(
    settings: &Entity<RagSettings>,
    services: &Entity<ExternalServices>,
    cx: &App,
) -> Vec<SettingGroup> {
    let provider = settings.read(cx).provider;
    let mut groups = vec![chat_model_group(settings, provider)];
    match provider {
        ChatProvider::LlamaCpp => {
            if uses_docker(settings, &[ServiceKind::LlamaCpp], cx) {
                groups.push(docker_group(settings.clone(), services.clone()));
            }
            groups.push(server_url_group(settings));
            groups.push(llamacpp_group(
                settings.clone(),
                services.clone(),
                no_changes(),
                cx,
            ));
        }
        ChatProvider::Ollama => groups.push(ollama_chat_group(settings)),
        _ => {}
    }
    groups
}

fn chat_model_group(settings: &Entity<RagSettings>, provider: ChatProvider) -> SettingGroup {
    let reader = settings.clone();
    let writer = settings.clone();
    let provider_control = dropdown_control(
        "onboarding-provider",
        |_| {
            ChatProvider::options()
                .iter()
                .map(|(provider, label)| {
                    (
                        SharedString::from(provider.label()),
                        SharedString::from(*label),
                    )
                })
                .collect()
        },
        move |cx| reader.read(cx).provider.label().to_owned(),
        move |value, cx| {
            let provider = ChatProvider::from_label(&value).unwrap_or(ChatProvider::Ollama);
            writer.update(cx, |settings, cx| {
                settings.provider = provider;
                cx.notify();
            });
        },
    );
    let reader = settings.clone();
    let writer = settings.clone();
    let model = text_control(
        "onboarding-chat-model",
        move |cx| reader.read(cx).model().to_owned(),
        move |value, cx| {
            writer.update(cx, |settings, cx| {
                settings.set_model(value);
                cx.notify();
            });
        },
    );
    let api_settings = settings.clone();
    let api_key = SettingItem::render(move |_, _, cx| {
        let provider = api_settings.read(cx).provider;
        let (message, color) = match provider {
            ChatProvider::Ollama => (
                "Ollama needs no API key.".to_owned(),
                cx.theme().muted_foreground,
            ),
            ChatProvider::LlamaCpp => (
                "LLAMACPP_API_KEY is optional and only needed for a secured server.".to_owned(),
                cx.theme().muted_foreground,
            ),
            _ => match provider.env_var() {
                Some(variable) if std::env::var_os(variable).is_some() => (
                    format!("{variable} is set in this environment."),
                    cx.theme().muted_foreground,
                ),
                Some(variable) => (
                    format!(
                        "{variable} is not set. Set it before launching Scico; the key is never written to settings or profiles."
                    ),
                    cx.theme().danger,
                ),
                None => (
                    "This provider needs no API key.".to_owned(),
                    cx.theme().muted_foreground,
                ),
            },
        };
        div()
            .text_sm()
            .text_color(color)
            .child(message)
            .into_any_element()
    });

    SettingGroup::new()
        .title("Chat model")
        .description(
            "For the best writing, use a frontier model (DeepSeek, OpenAI, Claude, Mistral, OpenRouter). Local Ollama and llama.cpp models keep everything on this machine, but need more hardware.",
        )
        .item(
            SettingRow::new("Provider", provider_control)
                .description("Cloud providers are billed per token; local models are free to run.")
                .into_item(),
        )
        .item(
            SettingRow::new("Model", model)
                .description(format!(
                    "Default for {}: {}. Each provider keeps its own model.",
                    provider.label(),
                    provider.default_model()
                ))
                .into_item(),
        )
        .item(api_key)
        .footer(|_, cx| {
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child("Token limits are suggested in the next step based on this model.")
        })
}

fn server_url_group(settings: &Entity<RagSettings>) -> SettingGroup {
    let reader = settings.clone();
    let writer = settings.clone();
    let url = text_control(
        "onboarding-llamacpp-url",
        move |cx| reader.read(cx).llamacpp_url.clone(),
        move |value, cx| {
            writer.update(cx, |settings, cx| {
                settings.llamacpp_url = value;
                cx.notify();
            });
        },
    );
    SettingGroup::new()
        .title("llama.cpp server")
        .description("The managed container publishes its API on this URL; an existing server keeps its own.")
        .item(
            SettingRow::new("Server URL", url)
                .description(format!(
                    "Include /v1. The default is {DEFAULT_LLAMACPP_BASE_URL}."
                ))
                .into_item(),
        )
}

fn ollama_chat_group(settings: &Entity<RagSettings>) -> SettingGroup {
    let settings = settings.clone();
    SettingGroup::new()
        .title("Local chat with Ollama")
        .description(
            "The Ollama server from step 1 serves this model too. Use Pull models on the Ollama group to fetch it; a missing model is reported in Chat when the agent starts.",
        )
        .item(SettingItem::render(move |_, _, cx| {
            let settings = settings.read(cx);
            div()
                .text_sm()
                .child(format!(
                    "{} · {}",
                    settings.provider.label(),
                    settings.effective_model()
                ))
                .into_any_element()
        }))
}

fn tokens_group(settings: &Entity<RagSettings>) -> SettingGroup {
    let reader = settings.clone();
    let writer = settings.clone();
    let context = token_control(
        "onboarding-context-tokens",
        9,
        20,
        move |cx| reader.read(cx).context_tokens,
        move |value, cx| {
            writer.update(cx, |settings, cx| {
                settings.context_tokens = value;
                cx.notify();
            });
        },
    );
    let reader = settings.clone();
    let writer = settings.clone();
    let max_tokens = token_control(
        "onboarding-max-tokens",
        9,
        20,
        move |cx| reader.read(cx).max_tokens,
        move |value, cx| {
            writer.update(cx, |settings, cx| {
                settings.max_tokens = value;
                cx.notify();
            });
        },
    );
    let suggestion_settings = settings.clone();
    let suggestion_writer = settings.clone();
    let suggestion = SettingItem::render(move |_, _, cx| {
        let settings = suggestion_settings.read(cx);
        let suggestion = token_suggestion(settings.provider, settings.effective_model());
        let matches = settings.context_tokens == suggestion.context_tokens
            && settings.max_tokens == suggestion.max_tokens;
        let writer = suggestion_writer.clone();
        h_flex()
            .w_full()
            .items_center()
            .gap_3()
            .child(
                v_flex()
                    .flex_1()
                    .gap_1()
                    .child(div().text_sm().child(format!(
                        "Suggested for {} · {}: {} context tokens, {} answer tokens.",
                        settings.provider.label(),
                        settings.effective_model(),
                        suggestion.context_tokens,
                        suggestion.max_tokens
                    )))
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(suggestion.note),
                    ),
            )
            .child(
                Button::new("onboarding-use-suggestion")
                    .label(if matches {
                        "Suggested"
                    } else {
                        "Use suggested"
                    })
                    .small()
                    .outline()
                    .disabled(matches)
                    .on_click(move |_, _, cx| {
                        writer.update(cx, |settings, cx| {
                            let suggestion =
                                token_suggestion(settings.provider, settings.effective_model());
                            settings.context_tokens = suggestion.context_tokens;
                            settings.max_tokens = suggestion.max_tokens;
                            cx.notify();
                        });
                    }),
            )
            .into_any_element()
    });

    SettingGroup::new()
        .title("Context and answer size")
        .description(
            "The context window is how much retrieved text and conversation the model sees. The answer limit caps a single reply.",
        )
        .item(
            SettingRow::new("Context tokens", context)
                .description("Total prompt budget. The suggestion keeps room for retrieval.")
                .into_item(),
        )
        .item(
            SettingRow::new("Maximum tokens", max_tokens)
                .description("Longest single answer.")
                .into_item(),
        )
        .item(suggestion)
}

fn retrieval_group(settings: &Entity<RagSettings>) -> SettingGroup {
    let reader = settings.clone();
    let writer = settings.clone();
    let top_k = number_control(
        "onboarding-top-k",
        NumberFieldOptions {
            min: 1.0,
            max: 500.0,
            step: 1.0,
        },
        move |cx| reader.read(cx).top_k as f64,
        move |value, cx| {
            writer.update(cx, |settings, cx| {
                settings.top_k = value.round().clamp(1.0, 500.0) as usize;
                cx.notify();
            });
        },
    );
    let reader = settings.clone();
    let writer = settings.clone();
    let threshold = number_control(
        "onboarding-threshold",
        NumberFieldOptions {
            min: 0.0,
            max: 1.0,
            step: 0.01,
        },
        move |cx| reader.read(cx).similarity_threshold,
        move |value, cx| {
            writer.update(cx, |settings, cx| {
                settings.similarity_threshold = value.clamp(0.0, 1.0);
                cx.notify();
            });
        },
    );

    SettingGroup::new()
        .title("Retrieval")
        .description(
            "Scico retrieves passages by keyword and by embedding similarity, then keeps the best matches.",
        )
        .item(
            SettingRow::new("Top-k chunks", top_k)
                .description("How many passages to retrieve. 50 is a good start for a paper library.")
                .into_item(),
        )
        .item(
            SettingRow::new("Similarity threshold", threshold)
                .description(
                    "Weak matches below this score are dropped. 0.04 is permissive; raise it if answers cite irrelevant passages.",
                )
                .into_item(),
        )
}

fn references_groups(
    settings: &Entity<RagSettings>,
    services: &Entity<ExternalServices>,
    cx: &App,
) -> Vec<SettingGroup> {
    #[cfg(feature = "grobid")]
    {
        let mut groups = vec![SettingGroup::new()
            .title("Optional")
            .description(
                "Without GROBID, Scico still indexes PDFs; you can fill in the .bib by hand and re-run Import PDFs later.",
            )
            .item(SettingItem::render(|_, _, cx| {
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child("You can configure GROBID later in Settings → External services.")
                    .into_any_element()
            }))];
        if uses_docker(settings, &[ServiceKind::Grobid], cx) {
            groups.push(docker_group(settings.clone(), services.clone()));
        }
        groups.push(grobid_group(
            settings.clone(),
            services.clone(),
            no_changes(),
            cx,
        ));
        groups
    }
    #[cfg(not(feature = "grobid"))]
    {
        let _ = (settings, services, cx);
        Vec::new()
    }
}

fn review_group(settings: &Entity<RagSettings>) -> SettingGroup {
    let settings = settings.clone();
    SettingGroup::new()
        .title("Review")
        .description("Finish applies these settings and starts the agent.")
        .item(SettingItem::render(move |_, _, cx| {
            let settings = settings.read(cx);
            let mut lines = v_flex()
                .gap_2()
                .child(summary_line("Workspace", settings.workspace.clone()))
                .child(summary_line(
                    "Embeddings",
                    if settings.hybrid {
                        let detail = match settings.ollama_source {
                            ServiceSource::Existing => String::new(),
                            ServiceSource::ExistingContainer => {
                                format!(" · {}", settings.ollama_container)
                            }
                            ServiceSource::Managed => format!(" · {}", settings.ollama_image),
                        };
                        format!(
                            "{} via Ollama ({}){detail}",
                            settings.embed_model,
                            settings.ollama_source.label()
                        )
                    } else {
                        "Off — keyword search only".to_owned()
                    },
                ))
                .child(summary_line(
                    "Chat",
                    format!(
                        "{} · {}",
                        settings.provider.label(),
                        settings.effective_model()
                    ),
                ))
                .child(summary_line(
                    "Context",
                    format!(
                        "{} tokens · at most {} answer tokens",
                        settings.context_tokens, settings.max_tokens
                    ),
                ))
                .child(summary_line(
                    "Retrieval",
                    format!(
                        "top {} · threshold {:.2}",
                        settings.top_k, settings.similarity_threshold
                    ),
                ));
            if settings.provider == ChatProvider::LlamaCpp {
                lines = lines.child(summary_line("Server", settings.llamacpp_url.clone()));
            }
            #[cfg(feature = "grobid")]
            {
                lines = lines.child(summary_line(
                    "GROBID",
                    match settings.grobid_source {
                        ServiceSource::Existing => {
                            format!("{} · {}", settings.grobid_source.label(), settings.grobid_url)
                        }
                        ServiceSource::ExistingContainer => format!(
                            "{} · {}",
                            settings.grobid_source.label(),
                            settings.grobid_container
                        ),
                        ServiceSource::Managed => {
                            format!("{} · {}", settings.grobid_source.label(), settings.grobid_image)
                        }
                    },
                ));
            }
            if settings.provider != ChatProvider::LlamaCpp
                && let Some(variable) = settings.provider.env_var()
                && std::env::var_os(variable).is_none()
            {
                lines = lines.child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().danger)
                        .child(format!(
                            "{variable} is not set in this environment. Set it and restart Scico before asking a question."
                        )),
                );
            }
            lines.into_any_element()
        }))
}

fn summary_line(label: &'static str, value: String) -> impl IntoElement {
    h_flex()
        .items_start()
        .gap_2()
        .child(
            div()
                .w(px(120.))
                .flex_none()
                .text_sm()
                .font_weight(FontWeight::BOLD)
                .child(label),
        )
        .child(div().text_sm().child(value))
}

fn navigation_group(
    step: OnboardingStep,
    onboarding: Entity<RagOnboarding>,
    settings: Entity<RagSettings>,
    actions: OnboardingActions,
) -> SettingGroup {
    let back_onboarding = onboarding.clone();
    let next_onboarding = onboarding.clone();
    let next_settings = settings.clone();
    let finish = actions.finish.clone();
    let skip_action = actions.skip.clone();
    let can_back = step.previous_step().is_some();
    let next_label = if step == OnboardingStep::Review {
        "Finish setup"
    } else {
        "Continue"
    };
    let error_onboarding = onboarding;

    SettingGroup::new()
        .title("Continue")
        .item(SettingItem::render(move |_, _, _cx| {
            let back = {
                let onboarding = back_onboarding.clone();
                Button::new("onboarding-back")
                    .label("Back")
                    .small()
                    .outline()
                    .disabled(!can_back)
                    .on_click(move |_, _, cx| {
                        onboarding.update(cx, |onboarding, cx| {
                            onboarding.back();
                            cx.notify();
                        });
                    })
            };
            let next = {
                let onboarding = next_onboarding.clone();
                let settings = next_settings.clone();
                let finish = finish.clone();
                Button::new("onboarding-next")
                    .label(next_label)
                    .small()
                    .primary()
                    .on_click(move |_, window, cx| {
                        if let Some(error) = step_error(step, settings.read(cx)) {
                            onboarding.update(cx, |onboarding, cx| {
                                onboarding.set_error(error);
                                cx.notify();
                            });
                            return;
                        }
                        if step == OnboardingStep::Review {
                            (finish)(window, cx);
                        } else {
                            onboarding.update(cx, |onboarding, cx| {
                                onboarding.advance();
                                cx.notify();
                            });
                        }
                    })
            };
            let skip = {
                let skip = skip_action.clone();
                Button::new("onboarding-skip")
                    .label("Set up later")
                    .small()
                    .ghost()
                    .on_click(move |_, window, cx| (skip)(window, cx))
            };
            h_flex()
                .w_full()
                .items_center()
                .gap_2()
                .child(back)
                .child(next)
                .child(skip)
                .into_any_element()
        }))
        .item(SettingItem::render(move |_, _, cx| {
            match error_onboarding.read(cx).error() {
                Some(error) => div()
                    .text_sm()
                    .text_color(cx.theme().danger)
                    .child(error.clone())
                    .into_any_element(),
                None => div().into_any_element(),
            }
        }))
        .footer(|_, cx| {
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child("Set up later keeps the current settings. You can run the guide again from Settings → Overview.")
        })
}

fn step_error(step: OnboardingStep, settings: &RagSettings) -> Option<String> {
    match step {
        OnboardingStep::Embeddings => {
            if settings.hybrid && settings.embed_model.trim().is_empty() {
                return Some("Enter an embedding model, or turn off hybrid retrieval.".to_owned());
            }
            if settings.hybrid
                && settings.ollama_source == ServiceSource::ExistingContainer
                && settings.ollama_container.trim().is_empty()
            {
                return Some("Enter the Ollama container name, or switch its source.".to_owned());
            }
            if settings.hybrid
                && settings.ollama_source == ServiceSource::Managed
                && settings.ollama_image.trim().is_empty()
            {
                return Some("Enter a Docker image for the Ollama container.".to_owned());
            }
            None
        }
        OnboardingStep::Chat => {
            if settings.model().trim().is_empty() {
                return Some(format!("Enter a {} chat model.", settings.provider.label()));
            }
            match settings.provider {
                ChatProvider::LlamaCpp => {
                    if settings.llamacpp_url.trim().is_empty() {
                        return Some("Enter the llama.cpp server URL.".to_owned());
                    }
                    match settings.llamacpp_source {
                        ServiceSource::ExistingContainer
                            if settings.llamacpp_container.trim().is_empty() =>
                        {
                            Some(
                                "Enter the llama.cpp container name, or switch its source."
                                    .to_owned(),
                            )
                        }
                        ServiceSource::Managed if settings.llamacpp_image.trim().is_empty() => {
                            Some("Enter a Docker image for llama.cpp.".to_owned())
                        }
                        ServiceSource::Managed if settings.llamacpp_model.trim().is_empty() => {
                            Some(
                                "Enter a Hugging Face model or a .gguf file for llama.cpp."
                                    .to_owned(),
                            )
                        }
                        _ => None,
                    }
                }
                ChatProvider::Ollama => match settings.ollama_source {
                    ServiceSource::ExistingContainer
                        if settings.ollama_container.trim().is_empty() =>
                    {
                        Some("Enter the Ollama container name, or switch its source.".to_owned())
                    }
                    ServiceSource::Managed if settings.ollama_image.trim().is_empty() => {
                        Some("Enter a Docker image for the Ollama container.".to_owned())
                    }
                    _ => None,
                },
                _ => None,
            }
        }
        OnboardingStep::Tokens => {
            if settings.max_tokens == 0 {
                Some("Enter a maximum answer length.".to_owned())
            } else if settings.max_tokens >= settings.context_tokens {
                Some(
                    "Keep the answer limit below the context window; otherwise retrieval has no room."
                        .to_owned(),
                )
            } else {
                None
            }
        }
        OnboardingStep::Retrieval => {
            if settings.top_k == 0 {
                Some("Retrieve at least one chunk.".to_owned())
            } else if !(0.0..=1.0).contains(&settings.similarity_threshold) {
                Some("The similarity threshold must be between 0 and 1.".to_owned())
            } else {
                None
            }
        }
        OnboardingStep::References => None,
        OnboardingStep::Review => settings.validate().err(),
    }
}

fn uses_docker(settings: &Entity<RagSettings>, kinds: &[ServiceKind], cx: &App) -> bool {
    let settings = settings.read(cx);
    kinds.iter().any(|kind| {
        matches!(
            settings.service_source(*kind),
            ServiceSource::Managed | ServiceSource::ExistingContainer
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_guide_visits_every_step_in_both_directions() {
        let settings = RagSettings::default();
        let mut onboarding = RagOnboarding::new(settings, true);
        let steps = OnboardingStep::steps();
        assert_eq!(onboarding.step(), steps[0]);
        for expected in &steps[1..] {
            assert!(onboarding.advance());
            assert_eq!(onboarding.step(), *expected);
        }
        assert!(!onboarding.advance(), "the last step has no successor");
        for expected in steps[..steps.len() - 1].iter().rev() {
            assert!(onboarding.back());
            assert_eq!(onboarding.step(), *expected);
        }
        assert!(!onboarding.back(), "the first step has no predecessor");
    }

    #[test]
    fn skipping_and_restarting_the_guide() {
        let settings = RagSettings::default();
        let mut onboarding = RagOnboarding::new(settings.clone(), true);
        onboarding.set_error("broken");
        onboarding.skip();
        assert!(!onboarding.is_active());
        assert!(onboarding.error().is_none());

        onboarding.restart(&settings);
        assert!(onboarding.is_active());
        assert_eq!(onboarding.step(), OnboardingStep::steps()[0]);
        assert_eq!(onboarding.original(), &settings);
    }

    #[test]
    fn local_token_suggestions_follow_the_parameter_count() {
        let small = token_suggestion(ChatProvider::Ollama, "qwen3:1.7b");
        assert_eq!(small.context_tokens, 4096);
        assert_eq!(small.max_tokens, 1024);
        assert!(small.max_tokens < small.context_tokens);

        let mid = token_suggestion(ChatProvider::Ollama, "qwen3.5:9b");
        assert_eq!(mid.context_tokens, 8192);
        assert_eq!(mid.max_tokens, 2048);

        let large = token_suggestion(ChatProvider::LlamaCpp, "llama3.1:70b");
        assert_eq!(large.context_tokens, 16_384);
        assert_eq!(large.max_tokens, 4096);

        let unknown = token_suggestion(ChatProvider::Ollama, "custom-model");
        assert_eq!(unknown.context_tokens, 8192);
        assert_eq!(unknown.max_tokens, 2048);
    }

    #[test]
    fn frontier_token_suggestions_are_generous() {
        for provider in [
            ChatProvider::DeepSeek,
            ChatProvider::OpenAi,
            ChatProvider::Claude,
            ChatProvider::Mistral,
            ChatProvider::OpenRouter,
        ] {
            let suggestion = token_suggestion(provider, "model-name");
            assert_eq!(suggestion.context_tokens, 32_768);
            assert_eq!(suggestion.max_tokens, 4_096);
            assert!(suggestion.max_tokens < suggestion.context_tokens);
        }
    }

    #[test]
    fn a_scaffolded_workspace_with_empty_folders_is_new() {
        let workspace = unique_temp_dir("ragrig-gpui-onboarding");
        std::fs::create_dir_all(workspace.join(".ragrig/profiles")).expect("profiles");
        std::fs::create_dir_all(workspace.join(".ragrig/sessions")).expect("sessions");
        assert!(!workspace_has_history(&workspace));

        mark_workspace_setup_done(&workspace).expect("marker");
        assert!(workspace_has_history(&workspace));
        let _ = std::fs::remove_dir_all(workspace);
    }

    #[test]
    fn workspace_history_is_detected_from_data_not_from_folders() {
        for marker in ["config.json", "profiles/lab.json", "sessions/one.json"] {
            let workspace = unique_temp_dir("ragrig-gpui-history");
            let path = workspace.join(".ragrig").join(marker);
            std::fs::create_dir_all(path.parent().expect("parent")).expect("parent");
            std::fs::write(&path, "{}").expect("file");
            assert!(workspace_has_history(&workspace), "{marker} is history");
            let _ = std::fs::remove_dir_all(workspace);
        }

        let workspace = unique_temp_dir("ragrig-gpui-store");
        std::fs::create_dir_all(workspace.join(".ragrig/.ragrig_store")).expect("store");
        assert!(workspace_has_history(&workspace));
        let _ = std::fs::remove_dir_all(workspace);
    }

    #[test]
    fn the_first_step_requires_an_embedding_model_only_with_hybrid() {
        let mut settings = RagSettings::default();
        settings.embed_model.clear();
        settings.hybrid = true;
        assert!(step_error(OnboardingStep::Embeddings, &settings).is_some());
        settings.hybrid = false;
        assert!(step_error(OnboardingStep::Embeddings, &settings).is_none());
    }

    #[test]
    fn the_token_step_rejects_an_answer_limit_that_fills_the_window() {
        let mut settings = RagSettings {
            context_tokens: 4096,
            max_tokens: 4096,
            ..RagSettings::default()
        };
        assert!(step_error(OnboardingStep::Tokens, &settings).is_some());
        settings.max_tokens = 2048;
        assert!(step_error(OnboardingStep::Tokens, &settings).is_none());
    }

    fn unique_temp_dir(prefix: &str) -> std::path::PathBuf {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let counter = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("{prefix}-{}-{counter}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }
}
