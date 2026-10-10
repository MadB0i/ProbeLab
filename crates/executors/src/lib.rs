//! Universal execution modes for ProbeLab.
//!
//! # Why this crate exists
//!
//! The laboratory could only measure models behind one HTTP protocol. A user
//! with a local `llama.cpp` server, an installed coding agent, or no network
//! at all had nothing to select. This crate describes *how* an evaluation can
//! be executed without claiming it can be executed: every descriptor carries
//! an [`Availability`] that names the reason when it is absent, and discovery
//! verifies rather than assumes.
//!
//! # The four modes
//!
//! * [`ExecutionMode::OfflineLocal`] — a model runtime on this machine
//!   (Ollama, `llama-server`). No Internet, no API key, no downloads.
//! * [`ExecutionMode::CodingAgent`] — an installed CLI agent (Codex, OpenCode,
//!   Claude Code) driven through its documented non-interactive interface.
//! * [`ExecutionMode::GuidedExternal`] — an isolated task package the user
//!   runs elsewhere; outputs are imported and independently verified. Always
//!   available, always marked as manually submitted evidence.
//! * [`ExecutionMode::RemoteProvider`] — the existing HTTP adapters. Optional,
//!   never mandatory.
//!
//! # What this crate will not do
//!
//! * Never download model files. Discovery lists what is installed; fetching
//!   gigabytes on a user's behalf is not discovery.
//! * Never read credentials. Auth detection checks for the *existence* of a
//!   config file or variable name, never its contents.
//! * Never run an agent CLI automatically without an explicit task call. The
//!   only automatic process execution in this crate is `preflight`, which runs
//!   `<binary> --version` with a bounded timeout and no task input.

#![deny(missing_docs)]

pub mod isolation;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// How an evaluation can be executed.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionMode {
    /// A model runtime on this machine. No network, no key.
    OfflineLocal,
    /// An installed CLI coding agent, driven non-interactively.
    CodingAgent,
    /// A task package executed externally, with imported evidence.
    GuidedExternal,
    /// An HTTP model provider. Optional, never mandatory.
    RemoteProvider,
}

impl ExecutionMode {
    /// A short stable identifier.
    pub fn as_str(self) -> &'static str {
        match self {
            ExecutionMode::OfflineLocal => "offline_local",
            ExecutionMode::CodingAgent => "coding_agent",
            ExecutionMode::GuidedExternal => "guided_external",
            ExecutionMode::RemoteProvider => "remote_provider",
        }
    }
}

/// Whether an executor can run right now, and why not when it cannot.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "state")]
pub enum Availability {
    /// Verified present (binary found, socket reachable, file listed).
    Available {
        /// What was verified, in one line.
        evidence: String,
    },
    /// Absent or unverified, with the reason and what would change it.
    Unavailable {
        /// Why it cannot run.
        reason: String,
        /// What the user would need to do, or `None` when nothing local helps.
        remedy: Option<String>,
    },
}

impl Availability {
    /// Whether the executor can run.
    pub fn is_available(&self) -> bool {
        matches!(self, Availability::Available { .. })
    }
}

/// One executable thing the user can select.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ExecutorDescriptor {
    /// Stable identifier, e.g. `ollama-local`, `codex-cli`.
    pub id: String,
    /// Human label.
    pub label: String,
    /// Which of the four modes this belongs to.
    pub mode: ExecutionMode,
    /// Verified presence or the honest reason for absence.
    pub availability: Availability,
    /// What this executor can do, as stable capability strings.
    pub capabilities: Vec<String>,
    /// One line of extra detail (endpoint, binary path, package kind).
    pub detail: String,
}

/// Discovers every executor this machine may offer.
///
/// Verification, not assumption: a binary must be found on `PATH`, a socket
/// must accept a connection within the timeout, a model file must exist on
/// disk. Anything unverified is reported as unavailable with its reason —
/// never as available on the strength of documentation.
pub fn discover_executors() -> Vec<ExecutorDescriptor> {
    let mut executors = Vec::new();
    executors.extend(discover_offline_runtimes());
    executors.extend(discover_coding_agents());
    executors.push(guided_executor());
    executors.extend(discover_remote_providers());
    executors
}

/// Runs the safe preflight for one executor: `<binary> --version` with a
/// bounded timeout, or a loopback socket connect. No task input, no network
/// beyond loopback, no credentials touched.
pub fn preflight_executor(id: &str) -> PreflightReport {
    for executor in discover_executors() {
        if executor.id == id {
            return run_preflight(&executor);
        }
    }
    PreflightReport {
        id: id.to_owned(),
        passed: false,
        checks: vec![PreflightCheck {
            name: "executor_known".to_owned(),
            passed: false,
            detail: format!("no executor named {id:?} was discovered"),
        }],
    }
}

/// One named check inside a preflight report.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct PreflightCheck {
    /// Stable check name.
    pub name: String,
    /// Whether it passed.
    pub passed: bool,
    /// Human detail.
    pub detail: String,
}

/// The outcome of [`preflight_executor`].
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct PreflightReport {
    /// The executor that was checked.
    pub id: String,
    /// Whether every check passed.
    pub passed: bool,
    /// The individual checks.
    pub checks: Vec<PreflightCheck>,
}

// ---------------------------------------------------------------------------
// Offline local runtimes
// ---------------------------------------------------------------------------

/// Candidate GGUF search roots, in order. Only the user's own model
/// directories; never a whole-drive walk.
fn model_search_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(home) = home_dir() {
        roots.push(home.join(".ollama").join("models"));
        roots.push(home.join(".cache").join("llama.cpp"));
        roots.push(home.join("models"));
    }
    roots.into_iter().filter(|root| root.is_dir()).collect()
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
}

/// Lists installed `.gguf` files, bounded: depth 2, at most 64 entries, so a
/// large model directory cannot stall discovery.
pub fn list_local_model_files() -> Vec<PathBuf> {
    const MAX_FILES: usize = 64;
    let mut found = Vec::new();
    for root in model_search_roots() {
        list_gguf_depth(&root, 0, &mut found, MAX_FILES);
        if found.len() >= MAX_FILES {
            break;
        }
    }
    found
}

fn list_gguf_depth(dir: &Path, depth: usize, found: &mut Vec<PathBuf>, max: usize) {
    if depth > 2 || found.len() >= max {
        return;
    }
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        if found.len() >= max {
            return;
        }
        let path = entry.path();
        if path.is_dir() {
            list_gguf_depth(&path, depth + 1, found, max);
        } else if path
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("gguf"))
        {
            found.push(path);
        }
    }
}

