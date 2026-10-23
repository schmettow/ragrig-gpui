//! External service management for the settings page.
//!
//! Available with the `grobid` feature.

mod grobid;
mod settings_page;

pub use grobid::{
    GROBID_DISCOVERY_PORTS, GrobidContainerStatus, GrobidDockerStatus, GrobidProbe,
    GrobidProbeStatus, GrobidService, GrobidServiceEvent, MANAGED_GROBID_CONTAINER,
    MANAGED_GROBID_PORT, MANAGED_GROBID_URL, normalize_grobid_url,
};
pub use settings_page::external_services_page;
