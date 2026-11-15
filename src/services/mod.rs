//! External service management for the split settings pages.
//!
//! Available with the `managed` feature; the GROBID-specific names stay
//! available when the `grobid` feature is enabled. Each service is a
//! [`SettingGroup`](gpui_kit::component::setting::SettingGroup) that a host can
//! place on the pane that uses it; there is no separate services page.

mod managed;
mod settings_page;
mod spec;

pub use managed::{
    ContainerStatus, DockerStatus, ExternalServiceEvent, ExternalServices, ProbeStatus,
    docker_message,
};
#[cfg(feature = "grobid")]
pub use managed::{
    GrobidContainerStatus, GrobidDockerStatus, GrobidProbe, GrobidProbeStatus, GrobidService,
    GrobidServiceEvent,
};
#[cfg(feature = "grobid")]
pub use settings_page::grobid_group;
pub use settings_page::{docker_group, llamacpp_group, ollama_group};
pub use spec::{
    GROBID_DISCOVERY_PORTS, LLAMACPP_DISCOVERY_PORTS, MANAGED_GROBID_CONTAINER,
    MANAGED_GROBID_PORT, MANAGED_GROBID_URL, MANAGED_LLAMACPP_CONTAINER, MANAGED_LLAMACPP_PORT,
    MANAGED_LLAMACPP_URL, MANAGED_OLLAMA_CONTAINER, MANAGED_OLLAMA_PORT, MANAGED_OLLAMA_URL,
    OLLAMA_DISCOVERY_PORTS, Probe, ServiceKind, ServiceLaunch, existing_url, llama_base_url,
    normalize_grobid_url, normalize_http_url, normalize_url, ollama_models, probe,
};
