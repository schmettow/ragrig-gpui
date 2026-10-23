//! Managed GROBID service.
//!
//! [`GrobidService`] owns the Docker lifecycle of the container created by the
//! **External services** settings page. It also probes existing servers, so a
//! host can switch between a local container and a server the user already
//! runs. The controller mirrors every state change for the settings page and
//! emits [`GrobidServiceEvent`]s for hosts that persist the URL.
//!
//! All Docker and HTTP work happens on a dedicated Tokio runtime thread. Do not
//! drive these futures from GPUI's executor.

use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use async_channel::{Sender, TrySendError};
use bollard::{
    Docker,
    models::{ContainerCreateBody, HostConfig, PortBinding, ResourcesUlimits},
    query_parameters::{
        CreateContainerOptionsBuilder, CreateImageOptionsBuilder, RemoveContainerOptionsBuilder,
        StopContainerOptionsBuilder,
    },
};
use futures_util::StreamExt;
use gpui_kit::{Context, Entity, EventEmitter};
use reqwest::Client;

use crate::rag::{GrobidSource, RagSettings};

/// Container name used for the managed GROBID container.
pub const MANAGED_GROBID_CONTAINER: &str = "ragrig-grobid";

/// Loopback URL exposed by the managed GROBID container.
pub const MANAGED_GROBID_URL: &str = "http://127.0.0.1:8070";

/// Host port exposed by the managed GROBID container.
pub const MANAGED_GROBID_PORT: u16 = 8070;

/// Ports probed by **Discover on localhost**.
pub const GROBID_DISCOVERY_PORTS: [u16; 2] = [8070, 8080];

const CONTAINER_LABEL: &str = "io.ragrig.managed";
const SERVICE_LABEL: &str = "io.ragrig.service";
const READY_TIMEOUT: Duration = Duration::from_secs(180);
const CANCELLED: &str = "cancelled";

/// Whether Docker is reachable from this process.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum GrobidDockerStatus {
    /// Docker has not been checked yet.
    #[default]
    Unknown,
    /// The daemon is being contacted.
    Checking,
    /// The daemon answered and reported its version.
    Available {
        /// Docker Engine version.
        version: String,
    },
    /// The daemon could not be reached.
    Unavailable {
        /// Human-readable diagnosis and remediation hint.
        message: String,
    },
}

impl GrobidDockerStatus {
    /// One line for the settings page.
    pub fn summary(&self) -> String {
        match self {
            Self::Unknown => "Not checked".to_owned(),
            Self::Checking => "Checking Docker…".to_owned(),
            Self::Available { version } => format!("Docker {version} ready"),
            Self::Unavailable { message } => message.clone(),
        }
    }
}

/// Lifecycle of the managed GROBID container.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum GrobidContainerStatus {
    /// The container has not been inspected yet.
    #[default]
    Unknown,
    /// Docker is being inspected.
    Checking,
    /// No container with Scico's name exists.
    Missing,
    /// The managed container exists but is stopped.
    Stopped,
    /// An image pull is running.
    Pulling {
        /// Image reference being pulled.
        image: String,
        /// Bytes downloaded across all layers so far.
        received: u64,
        /// Total bytes across all layers, when Docker reported it.
        total: u64,
    },
    /// The container is starting and its health endpoint is polled.
    Starting,
    /// GROBID answers its health endpoint.
    Running,
    /// The container is being stopped.
    Stopping,
    /// The last operation failed.
    Failed {
        /// Human-readable failure.
        message: String,
    },
}

impl GrobidContainerStatus {
    /// One line for the settings page.
    pub fn summary(&self) -> String {
        match self {
            Self::Unknown => "Not checked".to_owned(),
            Self::Checking => "Checking the container…".to_owned(),
            Self::Missing => "Container not created".to_owned(),
            Self::Stopped => "Container stopped".to_owned(),
            Self::Pulling {
                image,
                received,
                total,
            } if *total > 0 => {
                let percent = (*received as f64 / *total as f64 * 100.0).clamp(0.0, 100.0);
                format!(
                    "Pulling {image} — {percent:.0}% ({} of {})",
                    format_bytes(*received),
                    format_bytes(*total)
                )
            }
            Self::Pulling {
                image, received, ..
            } => {
                format!("Pulling {image} — {}", format_bytes(*received))
            }
            Self::Starting => "Starting GROBID…".to_owned(),
            Self::Running => "Running".to_owned(),
            Self::Stopping => "Stopping…".to_owned(),
            Self::Failed { message } => format!("Failed: {message}"),
        }
    }
}

/// Result of probing one GROBID endpoint.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GrobidProbe {
    /// Normalized base URL that answered.
    pub url: String,
    /// Version reported by `/api/version`, when available.
    pub version: Option<String>,
}

/// State of the last endpoint probe.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum GrobidProbeStatus {
    /// No endpoint has been tested.
    #[default]
    Idle,
    /// A probe is running.
    Checking {
        /// Endpoint being tested.
        url: String,
    },
    /// The endpoint answered `/api/isalive`.
    Reachable {
        /// Normalized endpoint that answered.
        url: String,
        /// Version reported by `/api/version`, when available.
        version: Option<String>,
    },
    /// The endpoint did not answer as GROBID.
    Unreachable {
        /// Endpoint that was tested.
        url: String,
        /// Human-readable failure.
        message: String,
    },
}