/// Finds a binary on `PATH` without executing a shell.
pub fn find_on_path(name: &str) -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        if dir.as_os_str().is_empty() {
            continue;
        }
        #[cfg(windows)]
        {
            for candidate in [dir.join(format!("{name}.exe")), dir.join(name)] {
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }
        #[cfg(not(windows))]
        {
            let candidate = dir.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// Whether a loopback TCP port accepts a connection within the timeout.
///
/// A connectivity check only: an open port is not evidence of a protocol.
/// Use [`openai_compatible_models`] when the question is whether an
/// OpenAI-compatible server is actually there.
pub fn loopback_port_open(port: u16, timeout: Duration) -> bool {
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};
    let address = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port);
    std::net::TcpStream::connect_timeout(&address, timeout).is_ok()
}

/// Lists the model identifiers a loopback OpenAI-compatible server reports.
///
/// Read-only `GET /v1/models`, loopback only, bounded (at most 128 names of
/// at most 128 characters each), no credentials, no task input. Returns an
/// empty list when the port does not speak the protocol — absence of models
/// and absence of a server are both "nothing to select", and the discovery
/// availability (which distinguishes them) is the place that says which.
pub fn local_model_ids(port: u16, timeout: Duration) -> Vec<String> {
    fetch_models_document(port, timeout)
        .and_then(|json| {
            json.get("data")
                .and_then(|data| data.as_array())
                .map(|models| {
                    models
                        .iter()
                        .filter_map(|model| model.get("id").and_then(|id| id.as_str()))
                        .take(128)
                        .map(|id| id.chars().take(128).collect())
                        .collect()
                })
        })
        .unwrap_or_default()
}

/// Fetches and parses the `GET /v1/models` response body, or `None` when
/// the port does not serve the OpenAI-compatible protocol.
fn fetch_models_document(port: u16, timeout: Duration) -> Option<serde_json::Value> {
    use std::io::{Read, Write};
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};
    let address = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port);
    let mut stream = std::net::TcpStream::connect_timeout(&address, timeout).ok()?;
    stream
        .set_read_timeout(Some(timeout))
        .and_then(|()| stream.set_write_timeout(Some(timeout)))
        .ok()?;
    stream
        .write_all(b"GET /v1/models HTTP/1.0\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n")
        .ok()?;
    let mut response = Vec::with_capacity(8192);
    let mut chunk = [0u8; 4096];
    let mut total = 0usize;
    loop {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(read) => {
                total += read;
                if total > 256 * 1024 {
                    return None;
                }
                response.extend_from_slice(&chunk[..read]);
            }
            Err(_) => break,
        }
    }
    let text = String::from_utf8_lossy(&response);
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
    let status: u16 = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse().ok())
        .unwrap_or(0);
    if !(200..300).contains(&status) {
        return None;
    }
    serde_json::from_str(body).ok()
}
///
/// Sends `GET /v1/models` and accepts only a 2xx response with a JSON body.
/// Returns the number of listed models when the body carries a `data` array,
/// or `Some(0)` when the server speaks the protocol but lists none.
///
/// A bare TCP connect is not enough: on the audited host, port 8080 answers
/// TCP but 404s on `/v1/models` — something else lives there, and reporting
/// it as a model runtime would be a false available. This probe is read-only
/// and loopback-only; it sends no credentials and no task input.
pub fn openai_compatible_models(port: u16, timeout: Duration) -> Option<usize> {
    let json = fetch_models_document(port, timeout)?;
    match json.get("data").and_then(|data| data.as_array()) {
        Some(models) => Some(models.len()),
        // Speaks the protocol (2xx + JSON) but lists no models: still a
        // compatible server, with nothing loaded.
        None => Some(0),
    }
}

fn discover_offline_runtimes() -> Vec<ExecutorDescriptor> {
    let mut executors = Vec::new();
    let gguf_files = list_local_model_files();
    let gguf_detail = if gguf_files.is_empty() {
        "no .gguf files found in the searched model directories".to_owned()
    } else {
        format!("{} .gguf file(s) installed", gguf_files.len())
    };

    // Ollama: the server must speak the protocol, not merely answer TCP.
    let ollama_binary = find_on_path("ollama");
    let ollama_models = openai_compatible_models(11434, Duration::from_millis(500));
    let ollama_availability = match (&ollama_binary, ollama_models) {
        (Some(_), Some(count)) | (None, Some(count)) => Availability::Available {
            evidence: format!(
                "an OpenAI-compatible server on 127.0.0.1:11434 listing {count} model(s)"
            ),
        },
        (Some(path), None) => {
            let open = loopback_port_open(11434, Duration::from_millis(300));
            Availability::Unavailable {
                reason: if open {
                    "something answers on 127.0.0.1:11434 but does not speak the OpenAI-compatible protocol"
                        .to_owned()
                } else {
                    format!(
                        "`ollama` found at {} but no server is listening on 127.0.0.1:11434",
                        path.display()
                    )
                },
                remedy: Some(
                    "run `ollama serve` (and `ollama pull` a model), then preflight again"
                        .to_owned(),
                ),
            }
        }
        (None, None) => Availability::Unavailable {
            reason: "no `ollama` binary on PATH and no OpenAI-compatible server on 127.0.0.1:11434"
                .to_owned(),
            remedy: Some(
                "install Ollama or start any OpenAI-compatible server on 127.0.0.1:11434"
                    .to_owned(),
            ),
        },
    };
    executors.push(ExecutorDescriptor {
        id: "ollama-local".to_owned(),
        label: "Ollama (local)".to_owned(),
        mode: ExecutionMode::OfflineLocal,
        availability: ollama_availability,
        capabilities: vec![
            "chat_completions".to_owned(),
            "offline".to_owned(),
            "no_api_key".to_owned(),
        ],
        detail: format!("OpenAI-compatible via 127.0.0.1:11434. {gguf_detail}."),
    });
    // llama.cpp server: the server must speak the protocol, not merely answer TCP.
    let llama_binary = find_on_path("llama-server").or_else(|| find_on_path("server"));
    let llama_models = openai_compatible_models(8080, Duration::from_millis(500));
    let llama_availability = match (&llama_binary, llama_models) {
        (Some(_), Some(count)) | (None, Some(count)) => Availability::Available {
            evidence: format!(
                "an OpenAI-compatible server on 127.0.0.1:8080 listing {count} model(s)"
            ),
        },
        (Some(path), None) => Availability::Unavailable {
            reason: format!(
                "`llama-server` found at {} but no OpenAI-compatible server answers on 127.0.0.1:8080",
                path.display()
            ),
            remedy: Some(
                "start `llama-server` with a model loaded, then preflight again".to_owned(),
            ),
        },
        (None, None) => {
            let open = loopback_port_open(8080, Duration::from_millis(300));
            Availability::Unavailable {
                reason: if open {
                    "something answers on 127.0.0.1:8080 but does not speak the OpenAI-compatible protocol"
                        .to_owned()
                } else {
                    "no `llama-server` binary on PATH and nothing on 127.0.0.1:8080".to_owned()
                },
                remedy: Some(
                    "install llama.cpp or point any OpenAI-compatible server at 127.0.0.1:8080"
                        .to_owned(),
                ),
            }
        }
    };
    executors.push(ExecutorDescriptor {
        id: "llamacpp-local".to_owned(),
        label: "llama.cpp server (local)".to_owned(),
        mode: ExecutionMode::OfflineLocal,
        availability: llama_availability,
        capabilities: vec![
            "chat_completions".to_owned(),
            "offline".to_owned(),
            "no_api_key".to_owned(),
            "gguf".to_owned(),
        ],
        detail: format!("OpenAI-compatible via 127.0.0.1:8080. {gguf_detail}; files are never downloaded by ProbeLab."),
    });
    executors
}

