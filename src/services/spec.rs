//! Per-service Docker specifications.
//!
//! One [`ServiceKind`] carries everything that differs between the managed
//! containers: names, ports, image defaults, the health probe, and the
//! container body. The lifecycle engine in [`super::managed`] is shared.

use std::{collections::HashMap, path::Path, time::Duration};

use bollard::models::{
    ContainerCreateBody, ContainerInspectResponse, HostConfig, Mount, MountType, PortBinding,
    ResourcesUlimits,
};
use reqwest::Client;

use crate::rag::{ChatProvider, RagSettings};

/// Name of the managed GROBID container.
pub const MANAGED_GROBID_CONTAINER: &str = crate::rag::DEFAULT_GROBID_CONTAINER;
/// Loopback URL exposed by the managed GROBID container.
pub const MANAGED_GROBID_URL: &str = "http://127.0.0.1:8070";
/// Host port exposed by the managed GROBID container.
pub const MANAGED_GROBID_PORT: u16 = 8070;
/// Ports probed by GROBID discovery.
pub const GROBID_DISCOVERY_PORTS: [u16; 2] = [8070, 8080];

/// Name of the managed Ollama container.
pub const MANAGED_OLLAMA_CONTAINER: &str = crate::rag::DEFAULT_OLLAMA_CONTAINER;
/// Loopback URL exposed by the managed Ollama container.
pub const MANAGED_OLLAMA_URL: &str = "http://localhost:11434";
/// Host port exposed by the managed Ollama container.
pub const MANAGED_OLLAMA_PORT: u16 = 11434;
/// Ports probed by Ollama discovery.
pub const OLLAMA_DISCOVERY_PORTS: [u16; 1] = [11434];

/// Name of the managed llama.cpp container.
pub const MANAGED_LLAMACPP_CONTAINER: &str = crate::rag::DEFAULT_LLAMACPP_CONTAINER;
/// OpenAI-compatible URL exposed by the managed llama.cpp container.
pub const MANAGED_LLAMACPP_URL: &str = "http://127.0.0.1:8080/v1";
/// Host port exposed by the managed llama.cpp container.
pub const MANAGED_LLAMACPP_PORT: u16 = 8080;
/// Ports probed by llama.cpp discovery.
pub const LLAMACPP_DISCOVERY_PORTS: [u16; 1] = [8080];

const CONTAINER_LABEL: &str = "io.ragrig.managed";
const SERVICE_LABEL: &str = "io.ragrig.service";

const OLLAMA_VOLUME: &str = "ragrig-ollama-models";
const LLAMACPP_VOLUME: &str = "ragrig-llamacpp-models";
const OLLAMA_DATA_DIR: &str = "/root/.ollama";
const LLAMACPP_DATA_DIR: &str = "/models";

/// One managed external service.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ServiceKind {
    /// GROBID reference extraction.
    #[default]
    Grobid,
    /// Ollama chat and embedding models.
    Ollama,
    /// A llama.cpp `llama-server`.
    LlamaCpp,
}

impl ServiceKind {
    /// Every service, in settings-page order.
    pub const ALL: [Self; 3] = [Self::Grobid, Self::Ollama, Self::LlamaCpp];

    /// Stable index for the per-service state array.
    pub fn index(self) -> usize {
        match self {
            Self::Grobid => 0,
            Self::Ollama => 1,
            Self::LlamaCpp => 2,
        }
    }