impl GrobidProbeStatus {
    /// One line for the settings page.
    pub fn summary(&self) -> String {
        match self {
            Self::Idle => "Not tested".to_owned(),
            Self::Checking { url } => format!("Testing {url}…"),
            Self::Reachable { url, version } => match version {
                Some(version) => format!("GROBID {version} at {url}"),
                None => format!("GROBID reachable at {url}"),
            },
            Self::Unreachable { url, message } => format!("{url}: {message}"),
        }
    }
}

/// Changes a host may want to observe.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GrobidServiceEvent {
    /// Docker availability changed.
    Docker(GrobidDockerStatus),
    /// Managed container lifecycle changed.
    Container(GrobidContainerStatus),
    /// Endpoint probe changed.
    Probe(GrobidProbeStatus),
    /// Localhost discovery found one or more servers.
    Discovered(Vec<GrobidProbe>),
    /// A managed container or an adopted server is ready.
    Ready {
        /// Base URL that answered.
        url: String,
        /// Version reported by `/api/version`, when available.
        version: Option<String>,
        /// Whether the app created or started the container.
        managed: bool,
    },
    /// The configured GROBID URL changed.
    UrlChanged(String),
    /// An operation failed with a user-facing message.
    Failed {
        /// Human-readable failure.
        message: String,
    },
}

enum GrobidCommand {
    Inspect { adopt: bool },
    Probe { url: String },
    Discover,
    PullAndStart { image: String },
    Stop,
    Remove,
}

/// Owns the managed GROBID container and probes existing servers.
///
/// Dropping the entity stops a container this service created or adopted in
/// managed mode. A GROBID server that is not a managed container is never
/// touched.
pub struct GrobidService {
    commands: Sender<GrobidCommand>,
    worker: Option<thread::JoinHandle<()>>,
    cancel: Arc<AtomicBool>,
    settings: Entity<RagSettings>,
    docker: GrobidDockerStatus,
    container: GrobidContainerStatus,
    probe: GrobidProbeStatus,
    discovered: Vec<GrobidProbe>,
    notice: Option<String>,
}

impl GrobidService {
    /// Spawn the service worker and start inspecting Docker and the endpoint.
    pub fn new(settings: Entity<RagSettings>, cx: &mut Context<Self>) -> Result<Self, String> {
        let initial = settings.read(cx).clone();
        let (commands, command_rx) = async_channel::bounded(16);
        let (events_tx, events) = async_channel::bounded(128);
        let cancel = Arc::new(AtomicBool::new(false));

        // GPUI's test scheduler rejects wakes that arrive from a foreign
        // thread, and starting the worker would probe Docker and localhost on
        // every test. Tests drive `on_event` directly instead.
        let worker = if cx.is_test() {
            None
        } else {
            let worker_cancel = cancel.clone();
            let worker = thread::Builder::new()
                .name("ragrig-grobid".to_owned())
                .spawn(move || {
                    let runtime = tokio::runtime::Builder::new_multi_thread()
                        .worker_threads(2)
                        .enable_all()
                        .build();
                    match runtime {
                        Ok(runtime) => runtime.block_on(run(command_rx, events_tx, worker_cancel)),
                        Err(error) => {
                            let _ = events_tx.send_blocking(GrobidServiceEvent::Failed {
                                message: format!(
                                    "cannot start the GROBID service runtime: {error}"
                                ),
                            });
                        }
                    }
                })
                .map_err(|error| format!("cannot start the GROBID service thread: {error}"))?;

            cx.spawn(async move |this, cx| {
                while let Ok(event) = events.recv().await {
                    if this
                        .update(cx, |service, cx| service.on_event(event, cx))
                        .is_err()
                    {
                        break;
                    }
                }
            })
            .detach();

            let _ = commands.try_send(GrobidCommand::Inspect {
                adopt: initial.grobid_source == GrobidSource::Managed,
            });
            if initial.grobid_source == GrobidSource::Existing {
                let _ = commands.try_send(GrobidCommand::Probe {
                    url: initial.grobid_url,
                });
            }

            Some(worker)
        };

        Ok(Self {
            commands,
            worker,
            cancel,
            settings,
            docker: GrobidDockerStatus::Unknown,
            container: GrobidContainerStatus::Unknown,
            probe: GrobidProbeStatus::Idle,
            discovered: Vec::new(),
            notice: None,
        })
    }

    /// Whether Docker is reachable.
    pub fn docker(&self) -> &GrobidDockerStatus {
        &self.docker
    }

    /// Managed container lifecycle.
    pub fn container(&self) -> &GrobidContainerStatus {
        &self.container
    }

    /// State of the last endpoint probe.
    pub fn probe(&self) -> &GrobidProbeStatus {
        &self.probe
    }

    /// Servers found by the last localhost discovery.
    pub fn discovered(&self) -> &[GrobidProbe] {
        &self.discovered
    }

    /// The last user-facing notice.
    pub fn notice(&self) -> Option<&str> {
        self.notice.as_deref()
    }

    /// Whether a container operation is in flight.
    pub fn is_busy(&self) -> bool {
        matches!(
            &self.container,
            GrobidContainerStatus::Pulling { .. }
                | GrobidContainerStatus::Starting
                | GrobidContainerStatus::Stopping
        )
    }