// ---------------------------------------------------------------------------
// Coding agents
// ---------------------------------------------------------------------------

/// A supported CLI coding agent.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CodingAgentKind {
    /// OpenAI Codex CLI (`codex`).
    Codex,
    /// OpenCode (`opencode`).
    OpenCode,
    /// Claude Code (`claude`).
    ClaudeCode,
}

impl CodingAgentKind {
    /// The binary name on `PATH`.
    pub fn binary(self) -> &'static str {
        match self {
            CodingAgentKind::Codex => "codex",
            CodingAgentKind::OpenCode => "opencode",
            CodingAgentKind::ClaudeCode => "claude",
        }
    }

    /// Stable executor id.
    pub fn executor_id(self) -> &'static str {
        match self {
            CodingAgentKind::Codex => "codex-cli",
            CodingAgentKind::OpenCode => "opencode-cli",
            CodingAgentKind::ClaudeCode => "claude-code-cli",
        }
    }

    /// Human label.
    pub fn label(self) -> &'static str {
        match self {
            CodingAgentKind::Codex => "OpenAI Codex CLI",
            CodingAgentKind::OpenCode => "OpenCode",
            CodingAgentKind::ClaudeCode => "Claude Code",
        }
    }

    /// All supported agents.
    pub fn all() -> [CodingAgentKind; 3] {
        [
            CodingAgentKind::Codex,
            CodingAgentKind::OpenCode,
            CodingAgentKind::ClaudeCode,
        ]
    }

    /// The documented non-interactive invocation shape, for display.
    /// These are shapes, not commands ProbeLab runs by itself: the task
    /// runner builds the exact argv from the task and the isolation policy.
    pub fn non_interactive_shape(self) -> &'static str {
        match self {
            CodingAgentKind::Codex => "codex exec <task> (non-interactive)",
            CodingAgentKind::OpenCode => "opencode run <task> (non-interactive)",
            CodingAgentKind::ClaudeCode => "claude -p <task> (print mode)",
        }
    }

    /// Whether a config directory exists for this agent. Existence only —
    /// the contents are never read, because they may hold credentials.
    pub fn auth_config_present(self) -> bool {
        let Some(home) = home_dir() else {
            return false;
        };
        let candidates: &[&str] = match self {
            CodingAgentKind::Codex => &[".codex"],
            CodingAgentKind::OpenCode => &[".config/opencode", ".opencode"],
            CodingAgentKind::ClaudeCode => &[".claude", ".config/claude"],
        };
        candidates.iter().any(|relative| {
            let path: PathBuf = relative
                .split('/')
                .fold(home.clone(), |base, part| base.join(part));
            path.is_dir() || path.is_file()
        })
    }
}

