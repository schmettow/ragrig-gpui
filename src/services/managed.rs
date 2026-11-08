//! Shared Docker lifecycle for the managed external services.
//!
//! [`ExternalServices`] owns the containers created by the **External
//! services** settings page: GROBID, Ollama, and a llama.cpp server. It also
//! probes servers that are already running, so a user can switch between a
//! local container and an installation they maintain themselves.
//!
//! All Docker and HTTP work happens on a dedicated Tokio runtime thread. Do not
//! drive these futures from GPUI's executor.

use std::{
    collections::HashMap,
    path::Path,
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
    container::LogOutput,
    models::{MountType, VolumeCreateRequest},
    query_parameters::{
        CreateContainerOptionsBuilder, CreateImageOptionsBuilder, LogsOptionsBuilder,
        RemoveContainerOptionsBuilder, StopContainerOptionsBuilder,
    },
};
use futures_util::StreamExt;
use gpui_kit::{App, Context, Entity, EventEmitter};
use reqwest::Client;

use crate::rag::{RagSettings, ServiceSource};

use super::spec::{self, MANAGED_OLLAMA_URL, Probe, ServiceKind, ServiceLaunch};

const CANCELLED: &str = "cancelled";

/// Whether Docker is reachable from this process.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum DockerStatus {
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

impl DockerStatus {
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

/// Lifecycle of one managed container.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum ContainerStatus {
    /// The container has not been inspected yet.
    #[default]
    Unknown,
    /// Docker is being inspected.
    Checking,
    /// No managed container with this service's name exists.
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
    /// Models are being pulled into a running Ollama container.
    Provisioning {
        /// Model being pulled.
        model: String,
        /// Bytes downloaded so far.
        received: u64,
        /// Total bytes, when the server reported it.
        total: u64,
    },
    /// The container is starting and its health endpoint is polled.
    Starting,
    /// The service answers its health endpoint.
    Running,
    /// The container is being stopped.
    Stopping,
    /// The last operation failed.
    Failed {
        /// Human-readable failure.
        message: String,
    },
}

impl ContainerStatus {
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
            } => format!("Pulling {image} — {}", format_bytes(*received)),
            Self::Provisioning {
                model,
                received,
                total,
            } if *total > 0 => {
                let percent = (*received as f64 / *total as f64 * 100.0).clamp(0.0, 100.0);
                format!(
                    "Pulling model {model} — {percent:.0}% ({} of {})",
                    format_bytes(*received),
                    format_bytes(*total)
                )
            }
            Self::Provisioning {
                model, received, ..
            } => {
                format!("Pulling model {model} — {}", format_bytes(*received))
            }
            Self::Starting => "Starting…".to_owned(),
            Self::Running => "Running".to_owned(),
            Self::Stopping => "Stopping…".to_owned(),
            Self::Failed { message } => format!("Failed: {message}"),
        }
    }
}

/// State of the last endpoint probe.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum ProbeStatus {
    /// No endpoint has been tested.
    #[default]
    Idle,
    /// A probe is running.
    Checking {
        /// Endpoint being tested.
        url: String,
    },
    /// The endpoint answered its health endpoint.
    Reachable {
        /// Normalized endpoint that answered.
        url: String,
        /// Version reported by the service, when available.
        version: Option<String>,
    },
    /// The endpoint did not answer as the expected service.
    Unreachable {
        /// Endpoint that was tested.
        url: String,
        /// Human-readable failure.
        message: String,
    },
}

impl ProbeStatus {
    /// One line for the settings page.
    pub fn summary(&self) -> String {
        match self {
            Self::Idle => "Not tested".to_owned(),
            Self::Checking { url } => format!("Testing {url}…"),
            Self::Reachable { url, version } => match version {
                Some(version) => format!("reachable at {url} ({version})"),
                None => format!("reachable at {url}"),
            },
            Self::Unreachable { url, message } => format!("{url}: {message}"),
        }
    }
}

/// Changes a host may want to observe.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExternalServiceEvent {
    /// Docker availability changed.
    Docker(DockerStatus),
    /// A managed container lifecycle changed.
    Container {
        /// Service the container belongs to.
        service: ServiceKind,
        /// New lifecycle state.
        status: ContainerStatus,
    },
    /// An endpoint probe changed.
    Probe {
        /// Service that was probed.
        service: ServiceKind,
        /// New probe state.
        status: ProbeStatus,
    },
    /// Localhost discovery found one or more servers.
    Discovered {
        /// Service that was discovered.
        service: ServiceKind,
        /// Servers found.
        probes: Vec<Probe>,
    },
    /// A managed container or an adopted server is ready.
    Ready {
        /// Service that is ready.
        service: ServiceKind,
        /// Base URL that answered.
        url: String,
        /// Version reported by the service, when available.
        version: Option<String>,
        /// Whether the app created or started the container.
        managed: bool,
    },
    /// The URL a host should persist for `service` changed.
    UrlChanged {
        /// Service whose URL changed.
        service: ServiceKind,
        /// New URL.
        url: String,
    },
    /// An operation failed with a user-facing message.
    Failed {
        /// Service the failure belongs to.
        service: ServiceKind,
        /// Human-readable failure.
        message: String,
    },
}

enum ServiceCommand {
    Inspect {
        kind: ServiceKind,
        adopt: bool,
    },
    Probe {
        kind: ServiceKind,
        url: String,
    },
    Discover {
        kind: ServiceKind,
    },
    PullAndStart {
        kind: ServiceKind,
        launch: ServiceLaunch,
    },
    PullModels {
        models: Vec<String>,
    },
    Stop {
        kind: ServiceKind,
    },
    Remove {
        kind: ServiceKind,
    },
}

#[derive(Clone, Debug, Default)]
struct ServiceState {
    container: ContainerStatus,
    probe: ProbeStatus,
    discovered: Vec<Probe>,
    notice: Option<String>,
}

impl ServiceState {
    fn is_busy(&self) -> bool {
        matches!(
            &self.container,
            ContainerStatus::Pulling { .. }
                | ContainerStatus::Provisioning { .. }
                | ContainerStatus::Starting
                | ContainerStatus::Stopping
        )
    }
}

/// Owns the managed containers and probes existing servers.
///
/// Dropping the entity stops containers this service created or adopted in
/// managed mode. A server that is not a managed container is never touched.
pub struct ExternalServices {
    commands: Sender<ServiceCommand>,
    worker: Option<thread::JoinHandle<()>>,
    cancel: Arc<AtomicBool>,
    settings: Entity<RagSettings>,
    docker: DockerStatus,
    services: [ServiceState; ServiceKind::ALL.len()],
}

impl ExternalServices {
    /// Spawn the service worker and start inspecting the configured sources.
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
                .name("ragrig-services".to_owned())
                .spawn(move || {
                    let runtime = tokio::runtime::Builder::new_multi_thread()
                        .worker_threads(2)
                        .enable_all()
                        .build();
                    match runtime {
                        Ok(runtime) => runtime.block_on(run(command_rx, events_tx, worker_cancel)),
                        Err(error) => {
                            let _ = events_tx.send_blocking(ExternalServiceEvent::Failed {
                                service: ServiceKind::Grobid,
                                message: format!(
                                    "cannot start the external services runtime: {error}"
                                ),
                            });
                        }
                    }
                })
                .map_err(|error| format!("cannot start the external services thread: {error}"))?;

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

