//! External service management for the settings page.
//!
//! Available with the `managed` feature; the GROBID-specific names stay
//! available when the `grobid` feature is enabled.

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
pub use settings_page::external_services_page;
pub use spec::{
    GROBID_DISCOVERY_PORTS, LLAMACPP_DISCOVERY_PORTS, MANAGED_GROBID_CONTAINER,
    MANAGED_GROBID_PORT, MANAGED_GROBID_URL, MANAGED_LLAMACPP_CONTAINER, MANAGED_LLAMACPP_PORT,
    MANAGED_LLAMACPP_URL, MANAGED_OLLAMA_CONTAINER, MANAGED_OLLAMA_PORT, MANAGED_OLLAMA_URL,
    OLLAMA_DISCOVERY_PORTS, Probe, ServiceKind, ServiceLaunch, existing_url, llama_base_url,
    normalize_grobid_url, normalize_http_url, normalize_url, ollama_models, probe,
};