fn discover_coding_agents() -> Vec<ExecutorDescriptor> {
    CodingAgentKind::all()
        .iter()
        .map(|kind| {
            let binary = find_on_path(kind.binary());
            let auth = kind.auth_config_present();
            let availability = match (&binary, auth) {
                (Some(path), true) => Availability::Available {
                    evidence: format!(
                        "`{}` at {} with existing user configuration",
                        kind.binary(),
                        path.display()
                    ),
                },
                (Some(path), false) => Availability::Unavailable {
                    reason: format!(
                        "`{}` found at {} but no user configuration was detected",
                        kind.binary(),
                        path.display()
                    ),
                    remedy: Some(
                        "complete the agent's own login/setup, then preflight again".to_owned(),
                    ),
                },
                (None, _) => Availability::Unavailable {
                    reason: format!(
                        "no `{}` binary on PATH{}",
                        kind.binary(),
                        if auth {
                            " (configuration exists but the binary is missing)"
                        } else {
                            ""
                        }
                    ),
                    remedy: Some(format!(
                        "install {} or use guided external mode",
                        kind.label()
                    )),
                },
            };
            ExecutorDescriptor {
                id: kind.executor_id().to_owned(),
                label: kind.label().to_owned(),
                mode: ExecutionMode::CodingAgent,
                availability,
                capabilities: vec![
                    "non_interactive_task".to_owned(),
                    "workspace_execution".to_owned(),
                    "user_auth".to_owned(),
                ],
                detail: format!(
                    "{}; {}",
                    kind.non_interactive_shape(),
                    if auth {
                        "user authentication detected (existence only, never read)"
                    } else {
                        "no user authentication detected"
                    }
                ),
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Guided external + remote
// ---------------------------------------------------------------------------

fn guided_executor() -> ExecutorDescriptor {
    ExecutorDescriptor {
        id: "guided-external".to_owned(),
        label: "Guided external evaluation".to_owned(),
        mode: ExecutionMode::GuidedExternal,
        availability: Availability::Available {
            evidence: "task packages are generated locally; no installation required".to_owned(),
        },
        capabilities: vec![
            "task_package".to_owned(),
            "manual_evidence_import".to_owned(),
            "independent_verification".to_owned(),
        ],
        detail: "For models without an automated interface: run the package elsewhere, import outputs, verify independently.".to_owned(),
    }
}

fn discover_remote_providers() -> Vec<ExecutorDescriptor> {
    let providers = [
        (
            "remote-openai-compatible",
            "Remote OpenAI-compatible",
            "OPENAI_COMPATIBLE_API_KEY",
        ),
        ("remote-anthropic", "Remote Anthropic", "ANTHROPIC_API_KEY"),
        ("remote-gemini", "Remote Gemini", "GEMINI_API_KEY"),
    ];
    providers
        .iter()
        .map(|(id, label, key_env)| {
            let configured = std::env::var_os(key_env).is_some_and(|value| !value.is_empty());
            ExecutorDescriptor {
                id: (*id).to_owned(),
                label: (*label).to_owned(),
                mode: ExecutionMode::RemoteProvider,
                availability: if configured {
                    Availability::Available {
                        evidence: format!(
                            "credential variable {key_env} is set (value never read by discovery)"
                        ),
                    }
                } else {
                    Availability::Unavailable {
                        reason: format!("credential variable {key_env} is not set"),
                        remedy: Some(format!(
                            "set {key_env} in the environment; API keys are never stored by ProbeLab"
                        )),
                    }
                },
                capabilities: vec![
                    "chat_completions".to_owned(),
                    "api_key_env".to_owned(),
                    "optional".to_owned(),
                ],
                detail: "Optional remote evaluation. API keys are never mandatory for the product.".to_owned(),
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Preflight
// ---------------------------------------------------------------------------

fn run_preflight(executor: &ExecutorDescriptor) -> PreflightReport {
    let mut checks = Vec::new();
    match executor.mode {
        ExecutionMode::GuidedExternal => {
            checks.push(PreflightCheck {
                name: "package_generator".to_owned(),
                passed: true,
                detail: "guided packages are generated from local benchmark definitions".to_owned(),
            });
        }
        ExecutionMode::RemoteProvider => {
            let available = executor.availability.is_available();
            checks.push(PreflightCheck {
                name: "credential_configured".to_owned(),
                passed: available,
                detail: executor.detail.clone(),
            });
        }
        ExecutionMode::OfflineLocal => {
            let port = if executor.id == "ollama-local" {
                11434
            } else {
                8080
            };
            // The protocol, not the port: a TCP answer from something else
            // (measured on this host's 8080) must not read as a model runtime.
            let models = openai_compatible_models(port, Duration::from_secs(2));
            checks.push(PreflightCheck {
                name: "loopback_server".to_owned(),
                passed: models.is_some(),
                detail: match models {
                    Some(count) => format!(
                        "127.0.0.1:{port} speaks the OpenAI-compatible protocol ({count} model(s) listed)"
                    ),
                    None => {
                        if loopback_port_open(port, Duration::from_millis(300)) {
                            format!("127.0.0.1:{port} answers TCP but not the OpenAI-compatible protocol")
                        } else {
                            format!("nothing listening on 127.0.0.1:{port}")
                        }
                    }
                },
            });
            let binary = if executor.id == "ollama-local" {
                find_on_path("ollama")
            } else {
                find_on_path("llama-server").or_else(|| find_on_path("server"))
            };
            checks.push(PreflightCheck {
                name: "runtime_binary".to_owned(),
                passed: binary.is_some(),
                detail: binary
                    .map(|path| format!("found at {}", path.display()))
                    .unwrap_or_else(|| "not on PATH".to_owned()),
            });
            if executor.id == "llamacpp-local" {
                let files = list_local_model_files();
                checks.push(PreflightCheck {
                    name: "model_files".to_owned(),
                    passed: !files.is_empty(),
                    detail: if files.is_empty() {
                        "no .gguf files found; ProbeLab never downloads models".to_owned()
                    } else {
                        format!("{} .gguf file(s) installed", files.len())
                    },
                });
            }
        }
        ExecutionMode::CodingAgent => {
            let all = CodingAgentKind::all();
            let kind = all.iter().find(|kind| kind.executor_id() == executor.id);
            match kind {
                Some(kind) => {
                    let binary = find_on_path(kind.binary());
                    match binary {
                        Some(path) => {
                            checks.push(PreflightCheck {
                                name: "agent_binary".to_owned(),
                                passed: true,
                                detail: format!("found at {}", path.display()),
                            });
                            checks.push(version_check(*kind, &path));
                            checks.push(PreflightCheck {
                                name: "user_auth".to_owned(),
                                passed: kind.auth_config_present(),
                                detail: if kind.auth_config_present() {
                                    "user configuration exists (existence only)".to_owned()
                                } else {
                                    "no user configuration detected; complete the agent's own setup"
                                        .to_owned()
                                },
                            });
                        }
                        None => checks.push(PreflightCheck {
                            name: "agent_binary".to_owned(),
                            passed: false,
                            detail: format!("no `{}` on PATH", kind.binary()),
                        }),
                    }
                }
                None => checks.push(PreflightCheck {
                    name: "executor_known".to_owned(),
                    passed: false,
                    detail: "unknown coding-agent executor".to_owned(),
                }),
            }
        }
    }
    let passed = checks.iter().all(|check| check.passed);
    PreflightReport {
        id: executor.id.clone(),
        passed,
        checks,
    }
}

/// Runs `<binary> --version` with a bounded timeout. The only automatic
/// process execution discovery performs.
fn version_check(kind: CodingAgentKind, path: &Path) -> PreflightCheck {
    let spawned = std::process::Command::new(path)
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn();
    let mut child = match spawned {
        Ok(child) => child,
        Err(error) => {
            return PreflightCheck {
                name: "agent_version".to_owned(),
                passed: false,
                detail: format!("could not run `{} --version`: {error}", kind.binary()),
            };
        }
    };
    // Stable Rust has no `Command::timeout`; poll `try_wait` against a
    // deadline instead. A version flag that hangs past 15s fails the check.
    let deadline = Duration::from_secs(15);
    let start = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let mut stdout = Vec::new();
                use std::io::Read;
                if let Some(mut pipe) = child.stdout.take() {
                    let _ = pipe.read_to_end(&mut stdout);
                }
                if status.success() {
                    let text = String::from_utf8_lossy(&stdout);
                    let first_line: String = text
                        .lines()
                        .next()
                        .unwrap_or("")
                        .chars()
                        .take(120)
                        .collect();
                    return PreflightCheck {
                        name: "agent_version".to_owned(),
                        passed: true,
                        detail: format!("`{} --version` succeeded: {first_line}", kind.binary()),
                    };
                }
                return PreflightCheck {
                    name: "agent_version".to_owned(),
                    passed: false,
                    detail: format!("`{} --version` exited with {status}", kind.binary()),
                };
            }
            Ok(None) => {
                if start.elapsed() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return PreflightCheck {
                        name: "agent_version".to_owned(),
                        passed: false,
                        detail: format!("`{} --version` did not answer within 15s", kind.binary()),
                    };
                }
                std::thread::sleep(Duration::from_millis(25));
            }
            Err(error) => {
                return PreflightCheck {
                    name: "agent_version".to_owned(),
                    passed: false,
                    detail: format!("could not poll `{} --version`: {error}", kind.binary()),
                };
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Controlled coding-task execution
// ---------------------------------------------------------------------------

/// How strongly the task workspace was isolated.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IsolationLevel {
    /// Disposable temp directory, scrubbed environment, timeout, canonical
    /// path containment. The baseline every task gets.
    WorkdirOnly,
    /// As above, plus the child ran under OS containment (Windows
    /// AppContainer/job object via the sandbox crate).
    ContainedProcess,
    /// Isolation could not be verified; the run failed closed.
    Unverified,
}

impl IsolationLevel {
    /// Whether this level permits reporting the task as automatically run.
    pub fn permits_automatic_run(self) -> bool {
        matches!(
            self,
            IsolationLevel::WorkdirOnly | IsolationLevel::ContainedProcess
        )
    }
}

/// Which agent should execute a task.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskAgent {
    /// A supported CLI coding agent.
    Cli(CodingAgentKind),
    /// No automatic execution: generate a guided package instead.
    Guided,
}

/// A controlled task for a coding agent.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct CodingTask {
    /// Stable task identifier.
    pub task_id: String,
    /// What the agent must accomplish, as plain instructions.
    pub instructions: String,
    /// Files placed in the disposable workspace, relative path → contents.
    pub files: BTreeMap<String, String>,
    /// Maximum wall-clock seconds for the agent invocation.
    pub timeout_seconds: u64,
    /// Maximum output bytes captured per stream.
    pub max_output_bytes: usize,
}

/// The outcome of a controlled task execution.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct TaskExecution {
    /// The task that ran.
    pub task_id: String,
    /// Which agent ran it.
    pub agent: String,
    /// How strongly the workspace was isolated.
    pub isolation: IsolationLevel,
    /// The agent's exit code, when it exited.
    pub exit_code: Option<i32>,
    /// Whether it exceeded its timeout.
    pub timed_out: bool,
    /// Captured stdout, bounded.
    pub stdout_preview: String,
    /// Captured stderr, bounded.
    pub stderr_preview: String,
    /// Total stdout bytes (which may exceed the preview).
    pub stdout_bytes: u64,
    /// Total stderr bytes.
    pub stderr_bytes: u64,
    /// Whether the workspace stayed inside its directory (canonical check).
    pub containment_held: bool,
    /// Files the agent left in the workspace, relative path → contents,
    /// bounded per file and in count. This is what the evaluator scores —
    /// never the agent's claims about what it did.
    pub artifacts: BTreeMap<String, String>,
}

/// Errors from controlled task execution.
#[derive(Clone, Debug, thiserror::Error, Eq, PartialEq)]
pub enum TaskExecutionError {
    /// The agent binary is missing or its auth is absent.
    #[error("the agent is not ready: {0}")]
    AgentNotReady(String),
    /// The workspace could not be prepared safely.
    #[error("the workspace could not be prepared: {0}")]
    Workspace(String),
    /// The agent could not be started.
    #[error("the agent could not be started: {0}")]
    Spawn(String),
    /// The timeout was zero.
    #[error("the task timeout must be greater than zero")]
    InvalidTimeout,
}

/// Prepares a disposable workspace for a task and verifies containment.
///
/// Creates a unique directory under the system temp directory, writes the
/// task files (refusing absolute paths, `..` segments, and symlinks that
/// escape), and canonicalizes the result. Anything that would place a file
/// outside the workspace fails closed with [`TaskExecutionError::Workspace`].
pub fn prepare_workspace(task: &CodingTask) -> Result<PathBuf, TaskExecutionError> {
    if task.timeout_seconds == 0 {
        return Err(TaskExecutionError::InvalidTimeout);
    }
    let root = std::env::temp_dir().join(format!(
        "probelab-task-{}-{}",
        sanitize_task_id(&task.task_id),
        std::process::id()
    ));
    std::fs::create_dir_all(&root).map_err(|error| {
        TaskExecutionError::Workspace(format!("could not create workspace: {error}"))
    })?;
    let canonical = std::fs::canonicalize(&root).map_err(|error| {
        TaskExecutionError::Workspace(format!("could not canonicalize workspace: {error}"))
    })?;
    for (relative, contents) in &task.files {
        let relative_path = Path::new(relative);
        if relative_path.is_absolute()
            || relative_path
                .components()
                .any(|component| component == std::path::Component::ParentDir)
        {
            let _ = std::fs::remove_dir_all(&canonical);
            return Err(TaskExecutionError::Workspace(format!(
                "task file {relative:?} escapes the workspace"
            )));
        }
        let destination = canonical.join(relative_path);
        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent).map_err(|error| {
                TaskExecutionError::Workspace(format!("could not create directory: {error}"))
            })?;
        }
        std::fs::write(&destination, contents).map_err(|error| {
            TaskExecutionError::Workspace(format!("could not write task file: {error}"))
        })?;
        // A symlink planted by a previous run (or a hostile task) that points
        // outside the workspace is refused rather than followed.
        let resolved = std::fs::canonicalize(&destination).map_err(|error| {
            TaskExecutionError::Workspace(format!("could not verify task file: {error}"))
        })?;
        if !resolved.starts_with(&canonical) {
            let _ = std::fs::remove_dir_all(&canonical);
            return Err(TaskExecutionError::Workspace(format!(
                "task file {relative:?} resolves outside the workspace"
            )));
        }
    }
    Ok(canonical)
}

fn sanitize_task_id(task_id: &str) -> String {
    task_id
        .chars()
        .filter(|character| character.is_ascii_alphanumeric() || *character == '-')
        .take(32)
        .collect()
}

/// Collects workspace files for scoring, bounded by count and per-file bytes.
/// Symlinks are refused: only regular files whose canonical path stays inside
/// the workspace are read.
fn collect_artifacts(
    workspace: &Path,
    max_files: usize,
    max_bytes_per_file: usize,
) -> BTreeMap<String, String> {
    fn walk(
        base: &Path,
        dir: &Path,
        out: &mut BTreeMap<String, String>,
        max_files: usize,
        max_bytes: usize,
        depth: usize,
    ) {
        if depth > 3 || out.len() >= max_files {
            return;
        }
        let entries = match std::fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(_) => return,
        };
        for entry in entries.flatten() {
            if out.len() >= max_files {
                return;
            }
            let path = entry.path();
            let file_type = match entry.file_type() {
                Ok(kind) => kind,
                Err(_) => continue,
            };
            if file_type.is_symlink() {
                continue;
            }
            if file_type.is_dir() {
                walk(base, &path, out, max_files, max_bytes, depth + 1);
            } else if file_type.is_file() {
                let resolved = match std::fs::canonicalize(&path) {
                    Ok(resolved) => resolved,
                    Err(_) => continue,
                };
                if !resolved.starts_with(base) {
                    continue;
                }
                let relative = match path.strip_prefix(base) {
                    Ok(relative) => relative.to_string_lossy().into_owned(),
                    Err(_) => continue,
                };
                let bytes = std::fs::read(&path).unwrap_or_default();
                let text = String::from_utf8_lossy(&bytes);
                out.insert(relative, text.chars().take(max_bytes).collect());
            }
        }
    }

    let mut out = BTreeMap::new();
    let Ok(canonical) = std::fs::canonicalize(workspace) else {
        return out;
    };
    walk(
        &canonical,
        &canonical,
        &mut out,
        max_files,
        max_bytes_per_file,
        0,
    );
    out
}

/// Builds the non-interactive argv for a coding agent.
///
/// The task instructions travel as one argument; the workspace travels as the
/// working directory. No shell is involved, so there is no quoting layer to
/// escape through.
pub fn agent_argv(kind: CodingAgentKind, task: &CodingTask) -> Vec<String> {
    match kind {
        CodingAgentKind::Codex => vec![
            "exec".to_owned(),
            "--skip-git-repo-check".to_owned(),
            task.instructions.clone(),
        ],
        CodingAgentKind::OpenCode => vec!["run".to_owned(), task.instructions.clone()],
        CodingAgentKind::ClaudeCode => {
            vec!["-p".to_owned(), task.instructions.clone()]
        }
    }
}

/// Executes a coding task in a disposable workspace with a scrubbed
/// environment and a wall-clock timeout.
///
/// The child inherits nothing: provider credential variables
/// (`*_API_KEY`, `*_TOKEN`, `*_SECRET`) are removed, not passed through, so
/// an evaluated agent can never exfiltrate a key it was never given. Standard
/// input is closed; stdout/stderr are captured to bounded previews.
///
/// This runs the agent CLI as the invoking user inside the workspace
/// directory ([`IsolationLevel::WorkdirOnly`]). It is not OS containment: a
/// coding agent is an arbitrary local program, and a temp directory does not
/// make one safe. Callers that need stronger isolation must route through the
/// sandbox crate's contained spawner (Windows) or refuse automatic execution
/// and emit a guided package instead.
pub fn execute_coding_task(
    kind: CodingAgentKind,
    task: &CodingTask,
) -> Result<TaskExecution, TaskExecutionError> {
    let binary = find_on_path(kind.binary()).ok_or_else(|| {
        TaskExecutionError::AgentNotReady(format!("no `{}` on PATH", kind.binary()))
    })?;
    if !kind.auth_config_present() {
        return Err(TaskExecutionError::AgentNotReady(format!(
            "no user configuration for {}; complete the agent's own setup first",
            kind.label()
        )));
    }
    let workspace = prepare_workspace(task)?;
    let argv = agent_argv(kind, task);
    let (program, args) = argv
        .split_first()
        .expect("agent_argv always yields at least one element");

    let mut command = std::process::Command::new(&binary);
    command
        .arg(program)
        .args(args)
        .current_dir(&workspace)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .env_clear();
    // A minimal, non-credential environment. `PATH` is required to resolve
    // the agent's own subprocesses; `SystemRoot`/`TEMP` are required on
    // Windows for the child to start at all. Everything else is withheld.
    if let Some(path) = std::env::var_os("PATH") {
        command.env("PATH", path);
    }
    #[cfg(windows)]
    {
        for name in ["SystemRoot", "TEMP", "TMP", "LOCALAPPDATA"] {
            if let Some(value) = std::env::var_os(name) {
                command.env(name, value);
            }
        }
    }
    #[cfg(not(windows))]
    {
        for name in ["HOME", "LANG", "LC_ALL", "TMPDIR"] {
            if let Some(value) = std::env::var_os(name) {
                command.env(name, value);
            }
        }
    }

    let mut child = command
        .spawn()
        .map_err(|error| TaskExecutionError::Spawn(error.to_string()))?;
    let timeout = Duration::from_secs(task.timeout_seconds);
    let waited = child
        .wait_with_output_timeout(timeout)
        .map_err(|error| TaskExecutionError::Spawn(error.to_string()))?;
    let (exit_code, timed_out, stdout_bytes_raw, stderr_bytes_raw) = match waited {
        Some(output) => (output.status.code(), false, output.stdout, output.stderr),
        None => {
            let _ = child.kill();
            let _ = child.wait();
            (None, true, Vec::new(), Vec::new())
        }
    };

    // Re-verify containment after the run: the workspace must still resolve
    // inside itself (an agent that replaced it with a symlink fails here).
    let containment_held = std::fs::canonicalize(&workspace)
        .map(|resolved| resolved.starts_with(&workspace))
        .unwrap_or(false);

    // Bounded previews; the full streams are not persisted, because an agent
    // that prints a secret it found on disk must not land that secret in a
    // result bundle. Lossy previews plus byte counts preserve the evidence
    // that output existed without preserving all of it.
    let stdout_bytes = stdout_bytes_raw.len() as u64;
    let stderr_bytes = stderr_bytes_raw.len() as u64;
    let preview_cap = task.max_output_bytes.max(1);
    let stdout_preview =
        String::from_utf8_lossy(&stdout_bytes_raw[..stdout_bytes_raw.len().min(preview_cap)])
            .into_owned();
    let stderr_preview =
        String::from_utf8_lossy(&stderr_bytes_raw[..stderr_bytes_raw.len().min(preview_cap)])
            .into_owned();

    // Collect what the agent left behind for the evaluator, bounded, and
    // only when containment held: artifacts from a workspace that escaped
    // are not evidence of anything except the escape.
    let artifacts = if containment_held {
        collect_artifacts(&workspace, 32, 64 * 1024)
    } else {
        BTreeMap::new()
    };

    // The workspace is disposable by design: remove it after the run so task
    // files cannot accumulate on the host.
    let _ = std::fs::remove_dir_all(&workspace);
    Ok(TaskExecution {
        task_id: task.task_id.clone(),
        agent: kind.executor_id().to_owned(),
        isolation: if containment_held {
            IsolationLevel::WorkdirOnly
        } else {
            IsolationLevel::Unverified
        },
        exit_code,
        timed_out,
        stdout_preview,
        stderr_preview,
        stdout_bytes,
        stderr_bytes,
        containment_held,
        artifacts,
    })
}

/// A scoped extension trait for waiting with a timeout without extra crates.
trait WaitWithOutputTimeout {
    /// Waits up to `timeout` for output; `Ok(None)` means the timeout expired
    /// and the child is still running.
    fn wait_with_output_timeout(
        &mut self,
        timeout: Duration,
    ) -> std::io::Result<Option<std::process::Output>>;
}

impl WaitWithOutputTimeout for std::process::Child {
    fn wait_with_output_timeout(
        &mut self,
        timeout: Duration,
    ) -> std::io::Result<Option<std::process::Output>> {
        use std::io::Read;
        let start = std::time::Instant::now();
        loop {
            match self.try_wait()? {
                Some(status) => {
                    let mut stdout = Vec::new();
                    let mut stderr = Vec::new();
                    if let Some(mut pipe) = self.stdout.take() {
                        let _ = pipe.read_to_end(&mut stdout);
                    }
                    if let Some(mut pipe) = self.stderr.take() {
                        let _ = pipe.read_to_end(&mut stderr);
                    }
                    return Ok(Some(std::process::Output {
                        status,
                        stdout,
                        stderr,
                    }));
                }
                None => {
                    if start.elapsed() >= timeout {
                        return Ok(None);
                    }
                    std::thread::sleep(Duration::from_millis(25));
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Guided external packages
// ---------------------------------------------------------------------------

/// An isolated benchmark package for external execution.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct GuidedPackage {
    /// Stable package format version.
    pub format_version: u32,
    /// The benchmark this package measures.
    pub benchmark_id: String,
    /// The benchmark version pinned at generation time.
    pub benchmark_version: String,
    /// The seed this package was generated from.
    pub seed: u64,
    /// Human instructions for running the tasks elsewhere.
    pub instructions: String,
    /// Task files, relative path → contents.
    pub files: BTreeMap<String, String>,
    /// What a submission must contain to be verifiable.
    pub submission_requirements: Vec<String>,
}

/// Evidence imported from an external run.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct GuidedEvidence {
    /// The package this evidence answers.
    pub task_id: String,
    /// Submitted artifacts, relative path → contents.
    pub artifacts: BTreeMap<String, String>,
    /// Submitted log text, bounded by the importer.
    pub log: String,
}

/// How imported evidence was produced. Always recorded, always shown:
///
/// a fully observed run and a manually submitted bundle are different kinds
/// of evidence, and presenting one as the other would be fabrication.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceProvenance {
    /// The lab observed every action through a supported adapter.
    ObservedRun,
    /// A human ran the package elsewhere and submitted the outputs.
    ManualSubmission,
}

/// Generates an isolated task package for external execution.
///
/// The package contains only the task inputs — never hidden answers, scoring
/// logic, or credentials. Verification stays in the lab.
pub fn generate_guided_package(
    benchmark_id: &str,
    benchmark_version: &str,
    seed: u64,
    tasks: BTreeMap<String, String>,
) -> GuidedPackage {
    GuidedPackage {
        format_version: 1,
        benchmark_id: benchmark_id.to_owned(),
        benchmark_version: benchmark_version.to_owned(),
        seed,
        instructions: format!(
            "ProbeLab guided package for {benchmark_id} v{benchmark_version} (seed {seed}).\n\
             \n\
             1. Copy the task files to an isolated machine.\n\
             2. Complete each task with the model or application under test.\n\
             3. Submit the resulting artifacts plus the execution log.\n\
             4. ProbeLab verifies the submission independently; a submission is\n\
             manual-submission evidence, never an observed run."
        ),
        files: tasks,
        submission_requirements: vec![
            "artifacts for every task file".to_owned(),
            "the execution log".to_owned(),
            "the model or application name and version".to_owned(),
        ],
    }
}

/// Validates imported evidence before verification: bounded sizes, no
/// absolute paths, no `..` escapes. Verification itself belongs to the
/// benchmark's evaluator, not to the importer.
pub fn import_guided_evidence(
    package: &GuidedPackage,
    artifacts: BTreeMap<String, String>,
    log: &str,
) -> Result<(GuidedEvidence, EvidenceProvenance), String> {
    const MAX_ARTIFACTS: usize = 64;
    const MAX_ARTIFACT_BYTES: usize = 256 * 1024;
    const MAX_LOG_CHARS: usize = 32_768;
    if artifacts.len() > MAX_ARTIFACTS {
        return Err(format!(
            "too many artifacts: {} (max {MAX_ARTIFACTS})",
            artifacts.len()
        ));
    }
    for (relative, contents) in &artifacts {
        let path = Path::new(relative);
        if path.is_absolute()
            || path
                .components()
                .any(|component| component == std::path::Component::ParentDir)
        {
            return Err(format!("artifact {relative:?} escapes the package"));
        }
        if contents.len() > MAX_ARTIFACT_BYTES {
            return Err(format!("artifact {relative:?} exceeds the size bound"));
        }
    }
    Ok((
        GuidedEvidence {
            task_id: format!(
                "{}-{}-{}",
                package.benchmark_id, package.benchmark_version, package.seed
            ),
            artifacts,
            log: log.chars().take(MAX_LOG_CHARS).collect(),
        },
        EvidenceProvenance::ManualSubmission,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovery_reports_six_or_more_executors() {
        let executors = discover_executors();
        // 2 offline + 3 coding agents + 1 guided + 3 remote = 9.
        assert_eq!(executors.len(), 9);
        for executor in &executors {
            assert!(!executor.id.is_empty());
            assert!(!executor.label.is_empty());
        }
    }

    #[test]
    fn every_executor_names_a_stable_mode() {
        for executor in discover_executors() {
            let round_trip = serde_json::to_string(&executor.mode).expect("mode serializes");
            let back: ExecutionMode = serde_json::from_str(&round_trip).expect("mode round-trips");
            assert_eq!(back, executor.mode);
            assert!(!executor.mode.as_str().is_empty());
        }
    }

    #[test]
    fn guided_mode_is_always_available() {
        let guided = guided_executor();
        assert!(guided.availability.is_available());
        assert_eq!(guided.mode, ExecutionMode::GuidedExternal);
    }

    #[test]
    fn preflight_of_an_unknown_executor_fails_closed() {
        let report = preflight_executor("no-such-executor");
        assert!(!report.passed);
    }

    #[test]
    fn preflight_of_guided_mode_passes_without_touching_the_machine() {
        let report = preflight_executor("guided-external");
        assert!(report.passed);
    }

    #[test]
    fn a_workspace_with_an_escaping_file_is_refused() {
        let mut files = BTreeMap::new();
        files.insert("../escape.txt".to_owned(), "nope".to_owned());
        let task = CodingTask {
            task_id: "evil".to_owned(),
            instructions: "do nothing".to_owned(),
            files,
            timeout_seconds: 10,
            max_output_bytes: 4096,
        };
        assert!(matches!(
            prepare_workspace(&task),
            Err(TaskExecutionError::Workspace(_))
        ));
    }

    #[test]
    fn a_zero_timeout_is_refused_before_any_directory_exists() {
        let task = CodingTask {
            task_id: "t".to_owned(),
            instructions: "do nothing".to_owned(),
            files: BTreeMap::new(),
            timeout_seconds: 0,
            max_output_bytes: 4096,
        };
        assert_eq!(
            prepare_workspace(&task),
            Err(TaskExecutionError::InvalidTimeout)
        );
    }

    #[test]
    fn agent_argv_never_invokes_a_shell() {
        let task = CodingTask {
            task_id: "t".to_owned(),
            instructions: "fix; rm -rf /".to_owned(),
            files: BTreeMap::new(),
            timeout_seconds: 10,
            max_output_bytes: 4096,
        };
        // The hostile string travels as one argument, never through a shell:
        // the argv shape is fixed and the instructions are a single element.
        for kind in CodingAgentKind::all() {
            let argv = agent_argv(kind, &task);
            assert!(argv.iter().any(|arg| arg == "fix; rm -rf /"));
        }
    }

    #[test]
    fn importing_oversized_evidence_is_refused() {
        let package = generate_guided_package("demo", "1", 7, BTreeMap::new());
        let mut artifacts = BTreeMap::new();
        for index in 0..70 {
            artifacts.insert(format!("file{index}.txt"), "x".to_owned());
        }
        assert!(import_guided_evidence(&package, artifacts, "log").is_err());
    }

    #[test]
    fn imported_evidence_is_always_marked_manual() {
        let package = generate_guided_package("demo", "1", 7, BTreeMap::new());
        let (_, provenance) =
            import_guided_evidence(&package, BTreeMap::new(), "log").expect("import");
        assert_eq!(provenance, EvidenceProvenance::ManualSubmission);
    }

    #[test]
    fn isolation_levels_report_honestly() {
        assert!(IsolationLevel::WorkdirOnly.permits_automatic_run());
        assert!(IsolationLevel::ContainedProcess.permits_automatic_run());
        assert!(!IsolationLevel::Unverified.permits_automatic_run());
    }

    /// Serves one HTTP response on loopback, then stops. The stand-in for a
    /// local runtime (compatible) or for whatever else answers a port (404).
    /// Accepts two connections: several probes each open their own, and a
    /// stand-in that served only the first would make the second look absent.
    fn serve_once(response: &'static str) -> u16 {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("loopback listener");
        let port = listener.local_addr().expect("port").port();
        std::thread::spawn(move || {
            for _ in 0..2 {
                let Ok((mut stream, _)) = listener.accept() else {
                    break;
                };
                let mut request = [0u8; 1024];
                let _ = stream.read(&mut request);
                let _ = stream.write_all(response.as_bytes());
            }
        });
        port
    }

    #[test]
    fn a_compatible_server_is_recognized_with_its_model_count() {
        let port = serve_once(
            "HTTP/1.0 200 OK\r\nContent-Type: application/json\r\n\r\n{\"data\": [{\"id\": \"a\"}, {\"id\": \"b\"}]}",
        );
        assert_eq!(
            openai_compatible_models(port, Duration::from_secs(2)),
            Some(2)
        );
        assert_eq!(
            local_model_ids(port, Duration::from_secs(2)),
            vec!["a".to_owned(), "b".to_owned()]
        );
    }

    #[test]
    fn model_listing_is_empty_rather_than_wrong_without_a_server() {
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .expect("listener")
            .local_addr()
            .expect("port")
            .port();
        assert!(local_model_ids(port, Duration::from_millis(300)).is_empty());
    }

    #[test]
    fn a_port_answering_the_wrong_protocol_is_not_a_model_runtime() {
        // The measured 8080 case: TCP answers, the protocol does not.
        let port = serve_once("HTTP/1.0 404 Not Found\r\n\r\nnothing here");
        assert_eq!(openai_compatible_models(port, Duration::from_secs(2)), None);
    }

    #[test]
    fn a_garbage_response_is_not_a_model_runtime() {
        let port = serve_once("HTTP/1.0 200 OK\r\n\r\nthis is not json");
        assert_eq!(openai_compatible_models(port, Duration::from_secs(2)), None);
    }

    #[test]
    fn a_closed_port_is_not_a_model_runtime() {
        // An ephemeral port nothing listens on. Binding then dropping the
        // listener frees a port the probe must report as absent.
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .expect("listener")
            .local_addr()
            .expect("port")
            .port();
        assert_eq!(
            openai_compatible_models(port, Duration::from_millis(300)),
            None
        );
    }
}