    /// Whether the running pull or startup can still be cancelled.
    pub fn is_cancellable(&self) -> bool {
        matches!(
            &self.container,
            GrobidContainerStatus::Pulling { .. } | GrobidContainerStatus::Starting
        ) && !self.cancel.load(Ordering::Relaxed)
    }

    /// Whether a cancellation was requested and the worker has not reacted yet.
    pub fn is_cancelling(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
            && matches!(
                &self.container,
                GrobidContainerStatus::Pulling { .. } | GrobidContainerStatus::Starting
            )
    }

    /// Ask the worker to abandon the current pull or startup.
    pub fn cancel(&mut self, cx: &mut Context<Self>) {
        if self.is_cancellable() {
            self.cancel.store(true, Ordering::Relaxed);
            self.notice = Some("cancelling…".to_owned());
            cx.notify();
        }
    }

    /// Re-inspect Docker and the managed container.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let adopt = self.settings.read(cx).grobid_source == GrobidSource::Managed;
        self.send_quiet(GrobidCommand::Inspect { adopt }, cx);
    }

    /// Probe the given URL without changing it until the probe succeeds.
    pub fn test_url(&mut self, url: impl Into<String>, cx: &mut Context<Self>) {
        let url = url.into();
        match normalize_grobid_url(&url) {
            Ok(url) => {
                self.probe = GrobidProbeStatus::Checking { url: url.clone() };
                self.discovered.clear();
                self.notice = None;
                self.send_quiet(GrobidCommand::Probe { url }, cx);
            }
            Err(message) => {
                self.probe = GrobidProbeStatus::Unreachable { url, message };
                cx.notify();
            }
        }
    }

    /// Probe the common localhost ports and use the first server found.
    pub fn discover(&mut self, cx: &mut Context<Self>) {
        self.probe = GrobidProbeStatus::Checking {
            url: "localhost".to_owned(),
        };
        self.discovered.clear();
        self.notice = None;
        self.send_quiet(GrobidCommand::Discover, cx);
    }

    /// Pull `image` when needed, create the container, and wait for health.
    pub fn pull_and_start(&mut self, image: impl Into<String>, cx: &mut Context<Self>) {
        let image = image.into();
        if image.trim().is_empty() {
            self.notice = Some("enter a Docker image first".to_owned());
            cx.notify();
            return;
        }
        self.notice = None;
        self.cancel.store(false, Ordering::Relaxed);
        self.send_quiet(
            GrobidCommand::PullAndStart {
                image: image.trim().to_owned(),
            },
            cx,
        );
    }

    /// Stop the managed container without removing it.
    pub fn stop(&mut self, cx: &mut Context<Self>) {
        self.container = GrobidContainerStatus::Stopping;
        self.send_quiet(GrobidCommand::Stop, cx);
    }

    /// Stop and remove the managed container.
    pub fn remove(&mut self, cx: &mut Context<Self>) {
        self.container = GrobidContainerStatus::Stopping;
        self.send_quiet(GrobidCommand::Remove, cx);
    }

    /// Clear a stale endpoint probe after the URL was edited.
    pub fn clear_probe(&mut self, cx: &mut Context<Self>) {
        if !matches!(&self.probe, GrobidProbeStatus::Idle) || !self.discovered.is_empty() {
            self.probe = GrobidProbeStatus::Idle;
            self.discovered.clear();
            cx.notify();
        }
    }

    fn send_quiet(&mut self, command: GrobidCommand, cx: &mut Context<Self>) {
        if let Err(error) = self.commands.try_send(command) {
            self.notice = Some(match error {
                TrySendError::Full(_) => "the GROBID service is busy; try again".to_owned(),
                TrySendError::Closed(_) => "the GROBID service stopped".to_owned(),
            });
        }
        cx.notify();
    }

    fn on_event(&mut self, event: GrobidServiceEvent, cx: &mut Context<Self>) {
        let mut changed_url = None;
        match &event {
            GrobidServiceEvent::Docker(status) => {
                if matches!(status, GrobidDockerStatus::Available { .. }) {
                    self.notice = None;
                }
                self.docker = status.clone();
            }
            GrobidServiceEvent::Container(status) => {
                if matches!(
                    status,
                    GrobidContainerStatus::Missing
                        | GrobidContainerStatus::Stopped
                        | GrobidContainerStatus::Running
                        | GrobidContainerStatus::Failed { .. }
                ) {
                    self.cancel.store(false, Ordering::Relaxed);
                }
                self.container = status.clone();
            }
            GrobidServiceEvent::Probe(status) => {
                if matches!(status, GrobidProbeStatus::Reachable { .. }) {
                    self.notice = None;
                }
                if let GrobidProbeStatus::Reachable { url, .. } = status {
                    changed_url = Some(url.clone());
                }
                self.probe = status.clone();
            }
            GrobidServiceEvent::Discovered(probes) => {
                self.discovered = probes.clone();
                if let Some(first) = probes.first() {
                    changed_url = Some(first.url.clone());
                    self.settings.update(cx, |settings, cx| {
                        if settings.grobid_source != GrobidSource::Existing {
                            settings.grobid_source = GrobidSource::Existing;
                            cx.notify();
                        }
                    });
                }
            }
            GrobidServiceEvent::Ready {
                url,
                version,
                managed,
            } => {
                self.container = GrobidContainerStatus::Running;
                self.probe = GrobidProbeStatus::Reachable {
                    url: url.clone(),
                    version: version.clone(),
                };
                self.notice = if *managed {
                    None
                } else {
                    Some(format!(
                        "{url} already answers; using it instead of starting a container"
                    ))
                };
                changed_url = Some(url.clone());
                let source = if *managed {
                    GrobidSource::Managed
                } else {
                    GrobidSource::Existing
                };
                self.settings.update(cx, |settings, cx| {
                    if settings.grobid_source != source {
                        settings.grobid_source = source;
                        cx.notify();
                    }
                });
            }
            GrobidServiceEvent::UrlChanged(_) => {}
            GrobidServiceEvent::Failed { message } => {
                self.notice = Some(message.clone());
            }
        }

        if let Some(url) = changed_url {
            let url_for_settings = url.clone();
            self.settings.update(cx, |settings, cx| {
                if settings.grobid_url != url_for_settings {
                    settings.grobid_url = url_for_settings;
                    cx.notify();
                }
            });
            cx.emit(GrobidServiceEvent::UrlChanged(url));
        }
        cx.notify();
        cx.emit(event);
    }
}

