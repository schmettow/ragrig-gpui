//! Chat settings shown on the settings page.

/// Chat-model configuration.
///
/// This is deliberately smaller than Ragrig's full configuration: it covers
/// the fields a chat surface exposes directly. Memory, embedding, parsing and
/// retrieval settings arrive with the M2 controller.
#[derive(Clone, Debug, PartialEq)]
pub struct ChatSettings {
    /// Model identifier, e.g. `deepseek-chat`.
    pub model: String,
    /// Sampling temperature in `0..=2`.
    pub temperature: f64,
    /// Maximum number of generated tokens.
    pub max_tokens: usize,
    /// System prompt sent before the conversation.
    pub system_prompt: String,
    /// Whether tokens are streamed into the transcript.
    pub stream: bool,
}

impl Default for ChatSettings {
    fn default() -> Self {
        Self {
            model: "deepseek-chat".to_owned(),
            temperature: 0.2,
            max_tokens: 2048,
            system_prompt: "Answer from the indexed corpus and cite sources.".to_owned(),
            stream: true,
        }
    }
}

impl ChatSettings {
    /// Models offered by the settings dropdown.
    ///
    /// The host may replace this list; it is a convenience for the demo and
    /// the default settings page.
    pub const MODELS: &'static [(&'static str, &'static str)] = &[
        ("deepseek-chat", "DeepSeek Chat"),
        ("qwen3:8b", "Qwen3 8B (Ollama)"),
        ("gpt-oss:20b", "GPT-OSS 20B (Ollama)"),
    ];

    /// Check the values before applying them.
    pub fn validate(&self) -> Result<(), String> {
        if self.model.trim().is_empty() {
            return Err("choose a chat model".to_owned());
        }
        if !self.temperature.is_finite() || !(0.0..=2.0).contains(&self.temperature) {
            return Err("temperature must be between 0 and 2".to_owned());
        }
        if self.max_tokens == 0 {
            return Err("max tokens must be at least 1".to_owned());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_valid() {
        assert_eq!(ChatSettings::default().validate(), Ok(()));
    }

    #[test]
    fn invalid_values_are_rejected() {
        let mut settings = ChatSettings {
            model: " ".to_owned(),
            ..Default::default()
        };
        assert_eq!(settings.validate(), Err("choose a chat model".to_owned()));

        settings = ChatSettings {
            temperature: 2.5,
            ..Default::default()
        };
        assert_eq!(
            settings.validate(),
            Err("temperature must be between 0 and 2".to_owned())
        );

        settings = ChatSettings {
            max_tokens: 0,
            ..Default::default()
        };
        assert_eq!(
            settings.validate(),
            Err("max tokens must be at least 1".to_owned())
        );
    }

    #[test]
    fn the_default_model_is_in_the_offered_list() {
        assert!(
            ChatSettings::MODELS
                .iter()
                .any(|(id, _)| *id == ChatSettings::default().model)
        );
    }
}