            for kind in ServiceKind::ALL {
                let source = initial.service_source(kind);
                match source {
                    ServiceSource::Managed => {
                        let _ = commands.try_send(ServiceCommand::Inspect { kind, adopt: true });
                        let launch = ServiceLaunch::from_settings(&initial, kind);
                        if initial.service_auto_start(kind) && launch.missing_field(kind).is_none()
                        {
                            // `Inspect` runs first and reports the container
                            // state; this starts a stopped container or pulls
                            // the image when needed.
                            let _ =
                                commands.try_send(ServiceCommand::PullAndStart { kind, launch });
                        }
                    }
                    ServiceSource::Existing => {
                        let _ = commands.try_send(ServiceCommand::Probe {
                            kind,
                            url: spec::existing_url(kind, &initial),
                        });
                    }
                }
            }

            Some(worker)
        };

        Ok(Self {
            commands,
            worker,
            cancel,
            settings,
            docker: DockerStatus::Unknown,
            services: std::array::from_fn(|_| ServiceState::default()),
        })
    }

    /// Whether Docker is reachable.
    pub fn docker(&self) -> &DockerStatus {
        &self.docker
    }

    /// Managed container lifecycle for `kind`.
    pub fn container(&self, kind: ServiceKind) -> &ContainerStatus {
        &self.state(kind).container
    }

    /// State of the last endpoint probe for `kind`.
    pub fn probe(&self, kind: ServiceKind) -> &ProbeStatus {
        &self.state(kind).probe
    }

    /// Servers found by the last localhost discovery for `kind`.
    pub fn discovered(&self, kind: ServiceKind) -> &[Probe] {
        &self.state(kind).discovered
    }

    /// The last user-facing notice for `kind`.
    pub fn notice(&self, kind: ServiceKind) -> Option<&str> {
        self.state(kind).notice.as_deref()
    }

    /// Whether a container operation is in flight for `kind`.
    pub fn is_busy(&self, kind: ServiceKind) -> bool {
        self.state(kind).is_busy()
    }

    /// Whether the running pull or startup for `kind` can still be cancelled.
    pub fn is_cancellable(&self, kind: ServiceKind) -> bool {
        matches!(
            &self.state(kind).container,
            ContainerStatus::Pulling { .. }
                | ContainerStatus::Provisioning { .. }
                | ContainerStatus::Starting
        ) && !self.cancel.load(Ordering::Relaxed)
    }

    /// Whether a cancellation was requested and the worker has not reacted yet.
    pub fn is_cancelling(&self, kind: ServiceKind) -> bool {
        self.cancel.load(Ordering::Relaxed)
            && matches!(
                &self.state(kind).container,
                ContainerStatus::Pulling { .. }
                    | ContainerStatus::Provisioning { .. }
                    | ContainerStatus::Starting
            )
    }

    /// Ask the worker to abandon the current pull or startup for `kind`.
    pub fn cancel(&mut self, kind: ServiceKind, cx: &mut Context<Self>) {
        if self.is_cancellable(kind) {
            self.cancel.store(true, Ordering::Relaxed);
            self.state_mut(kind).notice = Some("cancelling…".to_owned());
            cx.notify();
        }
    }

    /// Re-inspect `kind` according to its configured source.
    pub fn refresh(&mut self, kind: ServiceKind, cx: &mut Context<Self>) {
        match self.settings.read(cx).service_source(kind) {
            ServiceSource::Managed => {
                self.send_quiet(kind, ServiceCommand::Inspect { kind, adopt: true }, cx);
            }
            ServiceSource::Existing => self.probe_existing(kind, cx),
        }
    }

    /// Re-inspect every managed source and probe every existing source.
    pub fn refresh_all(&mut self, cx: &mut Context<Self>) {
        for kind in ServiceKind::ALL {
            self.refresh(kind, cx);
        }
    }

    /// Probe the configured existing server for `kind`.
    pub fn probe_existing(&mut self, kind: ServiceKind, cx: &mut Context<Self>) {
        let url = spec::existing_url(kind, self.settings.read(cx));
        self.test_url(kind, url, cx);
    }

    /// Probe `url` without changing the configured URL until the probe succeeds.
    pub fn test_url(&mut self, kind: ServiceKind, url: impl Into<String>, cx: &mut Context<Self>) {
        let url = url.into();
        match spec::normalize_url(kind, &url) {
            Ok(url) => {
                self.state_mut(kind).probe = ProbeStatus::Checking { url: url.clone() };
                self.state_mut(kind).discovered.clear();
                self.state_mut(kind).notice = None;
                self.send_quiet(kind, ServiceCommand::Probe { kind, url }, cx);
            }
            Err(message) => {
                self.state_mut(kind).probe = ProbeStatus::Unreachable { url, message };
                cx.notify();
            }
        }
    }

    /// Probe the common localhost ports for `kind` and use the first server
    /// found.
    pub fn discover(&mut self, kind: ServiceKind, cx: &mut Context<Self>) {
        self.state_mut(kind).probe = ProbeStatus::Checking {
            url: "localhost".to_owned(),
        };
        self.state_mut(kind).discovered.clear();
        self.state_mut(kind).notice = None;
        self.send_quiet(kind, ServiceCommand::Discover { kind }, cx);
    }

    /// Pull `kind`'s image when needed, create the container, and wait for
    /// health.
    pub fn start(&mut self, kind: ServiceKind, cx: &mut Context<Self>) {
        let launch = ServiceLaunch::from_settings(self.settings.read(cx), kind);
        if let Some(message) = launch.missing_field(kind) {
            self.state_mut(kind).notice = Some(message.to_owned());
            cx.notify();
            return;
        }
        if kind == ServiceKind::LlamaCpp {
            let model = Path::new(&launch.model);
            if model.is_absolute() && !model.is_file() {
                self.state_mut(kind).notice =
                    Some(format!("model file '{}' does not exist", model.display()));
                cx.notify();
                return;
            }
        }
        self.state_mut(kind).notice = None;
        self.cancel.store(false, Ordering::Relaxed);
        self.send_quiet(kind, ServiceCommand::PullAndStart { kind, launch }, cx);
    }

    /// Pull the configured Ollama models into the running container.
    pub fn pull_models(&mut self, cx: &mut Context<Self>) {
        let models = self.pullable_models(cx);
        if models.is_empty() {
            self.state_mut(ServiceKind::Ollama).notice =
                Some("no Ollama models are configured".to_owned());
            cx.notify();
            return;
        }
        self.cancel.store(false, Ordering::Relaxed);
        self.send_quiet(
            ServiceKind::Ollama,
            ServiceCommand::PullModels { models },
            cx,
        );
    }

    /// The models [`Self::pull_models`] would request for the current settings.
    pub fn pullable_models(&self, cx: &App) -> Vec<String> {
        spec::ollama_models(self.settings.read(cx))
    }

    /// Stop the managed container for `kind` without removing it.
    pub fn stop(&mut self, kind: ServiceKind, cx: &mut Context<Self>) {
        self.state_mut(kind).container = ContainerStatus::Stopping;
        self.send_quiet(kind, ServiceCommand::Stop { kind }, cx);
    }

    /// Stop and remove the managed container for `kind`.
    pub fn remove(&mut self, kind: ServiceKind, cx: &mut Context<Self>) {
        self.state_mut(kind).container = ContainerStatus::Stopping;
        self.send_quiet(kind, ServiceCommand::Remove { kind }, cx);
    }

    /// Clear a stale endpoint probe after the URL was edited.
    pub fn clear_probe(&mut self, kind: ServiceKind, cx: &mut Context<Self>) {
        let state = self.state_mut(kind);
        if !matches!(state.probe, ProbeStatus::Idle) || !state.discovered.is_empty() {
            state.probe = ProbeStatus::Idle;
            state.discovered.clear();
            cx.notify();
        }
    }

    fn state(&self, kind: ServiceKind) -> &ServiceState {
        &self.services[kind.index()]
    }

    fn state_mut(&mut self, kind: ServiceKind) -> &mut ServiceState {
        &mut self.services[kind.index()]
    }

    fn send_quiet(&mut self, kind: ServiceKind, command: ServiceCommand, cx: &mut Context<Self>) {
        if let Err(error) = self.commands.try_send(command) {
            self.state_mut(kind).notice = Some(match error {
                TrySendError::Full(_) => "the service is busy; try again".to_owned(),
                TrySendError::Closed(_) => "the service stopped".to_owned(),
            });
        }
        cx.notify();
    }

    fn on_event(&mut self, event: ExternalServiceEvent, cx: &mut Context<Self>) {
        let mut changed_url = None;
        match &event {
            ExternalServiceEvent::Docker(status) => {
                self.docker = status.clone();
            }
            ExternalServiceEvent::Container { service, status } => {
                if !matches!(status, ContainerStatus::Failed { .. }) {
                    self.state_mut(*service).notice = None;
                }
                if matches!(
                    status,
                    ContainerStatus::Missing
                        | ContainerStatus::Stopped
                        | ContainerStatus::Running
                        | ContainerStatus::Failed { .. }
                ) {
                    self.cancel.store(false, Ordering::Relaxed);
                }
                self.state_mut(*service).container = status.clone();
            }
            ExternalServiceEvent::Probe { service, status } => {
                if matches!(status, ProbeStatus::Reachable { .. }) {
                    self.state_mut(*service).notice = None;
                }
                if let ProbeStatus::Reachable { url, .. } = status {
                    changed_url = Some((*service, url.clone()));
                }
                self.state_mut(*service).probe = status.clone();
            }
            ExternalServiceEvent::Discovered { service, probes } => {
                self.state_mut(*service).discovered = probes.clone();
                if let Some(first) = probes.first() {
                    changed_url = Some((*service, first.url.clone()));
                }
            }
            ExternalServiceEvent::Ready {
                service,
                url,
                version,
                managed,
            } => {
                let state = self.state_mut(*service);
                state.container = ContainerStatus::Running;
                state.probe = ProbeStatus::Reachable {
                    url: url.clone(),
                    version: version.clone(),
                };
                state.notice = if *managed {
                    None
                } else {
                    Some(format!(
                        "{url} already answers; using it instead of starting a container"
                    ))
                };
                if service.has_url_setting() {
                    changed_url = Some((*service, url.clone()));
                }
                let source = if *managed {
                    ServiceSource::Managed
                } else {
                    ServiceSource::Existing
                };
                self.settings.update(cx, |settings, cx| {
                    if settings.service_source(*service) != source {
                        settings.set_service_source(*service, source);
                        cx.notify();
                    }
                });
            }
            ExternalServiceEvent::UrlChanged { .. } => {}
            ExternalServiceEvent::Failed { service, message } => {
                self.state_mut(*service).notice = Some(message.clone());
            }
        }

        if let Some((service, url)) = changed_url
            && service.has_url_setting()
        {
            let url_for_settings = url.clone();
            let changed = self.settings.update(cx, |settings, cx| {
                let changed = settings.set_service_url(service, url_for_settings);
                if changed {
                    cx.notify();
                }
                changed
            });
            if changed {
                cx.emit(ExternalServiceEvent::UrlChanged { service, url });
            }
        }
        cx.notify();
        cx.emit(event);
    }
}