    /// Human-readable name.
    pub fn label(self) -> &'static str {
        match self {
            Self::Grobid => "GROBID",
            Self::Ollama => "Ollama",
            Self::LlamaCpp => "llama.cpp",
        }
    }

    /// Docker container name.
    pub fn container(self) -> &'static str {
        match self {
            Self::Grobid => MANAGED_GROBID_CONTAINER,
            Self::Ollama => MANAGED_OLLAMA_CONTAINER,
            Self::LlamaCpp => MANAGED_LLAMACPP_CONTAINER,
        }
    }

    /// Host port published on loopback.
    pub fn port(self) -> u16 {
        match self {
            Self::Grobid => MANAGED_GROBID_PORT,
            Self::Ollama => MANAGED_OLLAMA_PORT,
            Self::LlamaCpp => MANAGED_LLAMACPP_PORT,
        }
    }

    /// URL of the managed container.
    pub fn managed_url(self) -> &'static str {
        match self {
            Self::Grobid => MANAGED_GROBID_URL,
            Self::Ollama => MANAGED_OLLAMA_URL,
            Self::LlamaCpp => MANAGED_LLAMACPP_URL,
        }
    }

    /// Default Docker image.
    pub fn default_image(self) -> &'static str {
        match self {
            Self::Grobid => crate::rag::DEFAULT_GROBID_IMAGE,
            Self::Ollama => crate::rag::DEFAULT_OLLAMA_IMAGE,
            Self::LlamaCpp => crate::rag::DEFAULT_LLAMACPP_IMAGE,
        }
    }

    /// Ports checked by **Discover on localhost**.
    pub fn discovery_ports(self) -> &'static [u16] {
        match self {
            Self::Grobid => &GROBID_DISCOVERY_PORTS,
            Self::Ollama => &OLLAMA_DISCOVERY_PORTS,
            Self::LlamaCpp => &LLAMACPP_DISCOVERY_PORTS,
        }
    }

    /// How long startup may take before it is reported as a timeout.
    ///
    /// A managed llama.cpp server may download its GGUF model before it starts
    /// answering `/health`, so it gets a longer budget.
    pub fn ready_timeout(self) -> Duration {
        match self {
            Self::Grobid => Duration::from_secs(180),
            Self::Ollama => Duration::from_secs(120),
            Self::LlamaCpp => Duration::from_secs(600),
        }
    }

    /// Docker platform override needed by the image, if any.
    pub fn managed_platform(self, image: &str) -> Option<&'static str> {
        match self {
            Self::Grobid => image.trim().ends_with("-full").then_some("linux/amd64"),
            Self::Ollama | Self::LlamaCpp => None,
        }
    }
}

/// Image and run arguments for one start request.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ServiceLaunch {
    /// Docker image to pull and run.
    pub image: String,
    /// Managed llama.cpp model (Hugging Face repo or absolute GGUF path).
    pub model: String,
    /// Extra managed llama.cpp arguments.
    pub args: String,
}

impl ServiceLaunch {
    /// Read the launch options for `kind` from the editable settings.
    pub fn from_settings(settings: &RagSettings, kind: ServiceKind) -> Self {
        let image = match kind {
            ServiceKind::Grobid => settings.grobid_image.clone(),
            ServiceKind::Ollama => settings.ollama_image.clone(),
            ServiceKind::LlamaCpp => settings.llamacpp_image.clone(),
        };
        Self {
            image: image.trim().to_owned(),
            model: settings.llamacpp_model.trim().to_owned(),
            args: settings.llamacpp_args.trim().to_owned(),
        }
    }

    /// A user-facing message when the launch options are incomplete.
    pub fn missing_field(&self, kind: ServiceKind) -> Option<&'static str> {
        if self.image.trim().is_empty() {
            return Some("enter a Docker image first");
        }
        if kind == ServiceKind::LlamaCpp && self.model.trim().is_empty() {
            return Some("enter a Hugging Face model or a .gguf file first");
        }
        None
    }
}

/// The models a managed Ollama container should hold for the current settings.
pub fn ollama_models(settings: &RagSettings) -> Vec<String> {
    let mut models = Vec::new();
    let mut push = |model: &str| {
        let model = model.trim();
        if !model.is_empty() && !models.iter().any(|known| known == model) {
            models.push(model.to_owned());
        }
    };
    if settings.provider == ChatProvider::Ollama {
        push(settings.effective_model());
    }
    if settings.hybrid {
        push(&settings.embed_model);
    }
    if settings.memory.uses_model() {
        push(&settings.memory_model);
    }
    models
}

