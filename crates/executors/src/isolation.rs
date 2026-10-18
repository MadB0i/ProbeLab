//! Read-only inspection of disposable guest-execution support.
//!
//! # Why inspection, not enablement
//!
//! Automatic execution of an installed coding agent is disabled at the
//! product boundary because a disposable workdir is not an isolation
//! boundary. The way back is a genuinely disposable guest: a sandbox or VM
//! the host orchestrates, the agent runs inside, and only scored evidence
//! crosses back. This module answers the prior question — *does this machine
//! even have the parts?* — without changing anything: no features enabled,
//! no configuration written, no guest started.
//!
//! Every check is read-only and bounded. Anything unanswerable reports
//! [`FeatureState::Unknown`] rather than guessing, and the verdict is always
//! [`GuestVerdict::GuidedOnly`]: enabling automatic execution additionally
//! requires testing the containment itself, which has not happened.

/// `Duration` bounds the DISM and WMI queries below, and every one of those queries is Windows-only.
/// The import is gated to match, so the non-Windows branch does not carry a type it cannot use.
#[cfg(windows)]
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Whether a Windows optional feature is turned on.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FeatureState {
    /// Reported enabled by the OS.
    Enabled,
    /// Reported disabled or absent by the OS.
    Disabled,
    /// Could not be determined (query failed, timed out, or non-Windows).
    Unknown,
}

/// What the inspection concludes for automatic agent execution.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GuestVerdict {
    /// Automatic execution stays off; guided external mode is the path.
    ///
    /// This is currently the only verdict: when the sandbox is unavailable
    /// there is nowhere to run, and when it is available its containment is
    /// still untested. Both branches end here, for different stated reasons.
    GuidedOnly,
}

/// Disposable guest-execution support, as inspected right now.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct GuestIsolation {
    /// Best-effort OS description (edition and build where queryable).
    pub os: String,
    /// Whether a hypervisor reports itself present (`None` when unqueryable).
    pub hypervisor_present: Option<bool>,
    /// Whether `WindowsSandbox.exe` exists on this machine.
    pub sandbox_binary_present: bool,
    /// The `Containers-DisposableClientVM` optional-feature state.
    pub sandbox_feature: FeatureState,
    /// The `Microsoft-Hyper-V` platform feature state.
    pub hyperv_feature: FeatureState,
    /// The standing product rule (see the module note).
    pub verdict: GuestVerdict,
    /// Human explanation, including what would change the answer.
    pub detail: String,
}

/// Inspects disposable guest-execution support without changing anything.
///
/// Bounded (each query has its own timeout) and side-effect free: the
/// heaviest operation is a read-only DISM/WMI query. Never enables a
/// feature, never starts a guest, never writes configuration.
pub fn inspect_guest_isolation() -> GuestIsolation {
    #[cfg(not(windows))]
    {
        GuestIsolation {
            os: std::env::consts::OS.to_owned(),
            hypervisor_present: None,
            sandbox_binary_present: false,
            sandbox_feature: FeatureState::Unknown,
            hyperv_feature: FeatureState::Unknown,
            verdict: GuestVerdict::GuidedOnly,
            detail: "guest inspection is a Windows assessment; this host is not Windows, so guided external mode is the path.".to_owned(),
        }
    }
    #[cfg(windows)]
    {
        inspect_windows_guest()
    }
}

#[cfg(windows)]
fn inspect_windows_guest() -> GuestIsolation {
    let os = query_os_description();
    let hypervisor_present = query_hypervisor_present();
    let sandbox_binary_present =
        std::path::Path::new(r"C:\Windows\System32\WindowsSandbox.exe").is_file();
    let sandbox_feature = query_optional_feature("Containers-DisposableClientVM");
    let hyperv_feature = query_optional_feature("Microsoft-Hyper-V-All");
    let detail = explain(
        &os,
        hypervisor_present,
        sandbox_binary_present,
        sandbox_feature,
        hyperv_feature,
    );
    GuestIsolation {
        os,
        hypervisor_present,
        sandbox_binary_present,
        sandbox_feature,
        hyperv_feature,
        verdict: GuestVerdict::GuidedOnly,
        detail,
    }
}

/// The standing rule, as a pure function so tests pin it without a host.
pub fn decide_verdict(_sandbox_usable: bool) -> GuestVerdict {
    // Both branches end here: unavailable means nowhere to run, available
    // means containment still untested. Enabling automatic execution needs a
    // tested boundary, not just a present feature.
    GuestVerdict::GuidedOnly
}