impl EventEmitter<ExternalServiceEvent> for ExternalServices {}

impl Drop for ExternalServices {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        self.commands.close();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl RagSettings {
    /// Where the given service comes from.
    pub fn service_source(&self, kind: ServiceKind) -> ServiceSource {
        match kind {
            ServiceKind::Grobid => self.grobid_source,
            ServiceKind::Ollama => self.ollama_source,
            ServiceKind::LlamaCpp => self.llamacpp_source,
        }
    }

    /// Set where the given service comes from.
    pub fn set_service_source(&mut self, kind: ServiceKind, source: ServiceSource) {
        match kind {
            ServiceKind::Grobid => self.grobid_source = source,
            ServiceKind::Ollama => self.ollama_source = source,
            ServiceKind::LlamaCpp => self.llamacpp_source = source,
        }
    }

    /// Whether the given service starts automatically.
    pub fn service_auto_start(&self, kind: ServiceKind) -> bool {
        match kind {
            ServiceKind::Grobid => self.grobid_auto_start,
            ServiceKind::Ollama => self.ollama_auto_start,
            ServiceKind::LlamaCpp => self.llamacpp_auto_start,
        }
    }

    /// Set whether the given service starts automatically.
    pub fn set_service_auto_start(&mut self, kind: ServiceKind, auto_start: bool) {
        match kind {
            ServiceKind::Grobid => self.grobid_auto_start = auto_start,
            ServiceKind::Ollama => self.ollama_auto_start = auto_start,
            ServiceKind::LlamaCpp => self.llamacpp_auto_start = auto_start,
        }
    }

    /// Docker image configured for the given service.
    pub fn service_image(&self, kind: ServiceKind) -> &str {
        match kind {
            ServiceKind::Grobid => &self.grobid_image,
            ServiceKind::Ollama => &self.ollama_image,
            ServiceKind::LlamaCpp => &self.llamacpp_image,
        }
    }

    /// Set the Docker image configured for the given service.
    pub fn set_service_image(&mut self, kind: ServiceKind, image: impl Into<String>) {
        let image = image.into();
        match kind {
            ServiceKind::Grobid => self.grobid_image = image,
            ServiceKind::Ollama => self.ollama_image = image,
            ServiceKind::LlamaCpp => self.llamacpp_image = image,
        }
    }