/// The base URL of a server that is already running.
pub fn existing_url(kind: ServiceKind, settings: &RagSettings) -> String {
    match kind {
        ServiceKind::Grobid => settings.grobid_url.clone(),
        ServiceKind::Ollama => MANAGED_OLLAMA_URL.to_owned(),
        ServiceKind::LlamaCpp => settings.llamacpp_url.clone(),
    }
}

/// A container body for `kind`.
///
/// `launch.model` is only used for llama.cpp. Relative `owner/repo[:quant]`
/// values are passed to `-hf`; absolute paths are mounted read-only at
/// `/models`.
pub fn container_config(
    kind: ServiceKind,
    launch: &ServiceLaunch,
) -> Result<ContainerCreateBody, String> {
    match kind {
        ServiceKind::Grobid => Ok(grobid_config(launch)),
        ServiceKind::Ollama => Ok(ollama_config(launch)),
        ServiceKind::LlamaCpp => llamacpp_config(launch),
    }
}

/// Whether an existing container was created with the current launch options.
///
/// A mismatch lets **Pull and start** recreate the container instead of
/// silently running an old image or model.
pub fn container_matches(
    kind: ServiceKind,
    launch: &ServiceLaunch,
    info: &ContainerInspectResponse,
) -> bool {
    let Ok(desired) = container_config(kind, launch) else {
        return false;
    };
    let Some(config) = info.config.as_ref() else {
        return false;
    };
    if config.image.as_deref() != desired.image.as_deref() {
        return false;
    }
    if kind == ServiceKind::LlamaCpp && config.cmd != desired.cmd {
        return false;
    }
    let desired_mounts = desired
        .host_config
        .as_ref()
        .and_then(|host| host.mounts.as_deref())
        .unwrap_or(&[]);
    let actual_mounts = info
        .host_config
        .as_ref()
        .and_then(|host| host.mounts.as_deref())
        .unwrap_or(&[]);
    if desired_mounts.len() != actual_mounts.len() {
        return false;
    }
    desired_mounts.iter().all(|desired| {
        actual_mounts.iter().any(|actual| {
            actual.source == desired.source
                && actual.target == desired.target
                && actual.typ == desired.typ
        })
    })
}

fn base_config(kind: ServiceKind, image: &str) -> ContainerCreateBody {
    let port = kind.port();
    let mut port_bindings = HashMap::new();
    port_bindings.insert(
        format!("{port}/tcp"),
        Some(vec![PortBinding {
            host_ip: Some("127.0.0.1".to_owned()),
            host_port: Some(port.to_string()),
        }]),
    );
    let mut labels = HashMap::new();
    labels.insert(CONTAINER_LABEL.to_owned(), "true".to_owned());
    labels.insert(
        SERVICE_LABEL.to_owned(),
        match kind {
            ServiceKind::Grobid => "grobid",
            ServiceKind::Ollama => "ollama",
            ServiceKind::LlamaCpp => "llamacpp",
        }
        .to_owned(),
    );

    ContainerCreateBody {
        image: Some(image.to_owned()),
        exposed_ports: Some(vec![format!("{port}/tcp")]),
        labels: Some(labels),
        host_config: Some(HostConfig {
            init: Some(true),
            port_bindings: Some(port_bindings),
            auto_remove: Some(false),
            ..HostConfig::default()
        }),
        ..ContainerCreateBody::default()
    }
}

fn grobid_config(launch: &ServiceLaunch) -> ContainerCreateBody {
    let mut config = base_config(ServiceKind::Grobid, &launch.image);
    if let Some(host_config) = config.host_config.as_mut() {
        host_config.ulimits = Some(vec![ResourcesUlimits {
            name: Some("core".to_owned()),
            soft: Some(0),
            hard: Some(0),
        }]);
    }
    config
}