impl EventEmitter<GrobidServiceEvent> for GrobidService {}

impl Drop for GrobidService {
    fn drop(&mut self) {
        self.commands.close();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

async fn run(
    commands: async_channel::Receiver<GrobidCommand>,
    events: Sender<GrobidServiceEvent>,
    cancel: Arc<AtomicBool>,
) {
    let client = match Client::builder().timeout(Duration::from_secs(3)).build() {
        Ok(client) => client,
        Err(error) => {
            let _ = events
                .send(GrobidServiceEvent::Failed {
                    message: format!("cannot build the GROBID HTTP client: {error}"),
                })
                .await;
            return;
        }
    };
    let mut owns_container = false;

    while let Ok(command) = commands.recv().await {
        match command {
            GrobidCommand::Inspect { adopt } => {
                let present = inspect(&client, &events, adopt).await;
                if adopt {
                    owns_container = present;
                }
            }
            GrobidCommand::Probe { url } => {
                probe_command(&client, &url, &events).await;
            }
            GrobidCommand::Discover => {
                discover(&client, &events, &commands).await;
            }
            GrobidCommand::PullAndStart { image } => {
                if pull_and_start(&client, &image, &events, &commands, &cancel).await {
                    owns_container = true;
                }
            }
            GrobidCommand::Stop => {
                stop(&events).await;
            }
            GrobidCommand::Remove => {
                if !remove(&events).await {
                    owns_container = false;
                }
            }
        }
    }

    if owns_container && let Ok(docker) = connect_docker_quiet().await {
        let options = StopContainerOptionsBuilder::default().t(5).build();
        let _ = tokio::time::timeout(
            Duration::from_secs(8),
            docker.stop_container(MANAGED_GROBID_CONTAINER, Some(options)),
        )
        .await;
    }
}

async fn inspect(client: &Client, events: &Sender<GrobidServiceEvent>, adopt: bool) -> bool {
    events
        .send(GrobidServiceEvent::Container(
            GrobidContainerStatus::Checking,
        ))
        .await
        .ok();
    let Some(docker) = connect_docker(events).await else {
        return false;
    };
    match docker
        .inspect_container(MANAGED_GROBID_CONTAINER, None)
        .await
    {
        Ok(info) => {
            let running = info
                .state
                .as_ref()
                .and_then(|state| state.running)
                .unwrap_or(false);
            if running {
                events
                    .send(GrobidServiceEvent::Container(
                        GrobidContainerStatus::Running,
                    ))
                    .await
                    .ok();
                if adopt
                    && let Ok(probe) =
                        probe_grobid(client, MANAGED_GROBID_URL, Duration::from_secs(2)).await
                {
                    events
                        .send(GrobidServiceEvent::Ready {
                            url: probe.url,
                            version: probe.version,
                            managed: true,
                        })
                        .await
                        .ok();
                }
            } else {
                events
                    .send(GrobidServiceEvent::Container(
                        GrobidContainerStatus::Stopped,
                    ))
                    .await
                    .ok();
            }
            true
        }
        Err(_) => {
            events
                .send(GrobidServiceEvent::Container(
                    GrobidContainerStatus::Missing,
                ))
                .await
                .ok();
            false
        }
    }
}

async fn probe_command(client: &Client, url: &str, events: &Sender<GrobidServiceEvent>) {
    let normalized = match normalize_grobid_url(url) {
        Ok(url) => url,
        Err(message) => {
            events
                .send(GrobidServiceEvent::Probe(GrobidProbeStatus::Unreachable {
                    url: url.to_owned(),
                    message,
                }))
                .await
                .ok();
            return;
        }
    };
    events
        .send(GrobidServiceEvent::Probe(GrobidProbeStatus::Checking {
            url: normalized.clone(),
        }))
        .await
        .ok();
    let status = match probe_grobid(client, &normalized, Duration::from_secs(3)).await {
        Ok(probe) => GrobidProbeStatus::Reachable {
            url: probe.url,
            version: probe.version,
        },
        Err(message) => GrobidProbeStatus::Unreachable {
            url: normalized,
            message,
        },
    };
    events.send(GrobidServiceEvent::Probe(status)).await.ok();
}

async fn discover(
    client: &Client,
    events: &Sender<GrobidServiceEvent>,
    commands: &async_channel::Receiver<GrobidCommand>,
) {
    events
        .send(GrobidServiceEvent::Probe(GrobidProbeStatus::Checking {
            url: "localhost".to_owned(),
        }))
        .await
        .ok();

    let mut found = Vec::new();
    for port in GROBID_DISCOVERY_PORTS {
        for host in ["127.0.0.1", "localhost"] {
            if commands.is_closed() {
                return;
            }
            let url = format!("http://{host}:{port}");
            if let Ok(probe) = probe_grobid(client, &url, Duration::from_millis(900)).await {
                found.push(probe);
                break;
            }
        }
        if !found.is_empty() {
            break;
        }
    }

    if found.is_empty() {
        events
            .send(GrobidServiceEvent::Probe(GrobidProbeStatus::Unreachable {
                url: "localhost".to_owned(),
                message: "No GROBID server answered on ports 8070 or 8080".to_owned(),
            }))
            .await
            .ok();
        return;
    }

    events
        .send(GrobidServiceEvent::Probe(GrobidProbeStatus::Reachable {
            url: found[0].url.clone(),
            version: found[0].version.clone(),
        }))
        .await
        .ok();
    events
        .send(GrobidServiceEvent::Discovered(found))
        .await
        .ok();
}

async fn pull_and_start(
    client: &Client,
    image: &str,
    events: &Sender<GrobidServiceEvent>,
    commands: &async_channel::Receiver<GrobidCommand>,
    cancel: &AtomicBool,
) -> bool {
    let Some(docker) = connect_docker(events).await else {
        return false;
    };

    let existing = docker
        .inspect_container(MANAGED_GROBID_CONTAINER, None)
        .await
        .ok();
    if let Some(info) = existing {
        let running = info
            .state
            .as_ref()
            .and_then(|state| state.running)
            .unwrap_or(false);
        if !running {
            events
                .send(GrobidServiceEvent::Container(
                    GrobidContainerStatus::Starting,
                ))
                .await
                .ok();
            if let Err(error) = docker.start_container(MANAGED_GROBID_CONTAINER, None).await {
                fail(
                    &format!("cannot start the GROBID container: {error}"),
                    events,
                )
                .await;
                return true;
            }
        }
    } else {
        // Never fight a server that already owns the port. It is not ours, so
        // the service adopts its URL without owning its lifecycle.
        if let Ok(probe) =
            probe_grobid(client, MANAGED_GROBID_URL, Duration::from_millis(900)).await
        {
            events
                .send(GrobidServiceEvent::Ready {
                    url: probe.url,
                    version: probe.version,
                    managed: false,
                })
                .await
                .ok();
            return false;
        }
        if let Err(error) = pull_image(&docker, image, events, commands, cancel).await {
            if error == CANCELLED {
                if !commands.is_closed() {
                    events
                        .send(GrobidServiceEvent::Container(
                            GrobidContainerStatus::Missing,
                        ))
                        .await
                        .ok();
                    events
                        .send(GrobidServiceEvent::Failed {
                            message: "pull cancelled".to_owned(),
                        })
                        .await
                        .ok();
                }
                return false;
            }
            fail(&format!("cannot pull '{image}': {error}"), events).await;
            return false;
        }
        if cancelled(commands, cancel) {
            if !commands.is_closed() {
                events
                    .send(GrobidServiceEvent::Failed {
                        message: "pull cancelled".to_owned(),
                    })
                    .await
                    .ok();
            }
            return false;
        }
        if let Err(error) = create_container(&docker, image).await {
            fail(
                &format!("cannot create the GROBID container: {error}"),
                events,
            )
            .await;
            return false;
        }
        if let Err(error) = docker.start_container(MANAGED_GROBID_CONTAINER, None).await {
            fail(
                &format!("cannot start the GROBID container: {error}"),
                events,
            )
            .await;
            return true;
        }
    }

    match wait_until_ready(client, events, commands, cancel).await {
        StartOutcome::Ready | StartOutcome::TimedOut => true,
        StartOutcome::Cancelled => {
            if commands.is_closed() {
                return true;
            }
            let options = StopContainerOptionsBuilder::default().t(5).build();
            let _ = docker
                .stop_container(MANAGED_GROBID_CONTAINER, Some(options))
                .await;
            events
                .send(GrobidServiceEvent::Container(
                    GrobidContainerStatus::Stopped,
                ))
                .await
                .ok();
            events
                .send(GrobidServiceEvent::Failed {
                    message: "start cancelled".to_owned(),
                })
                .await
                .ok();
            true
        }
    }
}

enum StartOutcome {
    Ready,
    TimedOut,
    Cancelled,
}

fn cancelled(commands: &async_channel::Receiver<GrobidCommand>, cancel: &AtomicBool) -> bool {
    commands.is_closed() || cancel.load(Ordering::Relaxed)
}

async fn pull_image(
    docker: &Docker,
    image: &str,
    events: &Sender<GrobidServiceEvent>,
    commands: &async_channel::Receiver<GrobidCommand>,
    cancel: &AtomicBool,
) -> Result<(), String> {
    let mut options = CreateImageOptionsBuilder::default().from_image(image);
    if let Some(platform) = managed_platform(image) {
        options = options.platform(platform);
    }
    let options = options.build();
    let mut stream = docker.create_image(Some(options), None, None);
    let mut layers: HashMap<String, u64> = HashMap::new();
    let mut totals: HashMap<String, u64> = HashMap::new();
    let mut last_emit = Instant::now() - Duration::from_secs(1);

    while let Some(item) = stream.next().await {
        if cancelled(commands, cancel) {
            return Err(CANCELLED.to_owned());
        }
        let info = item.map_err(|error| error.to_string())?;
        if let Some(error) = info
            .error_detail
            .as_ref()
            .and_then(|detail| detail.message.clone())
        {
            return Err(error);
        }
        if let Some(detail) = info.progress_detail.as_ref() {
            let id = info.id.clone().unwrap_or_default();
            let current = detail.current.unwrap_or_default().max(0) as u64;
            let total = detail.total.unwrap_or_default().max(0) as u64;
            layers.insert(id.clone(), current);
            if total > 0 {
                totals.insert(id, total);
            }
        }

        if last_emit.elapsed() >= Duration::from_millis(120) {
            let received = layers.values().sum();
            let total = totals.values().sum();
            send_progress(
                events,
                GrobidContainerStatus::Pulling {
                    image: image.to_owned(),
                    received,
                    total,
                },
            );
            last_emit = Instant::now();
        }
    }

    let received = layers.values().sum();
    let total = totals.values().sum();
    send_progress(
        events,
        GrobidContainerStatus::Pulling {
            image: image.to_owned(),
            received,
            total,
        },
    );
    Ok(())
}

/// Progress is droppable: a stalled UI must never block the Docker worker while
/// the app is shutting down and no longer drains the event channel.
fn send_progress(events: &Sender<GrobidServiceEvent>, status: GrobidContainerStatus) {
    let _ = events.try_send(GrobidServiceEvent::Container(status));
}

async fn create_container(docker: &Docker, image: &str) -> Result<(), String> {
    let mut options = CreateContainerOptionsBuilder::default().name(MANAGED_GROBID_CONTAINER);
    if let Some(platform) = managed_platform(image) {
        options = options.platform(platform);
    }
    docker
        .create_container(Some(options.build()), managed_container_config(image))
        .await
        .map(|_| ())
        .map_err(|error| error.to_string())
}

async fn wait_until_ready(
    client: &Client,
    events: &Sender<GrobidServiceEvent>,
    commands: &async_channel::Receiver<GrobidCommand>,
    cancel: &AtomicBool,
) -> StartOutcome {
    events
        .send(GrobidServiceEvent::Container(
            GrobidContainerStatus::Starting,
        ))
        .await
        .ok();
    let deadline = Instant::now() + READY_TIMEOUT;

    loop {
        if cancelled(commands, cancel) {
            return StartOutcome::Cancelled;
        }
        if let Ok(probe) = probe_grobid(client, MANAGED_GROBID_URL, Duration::from_secs(2)).await {
            events
                .send(GrobidServiceEvent::Probe(GrobidProbeStatus::Reachable {
                    url: probe.url.clone(),
                    version: probe.version.clone(),
                }))
                .await
                .ok();
            events
                .send(GrobidServiceEvent::Ready {
                    url: probe.url,
                    version: probe.version,
                    managed: true,
                })
                .await
                .ok();
            return StartOutcome::Ready;
        }
        if Instant::now() >= deadline {
            let message = format!(
                "GROBID did not answer {MANAGED_GROBID_URL}/api/isalive within {} seconds",
                READY_TIMEOUT.as_secs()
            );
            fail(&message, events).await;
            return StartOutcome::TimedOut;
        }
        tokio::time::sleep(Duration::from_millis(750)).await;
    }
}

async fn stop(events: &Sender<GrobidServiceEvent>) {
    let Some(docker) = connect_docker(events).await else {
        return;
    };
    if docker
        .inspect_container(MANAGED_GROBID_CONTAINER, None)
        .await
        .is_err()
    {
        events
            .send(GrobidServiceEvent::Container(
                GrobidContainerStatus::Missing,
            ))
            .await
            .ok();
        return;
    }
    let options = StopContainerOptionsBuilder::default().t(10).build();
    match docker
        .stop_container(MANAGED_GROBID_CONTAINER, Some(options))
        .await
    {
        Ok(()) => {
            events
                .send(GrobidServiceEvent::Container(
                    GrobidContainerStatus::Stopped,
                ))
                .await
                .ok();
        }
        Err(error) => {
            fail(
                &format!("cannot stop the GROBID container: {error}"),
                events,
            )
            .await;
        }
    }
}

async fn remove(events: &Sender<GrobidServiceEvent>) -> bool {
    let Some(docker) = connect_docker(events).await else {
        return true;
    };
    let exists = docker
        .inspect_container(MANAGED_GROBID_CONTAINER, None)
        .await
        .is_ok();
    if !exists {
        events
            .send(GrobidServiceEvent::Container(
                GrobidContainerStatus::Missing,
            ))
            .await
            .ok();
        return false;
    }
    let stop_options = StopContainerOptionsBuilder::default().t(5).build();
    let _ = docker
        .stop_container(MANAGED_GROBID_CONTAINER, Some(stop_options))
        .await;
    let options = RemoveContainerOptionsBuilder::default().force(true).build();
    match docker
        .remove_container(MANAGED_GROBID_CONTAINER, Some(options))
        .await
    {
        Ok(()) => {
            events
                .send(GrobidServiceEvent::Container(
                    GrobidContainerStatus::Missing,
                ))
                .await
                .ok();
            false
        }
        Err(error) => {
            fail(
                &format!("cannot remove the GROBID container: {error}"),
                events,
            )
            .await;
            true
        }
    }
}

async fn connect_docker(events: &Sender<GrobidServiceEvent>) -> Option<Docker> {
    events
        .send(GrobidServiceEvent::Docker(GrobidDockerStatus::Checking))
        .await
        .ok();
    match connect_docker_quiet().await {
        Ok(docker) => {
            let version = docker
                .version()
                .await
                .ok()
                .and_then(|version| version.version)
                .unwrap_or_else(|| "unknown".to_owned());
            events
                .send(GrobidServiceEvent::Docker(GrobidDockerStatus::Available {
                    version,
                }))
                .await
                .ok();
            Some(docker)
        }
        Err(error) => {
            let message = docker_message(&error);
            events
                .send(GrobidServiceEvent::Docker(
                    GrobidDockerStatus::Unavailable {
                        message: message.clone(),
                    },
                ))
                .await
                .ok();
            events
                .send(GrobidServiceEvent::Container(
                    GrobidContainerStatus::Failed { message },
                ))
                .await
                .ok();
            None
        }
    }
}

async fn connect_docker_quiet() -> Result<Docker, String> {
    let connection = if std::env::var_os("DOCKER_HOST").is_some() {
        Docker::connect_with_defaults()
    } else {
        Docker::connect_with_local_defaults()
    };
    let docker = connection
        .map_err(|error| error.to_string())?
        .with_timeout(Duration::from_secs(3));
    docker.ping().await.map_err(|error| error.to_string())?;
    Ok(docker)
}

fn docker_message(error: &str) -> String {
    format!(
        "Docker is not reachable ({error}) — start Docker Desktop or the Docker daemon, then retry"
    )
}

async fn fail(message: &str, events: &Sender<GrobidServiceEvent>) {
    events
        .send(GrobidServiceEvent::Container(
            GrobidContainerStatus::Failed {
                message: message.to_owned(),
            },
        ))
        .await
        .ok();
    events
        .send(GrobidServiceEvent::Failed {
            message: message.to_owned(),
        })
        .await
        .ok();
}

async fn probe_grobid(
    client: &Client,
    url: &str,
    timeout: Duration,
) -> Result<GrobidProbe, String> {
    let url = normalize_grobid_url(url)?;
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

    Ok(GrobidProbe { url, version })
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
    let candidate = if trimmed.contains("://") {
        trimmed.to_owned()
    } else {
        format!("http://{trimmed}")
    };
    let lower = candidate.to_ascii_lowercase();
    if !lower.starts_with("http://") && !lower.starts_with("https://") {
        return Err("the GROBID URL must start with http:// or https://".to_owned());
    }
    Ok(candidate.trim_end_matches('/').to_owned())
}

/// The full GROBID images are built for `amd64` only. Asking Docker for that
/// platform lets Apple silicon and `linux/arm64` hosts use emulation instead of
/// failing the pull. The `-crf` images are multi-architecture and stay native.
fn managed_platform(image: &str) -> Option<&'static str> {
    image.trim().ends_with("-full").then_some("linux/amd64")
}

fn managed_container_config(image: &str) -> ContainerCreateBody {
    let mut port_bindings = HashMap::new();
    port_bindings.insert(
        format!("{MANAGED_GROBID_PORT}/tcp"),
        Some(vec![PortBinding {
            host_ip: Some("127.0.0.1".to_owned()),
            host_port: Some(MANAGED_GROBID_PORT.to_string()),
        }]),
    );
    let mut labels = HashMap::new();
    labels.insert(CONTAINER_LABEL.to_owned(), "true".to_owned());
    labels.insert(SERVICE_LABEL.to_owned(), "grobid".to_owned());

    ContainerCreateBody {
        image: Some(image.to_owned()),
        exposed_ports: Some(vec![format!("{MANAGED_GROBID_PORT}/tcp")]),
        labels: Some(labels),
        host_config: Some(HostConfig {
            init: Some(true),
            ulimits: Some(vec![ResourcesUlimits {
                name: Some("core".to_owned()),
                soft: Some(0),
                hard: Some(0),
            }]),
            port_bindings: Some(port_bindings),
            auto_remove: Some(false),
            ..HostConfig::default()
        }),
        ..ContainerCreateBody::default()
    }
}

fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rag::DEFAULT_GROBID_IMAGE;

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
    fn the_managed_container_uses_the_full_image_and_a_loopback_port() {
        let config = managed_container_config(DEFAULT_GROBID_IMAGE);
        assert_eq!(config.image.as_deref(), Some(DEFAULT_GROBID_IMAGE));
        assert_eq!(config.exposed_ports, Some(vec!["8070/tcp".to_owned()]));
        assert_eq!(
            config
                .labels
                .as_ref()
                .and_then(|labels| labels.get(CONTAINER_LABEL))
                .map(String::as_str),
            Some("true")
        );

        let host = config.host_config.expect("host config");
        assert_eq!(host.init, Some(true));
        assert_eq!(host.auto_remove, Some(false));
        let binding = host
            .port_bindings
            .as_ref()
            .and_then(|bindings| bindings.get("8070/tcp"))
            .and_then(|binding| binding.as_ref())
            .and_then(|binding| binding.first())
            .expect("port binding");
        assert_eq!(binding.host_ip.as_deref(), Some("127.0.0.1"));
        assert_eq!(binding.host_port.as_deref(), Some("8070"));

        let ulimit = host
            .ulimits
            .as_ref()
            .and_then(|limits| limits.first())
            .expect("ulimit");
        assert_eq!(ulimit.name.as_deref(), Some("core"));
        assert_eq!((ulimit.soft, ulimit.hard), (Some(0), Some(0)));
    }