    /// Update the URL field of `kind`. Returns `true` when the value changed.
    fn set_service_url(&mut self, kind: ServiceKind, url: String) -> bool {
        let field = match kind {
            ServiceKind::Grobid => &mut self.grobid_url,
            ServiceKind::LlamaCpp => &mut self.llamacpp_url,
            ServiceKind::Ollama => return false,
        };
        if *field == url {
            return false;
        }
        *field = url;
        true
    }
}

impl ServiceKind {
    /// Whether this service has a URL setting in the rag configuration.
    fn has_url_setting(self) -> bool {
        matches!(self, Self::Grobid | Self::LlamaCpp)
    }
}

async fn run(
    commands: async_channel::Receiver<ServiceCommand>,
    events: Sender<ExternalServiceEvent>,
    cancel: Arc<AtomicBool>,
) {
    let client = match Client::builder()
        .connect_timeout(Duration::from_secs(4))
        .build()
    {
        Ok(client) => client,
        Err(error) => {
            let _ = events
                .send(ExternalServiceEvent::Failed {
                    service: ServiceKind::Grobid,
                    message: format!("cannot build the service HTTP client: {error}"),
                })
                .await;
            return;
        }
    };
    let mut owns = [false; ServiceKind::ALL.len()];

    while let Ok(command) = commands.recv().await {
        match command {
            ServiceCommand::Inspect { kind, adopt } => {
                let present = inspect(&client, &events, kind, adopt).await;
                if adopt {
                    owns[kind.index()] = present;
                }
            }
            ServiceCommand::Probe { kind, url } => {
                probe_command(&client, &url, kind, &events).await;
            }
            ServiceCommand::Discover { kind } => {
                discover(&client, &events, kind, &commands).await;
            }
            ServiceCommand::PullAndStart { kind, launch } => {
                if pull_and_start(&client, &launch, kind, &events, &commands, &cancel).await {
                    owns[kind.index()] = true;
                }
            }
            ServiceCommand::PullModels { models } => {
                pull_ollama_models(&client, &models, &events, &commands, &cancel).await;
            }
            ServiceCommand::Stop { kind } => {
                stop(&events, kind).await;
            }
            ServiceCommand::Remove { kind } => {
                if !remove(&events, kind).await {
                    owns[kind.index()] = false;
                }
            }
        }
    }

    if let Ok(docker) = connect_docker_quiet().await {
        for kind in ServiceKind::ALL {
            if !owns[kind.index()] {
                continue;
            }
            let options = StopContainerOptionsBuilder::default().t(5).build();
            let _ = tokio::time::timeout(
                Duration::from_secs(8),
                docker.stop_container(kind.container(), Some(options)),
            )
            .await;
        }
    }
}

async fn inspect(
    client: &Client,
    events: &Sender<ExternalServiceEvent>,
    kind: ServiceKind,
    adopt: bool,
) -> bool {
    send_container(events, kind, ContainerStatus::Checking).await;
    let Some(docker) = connect_docker(events, kind).await else {
        return false;
    };
    match docker.inspect_container(kind.container(), None).await {
        Ok(info) => {
            let running = info
                .state
                .as_ref()
                .and_then(|state| state.running)
                .unwrap_or(false);
            if running {
                send_container(events, kind, ContainerStatus::Running).await;
                if adopt
                    && let Ok(probe) =
                        spec::probe(client, kind, kind.managed_url(), Duration::from_secs(2)).await
                {
                    events
                        .send(ExternalServiceEvent::Ready {
                            service: kind,
                            url: probe.url,
                            version: probe.version,
                            managed: true,
                        })
                        .await
                        .ok();
                }
            } else {
                send_container(events, kind, ContainerStatus::Stopped).await;
            }
            true
        }
        Err(_) => {
            send_container(events, kind, ContainerStatus::Missing).await;
            false
        }
    }
}

async fn probe_command(
    client: &Client,
    url: &str,
    kind: ServiceKind,
    events: &Sender<ExternalServiceEvent>,
) {
    let normalized = match spec::normalize_url(kind, url) {
        Ok(url) => url,
        Err(message) => {
            events
                .send(ExternalServiceEvent::Probe {
                    service: kind,
                    status: ProbeStatus::Unreachable {
                        url: url.to_owned(),
                        message,
                    },
                })
                .await
                .ok();
            return;
        }
    };
    events
        .send(ExternalServiceEvent::Probe {
            service: kind,
            status: ProbeStatus::Checking {
                url: normalized.clone(),
            },
        })
        .await
        .ok();
    let status = match spec::probe(client, kind, &normalized, Duration::from_secs(3)).await {
        Ok(probe) => ProbeStatus::Reachable {
            url: probe.url,
            version: probe.version,
        },
        Err(message) => ProbeStatus::Unreachable {
            url: normalized,
            message,
        },
    };
    events
        .send(ExternalServiceEvent::Probe {
            service: kind,
            status,
        })
        .await
        .ok();
}

async fn discover(
    client: &Client,
    events: &Sender<ExternalServiceEvent>,
    kind: ServiceKind,
    commands: &async_channel::Receiver<ServiceCommand>,
) {
    events
        .send(ExternalServiceEvent::Probe {
            service: kind,
            status: ProbeStatus::Checking {
                url: "localhost".to_owned(),
            },
        })
        .await
        .ok();

    let mut found = Vec::new();
    for port in kind.discovery_ports() {
        for host in ["127.0.0.1", "localhost"] {
            if commands.is_closed() {
                return;
            }
            let url = format!("http://{host}:{port}");
            if let Ok(probe) = spec::probe(client, kind, &url, Duration::from_millis(900)).await {
                found.push(probe);
                break;
            }
        }
        if !found.is_empty() {
            break;
        }
    }

    if found.is_empty() {
        let ports = kind
            .discovery_ports()
            .iter()
            .map(u16::to_string)
            .collect::<Vec<_>>()
            .join(" or ");
        events
            .send(ExternalServiceEvent::Probe {
                service: kind,
                status: ProbeStatus::Unreachable {
                    url: "localhost".to_owned(),
                    message: format!("No {} server answered on ports {ports}", kind.label()),
                },
            })
            .await
            .ok();
        return;
    }

    events
        .send(ExternalServiceEvent::Probe {
            service: kind,
            status: ProbeStatus::Reachable {
                url: found[0].url.clone(),
                version: found[0].version.clone(),
            },
        })
        .await
        .ok();
    events
        .send(ExternalServiceEvent::Discovered {
            service: kind,
            probes: found,
        })
        .await
        .ok();
}

async fn pull_and_start(
    client: &Client,
    launch: &ServiceLaunch,
    kind: ServiceKind,
    events: &Sender<ExternalServiceEvent>,
    commands: &async_channel::Receiver<ServiceCommand>,
    cancel: &AtomicBool,
) -> bool {
    if let Some(message) = launch.missing_field(kind) {
        fail(events, kind, message).await;
        return false;
    }
    if kind == ServiceKind::LlamaCpp {
        let model = Path::new(&launch.model);
        if model.is_absolute() && !model.is_file() {
            fail(
                events,
                kind,
                &format!("model file '{}' does not exist", model.display()),
            )
            .await;
            return false;
        }
    }
    let Some(docker) = connect_docker(events, kind).await else {
        return false;
    };

    let image_is_local = docker.inspect_image(&launch.image).await.is_ok();
    if !image_is_local && docker.inspect_container(&launch.image, None).await.is_ok() {
        fail(
            events,
            kind,
            &format!(
                "'{image}' is a container, not an image. Start it with Docker and select Existing server (recommended for a hand-configured container), or enter the image it was built from.",
                image = launch.image
            ),
        )
        .await;
        return false;
    }

    let inspected = docker.inspect_container(kind.container(), None).await.ok();
    let running = inspected.as_ref().is_some_and(|info| {
        info.state
            .as_ref()
            .and_then(|state| state.running)
            .unwrap_or(false)
    });
    if !running {
        // Never fight a server that already owns the port. It is not ours, so
        // the service adopts its URL without owning its lifecycle.
        if let Ok(probe) =
            spec::probe(client, kind, kind.managed_url(), Duration::from_millis(900)).await
        {
            events
                .send(ExternalServiceEvent::Ready {
                    service: kind,
                    url: probe.url,
                    version: probe.version,
                    managed: false,
                })
                .await
                .ok();
            return false;
        }
        if port_in_use(kind).await {
            fail(
                events,
                kind,
                &format!(
                    "port {} is already in use, but {} did not answer its health endpoint. Stop the process using it, or switch {} to Existing server.",
                    kind.port(),
                    kind.label(),
                    kind.label()
                ),
            )
            .await;
            return false;
        }
    }
    let existing = match inspected {
        Some(info) if spec::container_matches(kind, launch, &info) => Some(info),
        Some(_) => {
            // The image or llama.cpp command changed. Recreate the container
            // so Pull and start uses the settings currently on screen.
            let options = RemoveContainerOptionsBuilder::default().force(true).build();
            let _ = docker
                .remove_container(kind.container(), Some(options))
                .await;
            send_container(events, kind, ContainerStatus::Missing).await;
            None
        }
        None => None,
    };
    if existing.is_some() {
        if !running {
            send_container(events, kind, ContainerStatus::Starting).await;
            if let Err(error) = docker.start_container(kind.container(), None).await {
                let logs = container_logs(&docker, kind).await;
                fail(
                    events,
                    kind,
                    &start_message(kind, &error.to_string(), logs.as_deref()),
                )
                .await;
                return true;
            }
        }
    } else {
        if !image_is_local {
            if let Err(error) =
                pull_image(&docker, kind, &launch.image, events, commands, cancel).await
            {
                if error == CANCELLED {
                    if !commands.is_closed() {
                        send_container(events, kind, ContainerStatus::Missing).await;
                        fail(events, kind, "pull cancelled").await;
                    }
                    return false;
                }
                fail(events, kind, &pull_message(kind, &launch.image, &error)).await;
                return false;
            }
            if cancelled(commands, cancel) {
                if !commands.is_closed() {
                    fail(events, kind, "pull cancelled").await;
                }
                return false;
            }
        }
        if let Err(error) = create_container(&docker, kind, launch).await {
            fail(events, kind, &create_message(kind, &error)).await;
            return false;
        }
        if let Err(error) = docker.start_container(kind.container(), None).await {
            let logs = container_logs(&docker, kind).await;
            fail(
                events,
                kind,
                &start_message(kind, &error.to_string(), logs.as_deref()),
            )
            .await;
            return true;
        }
    }

    match wait_until_ready(client, &docker, events, kind, commands, cancel).await {
        StartOutcome::Ready | StartOutcome::TimedOut => true,
        StartOutcome::Cancelled => {
            if commands.is_closed() {
                return true;
            }
            let options = StopContainerOptionsBuilder::default().t(5).build();
            let _ = docker.stop_container(kind.container(), Some(options)).await;
            send_container(events, kind, ContainerStatus::Stopped).await;
            fail(events, kind, "start cancelled").await;
            true
        }
    }
}

enum StartOutcome {
    Ready,
    TimedOut,
    Cancelled,
}

fn cancelled(commands: &async_channel::Receiver<ServiceCommand>, cancel: &AtomicBool) -> bool {
    commands.is_closed() || cancel.load(Ordering::Relaxed)
}

async fn pull_image(
    docker: &Docker,
    kind: ServiceKind,
    image: &str,
    events: &Sender<ExternalServiceEvent>,
    commands: &async_channel::Receiver<ServiceCommand>,
    cancel: &AtomicBool,
) -> Result<(), String> {
    let mut options = CreateImageOptionsBuilder::default().from_image(image);
    if let Some(platform) = kind.managed_platform(image) {
        options = options.platform(platform);
    }
    let mut stream = docker.create_image(Some(options.build()), None, None);
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
            progress_container(
                events,
                kind,
                ContainerStatus::Pulling {
                    image: image.to_owned(),
                    received: layers.values().sum(),
                    total: totals.values().sum(),
                },
            )
            .await;
            last_emit = Instant::now();
        }
    }

    progress_container(
        events,
        kind,
        ContainerStatus::Pulling {
            image: image.to_owned(),
            received: layers.values().sum(),
            total: totals.values().sum(),
        },
    )
    .await;
    Ok(())
}