fn ollama_config(launch: &ServiceLaunch) -> ContainerCreateBody {
    let mut config = base_config(ServiceKind::Ollama, &launch.image);
    if let Some(host_config) = config.host_config.as_mut() {
        host_config.mounts = Some(vec![Mount {
            target: Some(OLLAMA_DATA_DIR.to_owned()),
            source: Some(OLLAMA_VOLUME.to_owned()),
            typ: Some(MountType::VOLUME),
            ..Mount::default()
        }]);
    }
    config
}

fn llamacpp_config(launch: &ServiceLaunch) -> Result<ContainerCreateBody, String> {
    let run = llamacpp_command(&launch.model, &launch.args)?;
    let mut config = base_config(ServiceKind::LlamaCpp, &launch.image);
    config.cmd = Some(run.command);
    if !run.env.is_empty() {
        config.env = Some(run.env);
    }
    if let Some(host_config) = config.host_config.as_mut() {
        host_config.mounts = Some(run.mounts);
    }
    Ok(config)
}

struct LlamaCppRun {
    command: Vec<String>,
    mounts: Vec<Mount>,
    env: Vec<String>,
}

fn llamacpp_command(model: &str, args: &str) -> Result<LlamaCppRun, String> {
    let model = model.trim();
    if model.is_empty() {
        return Err("enter a Hugging Face model or a .gguf file".to_owned());
    }

    let mut command = vec![
        "--host".to_owned(),
        "0.0.0.0".to_owned(),
        "--port".to_owned(),
        ServiceKind::LlamaCpp.port().to_string(),
    ];
    let mut mounts = Vec::new();
    let mut env = Vec::new();

    let path = Path::new(model);
    if path.is_absolute() {
        let file = path
            .file_name()
            .ok_or_else(|| "the model path has no file name".to_owned())?;
        command.push("-m".to_owned());
        command.push(format!("/models/{}", file.to_string_lossy()));
        mounts.push(Mount {
            target: Some(format!("/models/{}", file.to_string_lossy())),
            source: Some(path.to_string_lossy().into_owned()),
            typ: Some(MountType::BIND),
            read_only: Some(true),
            ..Mount::default()
        });
    } else {
        command.push("-hf".to_owned());
        command.push(model.to_owned());
        mounts.push(Mount {
            target: Some(LLAMACPP_DATA_DIR.to_owned()),
            source: Some(LLAMACPP_VOLUME.to_owned()),
            typ: Some(MountType::VOLUME),
            ..Mount::default()
        });
        mounts.push(Mount {
            target: Some("/root/.cache/llama.cpp".to_owned()),
            source: Some(LLAMACPP_VOLUME.to_owned()),
            typ: Some(MountType::VOLUME),
            ..Mount::default()
        });
        env.push(format!("LLAMA_CACHE={LLAMACPP_DATA_DIR}"));
    }

    for argument in args.split_whitespace() {
        command.push(argument.to_owned());
    }

    Ok(LlamaCppRun {
        command,
        mounts,
        env,
    })
}

/// Result of probing one endpoint.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Probe {
    /// Normalized base URL that answered.
    pub url: String,
    /// Version reported by the service, when available.
    pub version: Option<String>,
}

/// Probe `url` as `kind`.
pub async fn probe(
    client: &Client,
    kind: ServiceKind,
    url: &str,
    timeout: Duration,
) -> Result<Probe, String> {
    match kind {
        ServiceKind::Grobid => probe_grobid(client, url, timeout).await,
        ServiceKind::Ollama => probe_ollama(client, url, timeout).await,
        ServiceKind::LlamaCpp => probe_llamacpp(client, url, timeout).await,
    }
}