    #[test]
    fn the_full_image_is_requested_for_amd64() {
        assert_eq!(managed_platform(DEFAULT_GROBID_IMAGE), Some("linux/amd64"));
        assert_eq!(managed_platform("grobid/grobid:0.9.1-crf"), None);
        assert_eq!(
            managed_platform("example.org/grobid:custom-full"),
            Some("linux/amd64")
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore = "requires a Docker daemon; run with --ignored"]
    async fn a_cancelled_pull_stops_before_touching_the_container() {
        let (commands_tx, commands) = async_channel::bounded(1);
        let (events, events_rx) = async_channel::bounded(8);
        let docker = connect_docker_quiet().await.expect("docker daemon");
        assert!(
            !remove(&events).await,
            "the managed container could not be removed"
        );

        let error = pull_image(
            &docker,
            "hello-world:latest",
            &events,
            &commands,
            &AtomicBool::new(true),
        )
        .await
        .expect_err("the pull should have been cancelled");

        assert_eq!(error, CANCELLED);
        drop(commands_tx);
        drop(events_rx);
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
    async fn a_live_server_reports_its_version() {
        let base = serve(vec![
            response("true"),
            response(r#"{"version":"0.9.1","revision":"abc"}"#),
        ]);
        let client = Client::builder()
            .timeout(Duration::from_secs(2))
            .build()
            .expect("client");

        let probe = probe_grobid(&client, &base, Duration::from_secs(2))
            .await
            .expect("probe");

        assert_eq!(probe.url, base);
        assert_eq!(probe.version.as_deref(), Some("0.9.1"));
    }

    #[tokio::test]
    async fn a_server_that_is_not_alive_is_rejected() {
        let base = serve(vec![response("false")]);
        let client = Client::builder()
            .timeout(Duration::from_secs(2))
            .build()
            .expect("client");

        let error = probe_grobid(&client, &base, Duration::from_secs(2))
            .await
            .expect_err("not alive");

        assert!(error.contains("isalive"), "{error}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore = "requires a Docker daemon; run with --ignored"]
    async fn the_managed_container_starts_answers_and_stops() {
        let image = std::env::var("RAGRIG_GROBID_TEST_IMAGE")
            .unwrap_or_else(|_| "grobid/grobid:0.9.1-crf".to_owned());
        let (commands_tx, commands) = async_channel::bounded(16);
        let (events_tx, events) = async_channel::bounded(256);
        let client = Client::builder()
            .timeout(Duration::from_secs(3))
            .build()
            .expect("client");
        let docker = connect_docker_quiet().await.expect("docker daemon");

        let remove_options = RemoveContainerOptionsBuilder::default().force(true).build();
        let _ = docker
            .remove_container(MANAGED_GROBID_CONTAINER, Some(remove_options))
            .await;

        pull_image(
            &docker,
            &image,
            &events_tx,
            &commands,
            &AtomicBool::new(false),
        )
        .await
        .expect("pull");
        create_container(&docker, &image).await.expect("create");
        docker
            .start_container(MANAGED_GROBID_CONTAINER, None)
            .await
            .expect("start");
        wait_until_ready(&client, &events_tx, &commands, &AtomicBool::new(false)).await;

        let ready = std::iter::from_fn(|| events.try_recv().ok())
            .any(|event| matches!(event, GrobidServiceEvent::Ready { managed: true, .. }));
        assert!(ready, "the managed container never reported ready");

        stop(&events_tx).await;
        let stopped = std::iter::from_fn(|| events.try_recv().ok()).any(|event| {
            matches!(
                event,
                GrobidServiceEvent::Container(GrobidContainerStatus::Stopped)
            )
        });
        assert!(stopped, "the managed container did not stop cleanly");

        assert!(!remove(&events_tx).await, "the container still exists");
        drop(commands_tx);
    }

    #[test]
    fn container_summaries_include_progress() {
        let status = GrobidContainerStatus::Pulling {
            image: "grobid/grobid:0.9.1-full".to_owned(),
            received: 512 * 1024 * 1024,
            total: 1024 * 1024 * 1024,
        };
        assert!(status.summary().contains("50%"));
        assert!(status.summary().contains("512.0 MB"));
    }
}