async fn pull_ollama_models(
    client: &Client,
    models: &[String],
    events: &Sender<ExternalServiceEvent>,
    commands: &async_channel::Receiver<ServiceCommand>,
    cancel: &AtomicBool,
) {
    for model in models {
        if cancelled(commands, cancel) {
            send_container(events, ServiceKind::Ollama, ContainerStatus::Running).await;
            notice(events, ServiceKind::Ollama, "model pull cancelled").await;
            return;
        }
        progress_container(
            events,
            ServiceKind::Ollama,
            ContainerStatus::Provisioning {
                model: model.clone(),
                received: 0,
                total: 0,
            },
        )
        .await;
        match pull_ollama_model(client, model, events, commands, cancel).await {
            Ok(()) => {}
            Err(error) if error == CANCELLED => {
                send_container(events, ServiceKind::Ollama, ContainerStatus::Running).await;
                notice(events, ServiceKind::Ollama, "model pull cancelled").await;
                return;
            }
            Err(error) => {
                fail(
                    events,
                    ServiceKind::Ollama,
                    &format!("cannot pull model '{model}': {error}"),
                )
                .await;
                return;
            }
        }
    }
    send_container(events, ServiceKind::Ollama, ContainerStatus::Running).await;
}

async fn pull_ollama_model(
    client: &Client,
    model: &str,
    events: &Sender<ExternalServiceEvent>,
    commands: &async_channel::Receiver<ServiceCommand>,
    cancel: &AtomicBool,
) -> Result<(), String> {
    let response = client
        .post(format!("{MANAGED_OLLAMA_URL}/api/pull"))
        .json(&serde_json::json!({ "model": model, "stream": true }))
        .send()
        .await
        .map_err(probe_error)?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        let body = body.trim();
        return Err(if body.is_empty() {
            format!("HTTP {status}")
        } else {
            format!("HTTP {status}: {body}")
        });
    }
    let mut stream = response.bytes_stream();
    let mut pending: Vec<u8> = Vec::new();
    let mut received = 0;
    let mut total = 0;
    let mut last_emit = Instant::now() - Duration::from_secs(1);

    loop {
        if cancelled(commands, cancel) {
            return Err(CANCELLED.to_owned());
        }
        let chunk = match tokio::time::timeout(Duration::from_secs(120), stream.next()).await {
            Ok(Some(chunk)) => chunk.map_err(probe_error)?,
            Ok(None) => break,
            Err(_) => {
                return Err("the model pull stalled for two minutes".to_owned());
            }
        };
        pending.extend_from_slice(&chunk);
        while let Some(newline) = pending.iter().position(|byte| *byte == b'\n') {
            let line: Vec<u8> = pending.drain(..=newline).collect();
            let line = line.strip_suffix(b"\n").unwrap_or(&line);
            if line.iter().all(u8::is_ascii_whitespace) {
                continue;
            }
            let Ok(value) = serde_json::from_slice::<serde_json::Value>(line) else {
                continue;
            };
            if let Some(error) = value.get("error").and_then(|error| error.as_str()) {
                return Err(error.to_owned());
            }
            received = value
                .get("completed")
                .and_then(|completed| completed.as_u64())
                .unwrap_or(received);
            total = value
                .get("total")
                .and_then(|total| total.as_u64())
                .unwrap_or(total);
            if last_emit.elapsed() >= Duration::from_millis(120) {
                progress_container(
                    events,
                    ServiceKind::Ollama,
                    ContainerStatus::Provisioning {
                        model: model.to_owned(),
                        received,
                        total,
                    },
                )
                .await;
                last_emit = Instant::now();
            }
        }
    }

    progress_container(
        events,
        ServiceKind::Ollama,
        ContainerStatus::Provisioning {
            model: model.to_owned(),
            received,
            total,
        },
    )
    .await;
    Ok(())
}