async fn probe_grobid(client: &Client, url: &str, timeout: Duration) -> Result<Probe, String> {
    let url = normalize_http_url(url)?;
    let response = client
        .get(format!("{url}/api/isalive"))
        .timeout(timeout)
        .send()
        .await
        .map_err(probe_error)?;
    if !response.status().is_success() {
        return Err(format!("HTTP {}", response.status()));
    }
    let body = response.text().await.map_err(probe_error)?;
    if !body.trim().eq_ignore_ascii_case("true") {
        return Err("the endpoint did not answer /api/isalive".to_owned());
    }

    let version = match client
        .get(format!("{url}/api/version"))
        .timeout(timeout)
        .send()
        .await
    {
        Ok(response) => match response.error_for_status() {
            Ok(response) => response
                .json::<serde_json::Value>()
                .await
                .ok()
                .and_then(|body| {
                    body.get("version")
                        .and_then(|value| value.as_str())
                        .map(str::to_owned)
                }),
            Err(_) => None,
        },
        Err(_) => None,
    };

    Ok(Probe { url, version })
}

async fn probe_ollama(client: &Client, url: &str, timeout: Duration) -> Result<Probe, String> {
    let url = normalize_http_url(url)?;
    let response = client
        .get(format!("{url}/api/version"))
        .timeout(timeout)
        .send()
        .await
        .map_err(probe_error)?;
    if !response.status().is_success() {
        return Err(format!("HTTP {}", response.status()));
    }
    let version = response
        .json::<serde_json::Value>()
        .await
        .ok()
        .and_then(|body| {
            body.get("version")
                .and_then(|value| value.as_str())
                .map(str::to_owned)
        })
        .ok_or_else(|| "the endpoint did not answer /api/version".to_owned())?;
    Ok(Probe {
        url,
        version: Some(version),
    })
}

async fn probe_llamacpp(client: &Client, url: &str, timeout: Duration) -> Result<Probe, String> {
    let base = llama_base_url(url)?;
    let response = client
        .get(format!("{base}/health"))
        .timeout(timeout)
        .send()
        .await
        .map_err(probe_error)?;
    if !response.status().is_success() {
        return Err(
            if response.status() == reqwest::StatusCode::SERVICE_UNAVAILABLE {
                "the model is still loading".to_owned()
            } else {
                format!("HTTP {}", response.status())
            },
        );
    }
    let health = response
        .json::<serde_json::Value>()
        .await
        .map_err(|_| "the endpoint did not answer /health".to_owned())?;
    if health.get("status").and_then(|value| value.as_str()) != Some("ok") {
        return Err("the model is still loading".to_owned());
    }
    let version = match client
        .get(format!("{base}/props"))
        .timeout(timeout)
        .send()
        .await
    {
        Ok(response) => match response.error_for_status() {
            Ok(response) => response
                .json::<serde_json::Value>()
                .await
                .ok()
                .and_then(|body| {
                    body.get("build_info")
                        .and_then(|value| value.as_str())
                        .map(str::to_owned)
                }),
            Err(_) => None,
        },
        Err(_) => None,
    };
    Ok(Probe {
        url: format!("{base}/v1"),
        version,
    })
}

/// Strip the OpenAI-compatible `/v1` suffix for llama.cpp's own endpoints.
pub fn llama_base_url(input: &str) -> Result<String, String> {
    let url = normalize_http_url(input)?;
    let url = url
        .strip_suffix("/v1/chat/completions")
        .or_else(|| url.strip_suffix("/chat/completions"))
        .unwrap_or(&url);
    let url = url.strip_suffix("/v1").unwrap_or(url);
    Ok(url.trim_end_matches('/').to_owned())
}

fn probe_error(error: reqwest::Error) -> String {
    if error.is_connect() {
        "connection refused".to_owned()
    } else if error.is_timeout() {
        "timed out".to_owned()
    } else {
        error.to_string()
    }
}

/// Normalize user input into the URL shape used by `kind`.
pub fn normalize_url(kind: ServiceKind, input: &str) -> Result<String, String> {
    match kind {
        ServiceKind::Grobid => normalize_grobid_url(input),
        ServiceKind::Ollama => normalize_http_url(input),
        ServiceKind::LlamaCpp => llama_base_url(input).map(|url| format!("{url}/v1")),
    }
}