#[cfg(windows)]
fn explain(
    os: &str,
    hypervisor_present: Option<bool>,
    sandbox_binary_present: bool,
    sandbox_feature: FeatureState,
    hyperv_feature: FeatureState,
) -> String {
    let hypervisor = match hypervisor_present {
        Some(true) => "a hypervisor reports itself present",
        Some(false) => "no hypervisor reports itself present",
        None => "hypervisor presence could not be determined",
    };
    let sandbox = match (sandbox_binary_present, sandbox_feature) {
        (true, FeatureState::Enabled) => {
            "Windows Sandbox is installed and its feature is enabled, but its containment of agent workloads is untested"
        }
        (true, FeatureState::Disabled) => {
            "the Sandbox binary exists but its optional feature is disabled"
        }
        (true, FeatureState::Unknown) => {
            "the Sandbox binary exists but the feature state could not be read"
        }
        (false, _) => "Windows Sandbox is not installed",
    };
    let hyperv = match hyperv_feature {
        FeatureState::Enabled => "the Hyper-V platform feature is enabled",
        FeatureState::Disabled => "the Hyper-V platform feature is disabled",
        FeatureState::Unknown => "the Hyper-V platform feature state could not be read",
    };
    format!(
        "{os}: {hypervisor}; {sandbox}; {hyperv}. Automatic agent execution stays disabled: guided external mode — isolated packages, independent scoring, manual-submission provenance — is the supported path until a disposable guest boundary is tested, not merely present."
    )
}

#[cfg(windows)]
fn query_os_description() -> String {
    let caption = run_powershell(
        "(Get-CimInstance Win32_OperatingSystem).Caption",
        Duration::from_secs(20),
    )
    .unwrap_or_default();
    let build = run_powershell(
        "(Get-CimInstance Win32_OperatingSystem).BuildNumber",
        Duration::from_secs(20),
    )
    .unwrap_or_default();
    let caption = caption.trim();
    let build = build.trim();
    if caption.is_empty() && build.is_empty() {
        return "Windows (edition unreadable)".to_owned();
    }
    format!("{caption} build {build}")
        .trim()
        .trim_start_matches("Microsoft ")
        .to_owned()
}

#[cfg(windows)]
fn query_hypervisor_present() -> Option<bool> {
    let output = run_powershell(
        "(Get-CimInstance Win32_ComputerSystem).HypervisorPresent",
        Duration::from_secs(20),
    )?;
    match output.trim().to_ascii_lowercase().as_str() {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

#[cfg(windows)]
fn query_optional_feature(name: &str) -> FeatureState {
    let script = format!("(Get-WindowsOptionalFeature -Online -FeatureName {name}).State");
    let output = match run_powershell(&script, Duration::from_secs(45)) {
        Some(output) => output,
        None => return FeatureState::Unknown,
    };
    match output.trim().to_ascii_lowercase().as_str() {
        "enabled" => FeatureState::Enabled,
        "disabled" | "disablepending" | "enablepending" => FeatureState::Disabled,
        _ => FeatureState::Unknown,
    }
}

/// Runs a read-only PowerShell query with a hard timeout.
///
/// `-NoProfile -NonInteractive`, stdout piped, stderr discarded. Anything
/// that fails, hangs, or prints nothing is `None`: inspection degrades to
/// `Unknown`, never to a guess.
#[cfg(windows)]
fn run_powershell(script: &str, timeout: Duration) -> Option<String> {
    let mut child = std::process::Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    let start = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => {
                use std::io::Read;
                let mut output = String::new();
                if let Some(mut pipe) = child.stdout.take() {
                    let _ = pipe.read_to_string(&mut output);
                }
                // Bound the answer: a version table is information, a novel is not.
                return Some(output.chars().take(512).collect());
            }
            Ok(Some(_)) => {
                let _ = child.wait();
                return None;
            }
            Ok(None) => {
                if start.elapsed() >= timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(_) => return None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_verdict_is_guided_only_whether_or_not_a_sandbox_exists() {
        // The security gate, pinned: presence of a feature is not a tested
        // boundary, so both branches refuse automatic execution.
        assert_eq!(decide_verdict(false), GuestVerdict::GuidedOnly);
        assert_eq!(decide_verdict(true), GuestVerdict::GuidedOnly);
    }

    #[test]
    fn inspection_reports_a_verdict_and_an_explanation() {
        let report = inspect_guest_isolation();
        assert_eq!(report.verdict, GuestVerdict::GuidedOnly);
        assert!(!report.detail.is_empty());
        assert!(!report.os.is_empty());
    }

    #[test]
    fn the_report_round_trips_for_the_ipc_boundary() {
        let report = inspect_guest_isolation();
        let json = serde_json::to_string(&report).expect("serializes");
        let back: GuestIsolation = serde_json::from_str(&json).expect("round-trips");
        assert_eq!(back, report);
    }
}