async fn create_container(
    docker: &Docker,
    kind: ServiceKind,
    launch: &ServiceLaunch,
) -> Result<(), String> {
    let mut options = CreateContainerOptionsBuilder::default().name(kind.container());
    if let Some(platform) = kind.managed_platform(&launch.image) {
        options = options.platform(platform);
    }
    let config = spec::container_config(kind, launch)?;
    if let Some(mounts) = config
        .host_config
        .as_ref()
        .and_then(|host| host.mounts.as_ref())
    {
        for mount in mounts {
            if mount.typ != Some(MountType::VOLUME) {
                continue;
            }
            let Some(name) = &mount.source else {
                continue;
            };
            let request = VolumeCreateRequest {
                name: Some(name.clone()),
                ..VolumeCreateRequest::default()
            };
            if let Err(error) = docker.create_volume(request).await {
                let text = error.to_string();
                if !text.to_ascii_lowercase().contains("already exists") {
                    return Err(text);
                }
            }
        }
    }
    docker
        .create_container(Some(options.build()), config)
        .await
        .map(|_| ())
        .map_err(|error| error.to_string())
}

async fn wait_until_ready(
    client: &Client,
    docker: &Docker,
    events: &Sender<ExternalServiceEvent>,
    kind: ServiceKind,
    commands: &async_channel::Receiver<ServiceCommand>,
    cancel: &AtomicBool,
) -> StartOutcome {
    send_container(events, kind, ContainerStatus::Starting).await;
    let timeout = kind.ready_timeout();
    let deadline = Instant::now() + timeout;

    loop {
        if cancelled(commands, cancel) {
            return StartOutcome::Cancelled;
        }
        if let Ok(probe) =
            spec::probe(client, kind, kind.managed_url(), Duration::from_secs(2)).await
        {
            events
                .send(ExternalServiceEvent::Probe {
                    service: kind,
                    status: ProbeStatus::Reachable {
                        url: probe.url.clone(),
                        version: probe.version.clone(),
                    },
                })
                .await
                .ok();
            events
                .send(ExternalServiceEvent::Ready {
                    service: kind,
                    url: probe.url,
                    version: probe.version,
                    managed: true,
                })
                .await
                .ok();
            return StartOutcome::Ready;
        }
        if Instant::now() >= deadline {
            let mut message = format!(
                "{} did not answer {} within {} seconds",
                kind.label(),
                kind.managed_url(),
                timeout.as_secs()
            );
            if let Some(logs) = container_logs(docker, kind).await {
                message.push_str(". Last container output: ");
                message.push_str(&logs);
            }
            fail(events, kind, &message).await;
            return StartOutcome::TimedOut;
        }
        tokio::time::sleep(Duration::from_millis(750)).await;
    }
}

async fn container_logs(docker: &Docker, kind: ServiceKind) -> Option<String> {
    let options = LogsOptionsBuilder::default()
        .stdout(true)
        .stderr(true)
        .tail("24")
        .build();
    let mut stream = docker.logs(kind.container(), Some(options));
    let mut text = String::new();
    while let Some(Ok(output)) = stream.next().await {
        match output {
            LogOutput::StdOut { message }
            | LogOutput::StdErr { message }
            | LogOutput::Console { message } => {
                text.push_str(&String::from_utf8_lossy(&message));
            }
            LogOutput::StdIn { .. } => {}
        }
        if text.chars().count() > 4000 {
            break;
        }
    }
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let text: String = text
        .chars()
        .rev()
        .take(1200)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    Some(text.replace('\n', " · "))
}

async fn stop(events: &Sender<ExternalServiceEvent>, kind: ServiceKind) {
    let Some(docker) = connect_docker(events, kind).await else {
        return;
    };
    if docker
        .inspect_container(kind.container(), None)
        .await
        .is_err()
    {
        send_container(events, kind, ContainerStatus::Missing).await;
        return;
    }
    let options = StopContainerOptionsBuilder::default().t(10).build();
    match docker.stop_container(kind.container(), Some(options)).await {
        Ok(()) => send_container(events, kind, ContainerStatus::Stopped).await,
        Err(error) => {
            fail(
                events,
                kind,
                &format!("cannot stop the {} container: {error}", kind.label()),
            )
            .await;
        }
    }
}

async fn remove(events: &Sender<ExternalServiceEvent>, kind: ServiceKind) -> bool {
    let Some(docker) = connect_docker(events, kind).await else {
        return true;
    };
    let exists = docker
        .inspect_container(kind.container(), None)
        .await
        .is_ok();
    if !exists {
        send_container(events, kind, ContainerStatus::Missing).await;
        return false;
    }
    let stop_options = StopContainerOptionsBuilder::default().t(5).build();
    let _ = docker
        .stop_container(kind.container(), Some(stop_options))
        .await;
    let options = RemoveContainerOptionsBuilder::default().force(true).build();
    match docker
        .remove_container(kind.container(), Some(options))
        .await
    {
        Ok(()) => {
            send_container(events, kind, ContainerStatus::Missing).await;
            false
        }
        Err(error) => {
            fail(
                events,
                kind,
                &format!("cannot remove the {} container: {error}", kind.label()),
            )
            .await;
            true
        }
    }
}