/// Normalize user input into a GROBID base URL.
///
/// A missing scheme becomes `http://`; whitespace and trailing slashes are
/// removed. Other schemes are rejected.
pub fn normalize_grobid_url(input: &str) -> Result<String, String> {
    let trimmed = input.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return Err("enter the GROBID server URL".to_owned());
    }
    if trimmed.split_whitespace().count() != 1 {
        return Err("the GROBID URL must not contain spaces".to_owned());
    }
    normalize_http_url(trimmed)
}

/// Normalize user input into an HTTP base URL.
///
/// A missing scheme becomes `http://`; whitespace and trailing slashes are
/// removed. Other schemes are rejected.
pub fn normalize_http_url(input: &str) -> Result<String, String> {
    let trimmed = input.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return Err("enter the server URL".to_owned());
    }
    if trimmed.split_whitespace().count() != 1 {
        return Err("the server URL must not contain spaces".to_owned());
    }
    let candidate = if trimmed.contains("://") {
        trimmed.to_owned()
    } else {
        format!("http://{trimmed}")
    };
    let lower = candidate.to_ascii_lowercase();
    if !lower.starts_with("http://") && !lower.starts_with("https://") {
        return Err("the server URL must start with http:// or https://".to_owned());
    }
    Ok(candidate.trim_end_matches('/').to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rag::{DEFAULT_GROBID_IMAGE, DEFAULT_LLAMACPP_IMAGE, DEFAULT_OLLAMA_IMAGE};

    #[test]
    fn urls_gain_a_scheme_and_lose_trailing_slashes() {
        assert_eq!(
            normalize_grobid_url(" localhost:8070/ ").expect("url"),
            "http://localhost:8070"
        );
        assert_eq!(
            normalize_grobid_url("https://example.org/grobid/").expect("url"),
            "https://example.org/grobid"
        );
    }

    #[test]
    fn empty_and_unsupported_urls_are_rejected() {
        assert!(normalize_grobid_url("   ").is_err());
        assert!(normalize_grobid_url("ftp://localhost:8070").is_err());
        assert!(normalize_grobid_url("not a url").is_err());
    }

    #[test]
    fn llamacpp_urls_keep_one_v1_suffix() {
        assert_eq!(
            normalize_url(ServiceKind::LlamaCpp, "localhost:8080").expect("url"),
            "http://localhost:8080/v1"
        );
        assert_eq!(
            normalize_url(ServiceKind::LlamaCpp, "http://localhost:8080/v1/").expect("url"),
            "http://localhost:8080/v1"
        );
        assert_eq!(
            llama_base_url("http://localhost:8080/v1").expect("url"),
            "http://localhost:8080"
        );
        assert_eq!(
            llama_base_url("http://localhost:8080/v1/chat/completions").expect("url"),
            "http://localhost:8080"
        );
    }

    #[test]
    fn managed_containers_use_their_own_loopback_ports() {
        for (kind, port) in [
            (ServiceKind::Grobid, 8070),
            (ServiceKind::Ollama, 11434),
            (ServiceKind::LlamaCpp, 8080),
        ] {
            let launch = ServiceLaunch {
                image: kind.default_image().to_owned(),
                model: "owner/model:Q4_K_M".to_owned(),
                args: String::new(),
            };
            let config = container_config(kind, &launch).expect("config");
            let host = config.host_config.expect("host config");
            let binding = host
                .port_bindings
                .as_ref()
                .and_then(|bindings| bindings.get(&format!("{port}/tcp")))
                .and_then(|binding| binding.as_ref())
                .and_then(|binding| binding.first())
                .expect("port binding");
            assert_eq!(binding.host_ip.as_deref(), Some("127.0.0.1"));
            assert_eq!(binding.host_port, Some(port.to_string()));
            assert_eq!(
                config
                    .labels
                    .as_ref()
                    .and_then(|labels| labels.get("io.ragrig.service"))
                    .map(String::as_str),
                Some(match kind {
                    ServiceKind::Grobid => "grobid",
                    ServiceKind::Ollama => "ollama",
                    ServiceKind::LlamaCpp => "llamacpp",
                })
            );
        }
    }

    #[test]
    fn default_images_match_the_official_names() {
        assert_eq!(DEFAULT_GROBID_IMAGE, "grobid/grobid:0.9.1-full");
        assert_eq!(DEFAULT_OLLAMA_IMAGE, "ollama/ollama:latest");
        assert_eq!(DEFAULT_LLAMACPP_IMAGE, "ghcr.io/ggml-org/llama.cpp:server");
        assert_eq!(
            ServiceKind::Grobid.managed_platform("grobid/grobid:0.9.1-full"),
            Some("linux/amd64")
        );
        assert_eq!(
            ServiceKind::Grobid.managed_platform("grobid/grobid:0.9.1-crf"),
            None
        );
    }

    #[test]
    fn ollama_keeps_models_in_a_named_volume() {
        let launch = ServiceLaunch {
            image: DEFAULT_OLLAMA_IMAGE.to_owned(),
            ..ServiceLaunch::default()
        };
        let config = container_config(ServiceKind::Ollama, &launch).expect("config");
        let mounts = config
            .host_config
            .and_then(|host| host.mounts)
            .expect("mounts");
        assert_eq!(mounts.len(), 1);
        assert_eq!(mounts[0].target.as_deref(), Some("/root/.ollama"));
        assert_eq!(mounts[0].source.as_deref(), Some("ragrig-ollama-models"));
        assert_eq!(mounts[0].typ, Some(MountType::VOLUME));
    }

    #[test]
    fn llamacpp_downloads_huggingface_models_into_the_cache_volume() {
        let launch = ServiceLaunch {
            image: DEFAULT_LLAMACPP_IMAGE.to_owned(),
            model: "owner/model:Q4_K_M".to_owned(),
            args: "--n-gpu-layers all".to_owned(),
        };
        let config = container_config(ServiceKind::LlamaCpp, &launch).expect("config");
        let command = config.cmd.expect("command");
        assert!(
            command
                .windows(2)
                .any(|pair| pair[0] == "-hf" && pair[1] == "owner/model:Q4_K_M")
        );
        assert!(command.iter().any(|argument| argument == "--n-gpu-layers"));
        assert!(command.iter().any(|argument| argument == "all"));
        let host = config.host_config.expect("host config");
        assert_eq!(host.mounts.as_ref().map(Vec::len), Some(2));
        assert!(host.mounts.as_ref().is_some_and(|mounts| {
            mounts
                .iter()
                .all(|mount| mount.source.as_deref() == Some("ragrig-llamacpp-models"))
        }));
        assert!(host.mounts.as_ref().is_some_and(|mounts| {
            mounts
                .iter()
                .any(|mount| mount.target.as_deref() == Some("/root/.cache/llama.cpp"))
        }));
        assert_eq!(config.env, Some(vec!["LLAMA_CACHE=/models".to_owned()]));
    }

    #[test]
    fn llamacpp_mounts_an_absolute_model_file_read_only() {
        let launch = ServiceLaunch {
            image: DEFAULT_LLAMACPP_IMAGE.to_owned(),
            model: "/home/me/models/qwen.gguf".to_owned(),
            args: String::new(),
        };
        let config = container_config(ServiceKind::LlamaCpp, &launch).expect("config");
        let command = config.cmd.expect("command");
        assert!(
            command
                .windows(2)
                .any(|pair| pair[0] == "-m" && pair[1] == "/models/qwen.gguf")
        );
        let mounts = config
            .host_config
            .and_then(|host| host.mounts)
            .expect("mounts");
        assert_eq!(
            mounts[0].source.as_deref(),
            Some("/home/me/models/qwen.gguf")
        );
        assert_eq!(mounts[0].target.as_deref(), Some("/models/qwen.gguf"));
        assert_eq!(mounts[0].read_only, Some(true));
    }

    #[test]
    fn changed_images_or_mounts_trigger_recreation() {
        let launch = ServiceLaunch {
            image: DEFAULT_LLAMACPP_IMAGE.to_owned(),
            model: "/home/me/models/qwen.gguf".to_owned(),
            args: String::new(),
        };
        let desired = container_config(ServiceKind::LlamaCpp, &launch).expect("config");
        let info = ContainerInspectResponse {
            config: Some(bollard::models::ContainerConfig {
                image: desired.image.clone(),
                cmd: desired.cmd.clone(),
                ..Default::default()
            }),
            host_config: desired.host_config.clone(),
            ..Default::default()
        };
        assert!(container_matches(ServiceKind::LlamaCpp, &launch, &info));

        let mut changed_image = info.clone();
        changed_image.config.as_mut().expect("config").image =
            Some("ghcr.io/ggml-org/llama.cpp:server-cuda".to_owned());
        assert!(!container_matches(
            ServiceKind::LlamaCpp,
            &launch,
            &changed_image
        ));

        let mut other_model = launch.clone();
        other_model.model = "/home/me/other/qwen.gguf".to_owned();
        assert!(!container_matches(
            ServiceKind::LlamaCpp,
            &other_model,
            &info
        ));
    }

    #[test]
    fn ollama_models_follow_the_configured_roles() {
        let mut settings = RagSettings::default();
        settings.provider = ChatProvider::Ollama;
        settings.hybrid = true;
        settings.embed_model = "nomic-embed-text".to_owned();
        settings.memory_model = settings.effective_model().to_owned();
        let models = ollama_models(&settings);
        assert_eq!(
            models.first().map(String::as_str),
            Some(settings.effective_model())
        );
        assert!(models.iter().any(|model| model == "nomic-embed-text"));
        assert_eq!(
            models
                .iter()
                .filter(|model| *model == settings.effective_model())
                .count(),
            1
        );
    }

    /// Serve one HTTP response per accepted connection and return the base URL.
    fn serve(responses: Vec<String>) -> String {
        use std::io::{Read as _, Write as _};

        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let address = listener.local_addr().expect("address");
        std::thread::spawn(move || {
            for response in responses {
                let Ok((mut socket, _)) = listener.accept() else {
                    return;
                };
                let mut request = [0u8; 2048];
                let _ = socket.read(&mut request);
                let _ = socket.write_all(response.as_bytes());
            }
        });
        format!("http://{address}")
    }

    fn response(body: &str) -> String {
        format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
    }

    #[tokio::test]
    async fn ollama_reports_its_version() {
        let base = serve(vec![response(r#"{"version":"0.6.2"}"#)]);
        let client = Client::builder()
            .timeout(Duration::from_secs(2))
            .build()
            .expect("client");

        let probe = probe(&client, ServiceKind::Ollama, &base, Duration::from_secs(2))
            .await
            .expect("probe");

        assert_eq!(probe.url, base);
        assert_eq!(probe.version.as_deref(), Some("0.6.2"));
    }

    #[tokio::test]
    async fn llamacpp_reports_a_loaded_model_and_its_build() {
        let base = serve(vec![
            response(r#"{"status":"ok"}"#),
            response(r#"{"build_info":"b1234-abc"}"#),
        ]);
        let client = Client::builder()
            .timeout(Duration::from_secs(2))
            .build()
            .expect("client");

        let probe = probe(
            &client,
            ServiceKind::LlamaCpp,
            &format!("{base}/v1"),
            Duration::from_secs(2),
        )
        .await
        .expect("probe");

        assert_eq!(probe.url, format!("{base}/v1"));
        assert_eq!(probe.version.as_deref(), Some("b1234-abc"));
    }

    #[tokio::test]
    async fn grobid_reports_its_version() {
        let base = serve(vec![response("true"), response(r#"{"version":"0.9.1"}"#)]);
        let client = Client::builder()
            .timeout(Duration::from_secs(2))
            .build()
            .expect("client");

        let probe = probe(&client, ServiceKind::Grobid, &base, Duration::from_secs(2))
            .await
            .expect("probe");

        assert_eq!(probe.url, base);
        assert_eq!(probe.version.as_deref(), Some("0.9.1"));
    }
}