async fn connect_docker(
    events: &Sender<ExternalServiceEvent>,
    kind: ServiceKind,
) -> Option<Docker> {
    events
        .send(ExternalServiceEvent::Docker(DockerStatus::Checking))
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
                .send(ExternalServiceEvent::Docker(DockerStatus::Available {
                    version,
                }))
                .await
                .ok();
            Some(docker)
        }
        Err(error) => {
            let message = docker_message(&error);
            events
                .send(ExternalServiceEvent::Docker(DockerStatus::Unavailable {
                    message: message.clone(),
                }))
                .await
                .ok();
            fail(events, kind, &message).await;
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

/// Whether something already listens on the service's loopback port.
///
/// A managed start must not pull a large image before discovering that the
/// port belongs to a server which is still loading.
async fn port_in_use(kind: ServiceKind) -> bool {
    tokio::time::timeout(
        Duration::from_millis(400),
        tokio::net::TcpStream::connect(("127.0.0.1", kind.port())),
    )
    .await
    .is_ok_and(|result| result.is_ok())
}

/// Turn a Docker connection error into an actionable message.
pub fn docker_message(error: &str) -> String {
    let lower = error.to_ascii_lowercase();
    if lower.contains("permission denied")
        || lower.contains("access denied")
        || lower.contains("access is denied")
    {
        "Docker is running but this user cannot access it. On Linux, run `sudo usermod -aG docker $USER`, then sign out and back in (or run `newgrp docker` for a terminal launch). On other systems, make sure Docker Desktop is running for this user.".to_owned()
    } else if lower.contains("connection refused")
        || lower.contains("is the docker daemon running")
        || lower.contains("cannot connect to the docker daemon")
    {
        "Docker is not running. Start Docker Desktop, or run `sudo systemctl start docker` on Linux, then press Refresh.".to_owned()
    } else if lower.contains("no such file or directory") {
        "Docker is not running or not installed. Install Docker Desktop (or the docker package), start it, then press Refresh.".to_owned()
    } else if lower.contains("timed out") || lower.contains("deadline exceeded") {
        "Docker did not answer in time. Check that the daemon is responsive, then press Refresh."
            .to_owned()
    } else {
        format!(
            "Docker is not reachable ({error}). Start Docker Desktop or the Docker daemon, then press Refresh."
        )
    }
}

/// Whether an error is the daemon refusing access to the Docker socket.
///
/// Container runtime errors can also contain "permission denied" (a missing
/// device, an unreadable bind mount, or a non-root image user). Only errors
/// that name the Docker socket or daemon are socket-access problems.
fn is_docker_access_error(error: &str) -> bool {
    let lower = error.to_ascii_lowercase();
    let names_daemon = lower.contains("docker daemon")
        || lower.contains("docker.sock")
        || lower.contains("docker_engine")
        || lower.contains("error during connect")
        || lower.contains("dial unix")
        || lower.contains("cannot connect to the docker");
    let denied = lower.contains("permission denied")
        || lower.contains("access denied")
        || lower.contains("access is denied")
        || lower.contains("connection refused")
        || lower.contains("no such file or directory");
    names_daemon && denied
}

fn start_message(kind: ServiceKind, error: &str, logs: Option<&str>) -> String {
    let lower = error.to_ascii_lowercase();
    if is_docker_access_error(error) {
        return docker_message(error);
    }
    let message = if lower.contains("mount")
        && (lower.contains("permission denied")
            || lower.contains("access denied")
            || lower.contains("not shared")
            || lower.contains("mounts denied"))
    {
        "Docker cannot read the model file. Share its folder with Docker Desktop, or move the .gguf file below your home directory, then press Start again.".to_owned()
    } else if lower.contains("port is already allocated")
        || lower.contains("address already in use")
    {
        format!(
            "port {} is already in use. Stop the process using it, or set {} to Existing server.",
            kind.port(),
            kind.label()
        )
    } else if lower.contains("exec format error") {
        format!(
            "Docker cannot run the {} image on this host. Choose an image built for this platform.",
            kind.label()
        )
    } else if lower.contains("permission denied")
        || lower.contains("access denied")
        || lower.contains("operation not permitted")
    {
        match kind {
            ServiceKind::Ollama => "The Ollama container could not start because its process was denied access to a file or device. A custom image often needs devices such as /dev/dri (SYCL) or a different user; the managed container only publishes port 11434 and mounts its own model volume. Use Existing server for a container you configured by hand.".to_owned(),
            ServiceKind::LlamaCpp => "The llama.cpp container could not start because its process was denied access to a file or device. Check the model file permissions and the container output.".to_owned(),
            ServiceKind::Grobid => "The GROBID container could not start because its process was denied access to a file or device. Check the container output.".to_owned(),
        }
    } else {
        format!("cannot start the {} container: {error}", kind.label())
    };
    with_logs(message, logs)
}

fn with_logs(mut message: String, logs: Option<&str>) -> String {
    if let Some(logs) = logs.map(str::trim).filter(|logs| !logs.is_empty()) {
        message.push_str(" Last container output: ");
        message.push_str(logs);
    }
    message
}

fn pull_message(kind: ServiceKind, image: &str, error: &str) -> String {
    let lower = error.to_ascii_lowercase();
    if is_docker_access_error(error) {
        return docker_message(error);
    }
    if lower.contains("no matching manifest")
        || lower.contains("does not match the detected host platform")
    {
        return format!(
            "the image '{image}' has no build for this platform. Choose a multi-architecture image for {}.",
            kind.label()
        );
    }
    if lower.contains("unauthorized")
        || lower.contains("pull access denied")
        || lower.contains("authentication required")
    {
        return format!(
            "Docker could not pull '{image}'. Check the image name and tag with `docker images`; a local image needs its tag (for example `ollama-sycl:local`), and a private registry needs `docker login`. If '{image}' is a container you created by hand, start it with Docker and select Existing server."
        );
    }
    format!("cannot pull '{image}': {error}")
}

fn create_message(kind: ServiceKind, error: &str) -> String {
    if is_docker_access_error(error) {
        docker_message(error)
    } else {
        format!("cannot create the {} container: {error}", kind.label())
    }
}

async fn fail(events: &Sender<ExternalServiceEvent>, kind: ServiceKind, message: &str) {
    events
        .send(ExternalServiceEvent::Container {
            service: kind,
            status: ContainerStatus::Failed {
                message: message.to_owned(),
            },
        })
        .await
        .ok();
    notice(events, kind, message).await;
}

async fn notice(events: &Sender<ExternalServiceEvent>, kind: ServiceKind, message: &str) {
    events
        .send(ExternalServiceEvent::Failed {
            service: kind,
            message: message.to_owned(),
        })
        .await
        .ok();
}

/// Container lifecycle events are droppable: a stalled UI must never block the
/// Docker worker while the app is shutting down and no longer drains the event
/// channel.
async fn send_container(
    events: &Sender<ExternalServiceEvent>,
    kind: ServiceKind,
    status: ContainerStatus,
) {
    events
        .send(ExternalServiceEvent::Container {
            service: kind,
            status,
        })
        .await
        .ok();
}

/// Image and model progress may be dropped; a full channel must not stall a
/// pull.
async fn progress_container(
    events: &Sender<ExternalServiceEvent>,
    kind: ServiceKind,
    status: ContainerStatus,
) {
    let _ = events.try_send(ExternalServiceEvent::Container {
        service: kind,
        status,
    });
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

// Backwards-compatible names for the GROBID-only service.
/// Backwards-compatible alias for the shared Docker status.
#[cfg(feature = "grobid")]
pub type GrobidDockerStatus = DockerStatus;
/// Backwards-compatible alias for the shared container status.
#[cfg(feature = "grobid")]
pub type GrobidContainerStatus = ContainerStatus;
/// Backwards-compatible alias for the shared probe result.
#[cfg(feature = "grobid")]
pub type GrobidProbe = Probe;
/// Backwards-compatible alias for the shared probe status.
#[cfg(feature = "grobid")]
pub type GrobidProbeStatus = ProbeStatus;
/// Backwards-compatible alias for the shared event type.
#[cfg(feature = "grobid")]
pub type GrobidServiceEvent = ExternalServiceEvent;
/// Backwards-compatible alias for the shared controller.
#[cfg(feature = "grobid")]
pub type GrobidService = ExternalServices;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::spec::{
        GROBID_DISCOVERY_PORTS, MANAGED_GROBID_CONTAINER, MANAGED_GROBID_PORT, MANAGED_GROBID_URL,
    };

    #[test]
    fn docker_reports_a_stopped_daemon() {
        let message = docker_message(
            "Cannot connect to the Docker daemon at unix:///var/run/docker.sock. Is the docker daemon running?",
        );
        assert!(message.contains("not running"), "{message}");
        assert!(message.contains("Refresh"), "{message}");
    }

    #[test]
    fn docker_reports_missing_permissions() {
        let error = "Got permission denied while trying to connect to the Docker daemon socket";
        let message = docker_message(error);
        assert!(message.contains("usermod -aG docker"), "{message}");
        assert!(message.contains("sign out"), "{message}");
    }

    #[test]
    fn docker_reports_a_missing_socket() {
        let message = docker_message(
            "error connecting to unix socket: /var/run/docker.sock: No such file or directory",
        );
        assert!(message.contains("not installed"), "{message}");
        assert!(message.contains("Refresh"), "{message}");
    }

    #[test]
    fn unknown_docker_errors_keep_the_detail() {
        let message = docker_message("something unexpected");
        assert!(message.contains("something unexpected"), "{message}");
    }

    #[test]
    fn port_conflicts_name_the_service_and_the_fix() {
        let message = start_message(
            ServiceKind::Ollama,
            "Bind for 127.0.0.1:11434 failed: port is already allocated",
            None,
        );
        assert!(message.contains("11434"), "{message}");
        assert!(message.contains("Existing server"), "{message}");
    }

    #[test]
    fn mount_permission_errors_name_the_model_file() {
        let message = start_message(
            ServiceKind::LlamaCpp,
            "error while creating mount source path '/models/qwen.gguf': mkdir /models: permission denied",
            None,
        );
        assert!(message.contains("model file"), "{message}");
        assert!(message.contains("Docker Desktop"), "{message}");
    }

    #[test]
    fn pull_access_denied_is_not_a_docker_socket_error() {
        let message = pull_message(
            ServiceKind::Ollama,
            "ollama-sycl",
            "Error response from daemon: pull access denied for ollama-sycl, repository does not exist or may require 'docker login'",
        );
        assert!(!message.contains("usermod"), "{message}");
        assert!(message.contains("docker images"), "{message}");
        assert!(message.contains("Existing server"), "{message}");
    }

    #[test]
    fn a_container_runtime_permission_error_does_not_blame_the_socket() {
        let message = start_message(
            ServiceKind::Ollama,
            "failed to create task for container: error gathering device information while adding custom device \"/dev/dri\": permission denied",
            Some("SYCL device not found"),
        );
        assert!(!message.contains("usermod"), "{message}");
        assert!(message.contains("device"), "{message}");
        assert!(message.contains("SYCL device not found"), "{message}");
    }

    #[test]
    fn a_docker_socket_permission_error_names_the_group_fix() {
        let message = start_message(
            ServiceKind::Ollama,
            "Got permission denied while trying to connect to the Docker daemon socket at unix:///var/run/docker.sock",
            None,
        );
        assert!(message.contains("usermod -aG docker"), "{message}");
    }

    #[test]
    fn container_summaries_include_progress() {
        let status = ContainerStatus::Pulling {
            image: "grobid/grobid:0.9.1-full".to_owned(),
            received: 512 * 1024 * 1024,
            total: 1024 * 1024 * 1024,
        };
        assert!(status.summary().contains("50%"));
        assert!(status.summary().contains("512.0 MB"));

        let status = ContainerStatus::Provisioning {
            model: "qwen3.5:9b".to_owned(),
            received: 0,
            total: 0,
        };
        assert!(status.summary().contains("qwen3.5:9b"));
    }

    #[test]
    fn managed_container_names_are_distinct() {
        let names: Vec<_> = ServiceKind::ALL
            .iter()
            .map(|kind| kind.container())
            .collect();
        assert_eq!(
            names,
            vec!["ragrig-grobid", "ragrig-ollama", "ragrig-llamacpp"]
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore = "requires Docker and pulls the Ollama image; run with --ignored"]
    async fn the_managed_ollama_container_starts_answers_and_stops() {
        let docker = connect_docker_quiet().await.expect("docker daemon");
        if docker
            .inspect_container(ServiceKind::Ollama.container(), None)
            .await
            .is_ok()
        {
            eprintln!(
                "skipping: {} already exists",
                ServiceKind::Ollama.container()
            );
            return;
        }

        let image = std::env::var("RAGRIG_OLLAMA_TEST_IMAGE")
            .unwrap_or_else(|_| ServiceKind::Ollama.default_image().to_owned());
        if port_in_use(ServiceKind::Ollama).await {
            eprintln!(
                "skipping: port {} is already in use",
                ServiceKind::Ollama.port()
            );
            return;
        }
        let (commands_tx, commands) = async_channel::bounded(16);
        let (events_tx, events) = async_channel::bounded(256);
        let launch = ServiceLaunch {
            image: image.clone(),
            ..ServiceLaunch::default()
        };
        let client = Client::builder()
            .connect_timeout(Duration::from_secs(4))
            .build()
            .expect("client");
        let cancel = AtomicBool::new(false);

        pull_image(
            &docker,
            ServiceKind::Ollama,
            &image,
            &events_tx,
            &commands,
            &cancel,
        )
        .await
        .expect("pull");
        create_container(&docker, ServiceKind::Ollama, &launch)
            .await
            .expect("create");
        if let Err(error) = docker
            .start_container(ServiceKind::Ollama.container(), None)
            .await
        {
            let options = RemoveContainerOptionsBuilder::default().force(true).build();
            let _ = docker
                .remove_container(ServiceKind::Ollama.container(), Some(options))
                .await;
            panic!("start: {error}");
        }
        let outcome = wait_until_ready(
            &client,
            &docker,
            &events_tx,
            ServiceKind::Ollama,
            &commands,
            &cancel,
        )
        .await;
        stop(&events_tx, ServiceKind::Ollama).await;
        let removed = remove(&events_tx, ServiceKind::Ollama).await;

        assert!(matches!(outcome, StartOutcome::Ready));
        assert!(
            std::iter::from_fn(|| events.try_recv().ok()).any(|event| matches!(
                event,
                ExternalServiceEvent::Ready {
                    service: ServiceKind::Ollama,
                    managed: true,
                    ..
                }
            )),
            "the managed container never reported ready"
        );
        assert!(!removed);
        drop(commands_tx);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore = "requires Docker and a server on the Ollama port; run with --ignored"]
    async fn an_existing_ollama_is_adopted_without_a_container() {
        let docker = connect_docker_quiet().await.expect("docker daemon");
        if docker
            .inspect_container(ServiceKind::Ollama.container(), None)
            .await
            .is_ok()
        {
            eprintln!(
                "skipping: {} already exists",
                ServiceKind::Ollama.container()
            );
            return;
        }
        if !port_in_use(ServiceKind::Ollama).await {
            eprintln!(
                "skipping: nothing listens on port {}",
                ServiceKind::Ollama.port()
            );
            return;
        }

        let (commands_tx, commands) = async_channel::bounded(16);
        let (events_tx, events) = async_channel::bounded(64);
        let client = Client::builder()
            .connect_timeout(Duration::from_secs(4))
            .build()
            .expect("client");
        let launch = ServiceLaunch {
            image: ServiceKind::Ollama.default_image().to_owned(),
            ..ServiceLaunch::default()
        };

        let owned = pull_and_start(
            &client,
            &launch,
            ServiceKind::Ollama,
            &events_tx,
            &commands,
            &AtomicBool::new(false),
        )
        .await;

        assert!(!owned, "an adopted server must not be owned");
        assert!(
            std::iter::from_fn(|| events.try_recv().ok()).any(|event| matches!(
                event,
                ExternalServiceEvent::Ready {
                    service: ServiceKind::Ollama,
                    managed: false,
                    ..
                }
            )),
            "the existing server was not adopted"
        );
        assert!(
            docker
                .inspect_container(ServiceKind::Ollama.container(), None)
                .await
                .is_err(),
            "no managed container should have been created"
        );
        drop(commands_tx);
    }

    #[test]
    fn grobid_only_constants_are_re_exported() {
        assert_eq!(MANAGED_GROBID_CONTAINER, "ragrig-grobid");
        assert_eq!(MANAGED_GROBID_PORT, 8070);
        assert_eq!(MANAGED_GROBID_URL, "http://127.0.0.1:8070");
        assert_eq!(GROBID_DISCOVERY_PORTS, [8070, 8080]);
    }
}
