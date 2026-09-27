//! Adversarial self-tests.
//!
//! # What this file is for
//!
//! ProbeLab's central claim is that its boundaries hold: the broker is the only authorization authority,
//! the sandbox is the only way a process starts, the evaluator is independent, and a blocked probe is a
//! *successful test* rather than a vulnerability. This file attacks those claims from the outside.
//!
//! The existing suites already cover the broker's path handling, the arena's rules, the provider's
//! endpoint construction, and the containment identity. What was missing is the adversarial *transport*
//! and *replay corruption* surface, and -- more importantly -- a single place that states the distinction
//! the whole security story rests on.
//!
//! # The distinction, stated once
//!
//! **A probe that was blocked is a successful test.** It proves the boundary held.
//! **A model that violated the boundary is a security finding.** It proves the boundary did not.
//!
//! Nothing here may convert the first into the second. Every blocked-probe assertion is written so that it
//! would *fail* if the probe had succeeded, which is the only way to know a refusal was real rather than a
//! test that passed because nothing happened either way.
//!
//! # Why the transport tests bind real sockets
//!
//! A mocked HTTP client proves the code calls the method you think it calls. It cannot prove what happens
//! when a server sends a 307 to another host, a truncated body, a four-megabyte response, or nothing at
//! all. Those are exactly the cases worth testing, so these tests bind a loopback listener and let the
//! transport misbehave for real.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use probelab_core::{
    Action, CapabilityDenial, CapabilityPermission, CapabilityRequest, GameMove, ModelDecision,
    ModelRequest, Observation, ProcessLifecycle, RequestedCapability, TelemetryEvent,
};

// # Section 1 -- provider transport
//
// The model's output arrives over a network from a host the user configured. Everything here is a way that
// channel could misbehave, and each must produce a typed failure rather than a panic, a hang, a silent
// success, or a credential sent somewhere unconfigured.

/// A loopback server that answers every connection with one canned response.
struct Canned {
    base_url: String,
}

impl Canned {
    /// Binds an ephemeral loopback port and serves `response` to every connection.
    ///
    /// The response is leaked so it can be `'static`, which is what lets each test name its own literal
    /// rather than sharing one. These are test processes; the leak is bounded by the test binary.
    fn serving(response: &'static [u8]) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let base_url = format!("http://{}/v1", listener.local_addr().expect("an address"));
        std::thread::spawn(move || {
            for stream in listener.incoming().take(64) {
                let Ok(mut stream) = stream else { break };
                // Read whatever arrived so the client's write does not fail, then answer.
                let mut buffer = [0u8; 4096];
                let _ = stream.read(&mut buffer);
                let _ = stream.write_all(response);
                let _ = stream.flush();
                let _ = stream.shutdown(std::net::Shutdown::Both);
            }
        });
        Self { base_url }
    }

    /// Serves a response built at run time.
    fn serving_owned(response: Vec<u8>) -> Self {
        Self::serving(Box::leak(response.into_boxed_slice()))
    }
}

/// A server that accepts connections and never answers, for the timeout case.
struct Blackhole {
    base_url: String,
    /// The listener is kept alive here for the test's lifetime. Dropping it would stop the accept loop,
    /// which would make the client fail fast on a *connection* error rather than on its own timeout --
    /// a different failure, and one that would not prove the timeout works.
    _listener: TcpListener,
}

impl Blackhole {
    fn binding() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let base_url = format!("http://{}/v1", listener.local_addr().expect("an address"));
        let accepting = listener.try_clone().expect("clone the listener");
        std::thread::spawn(move || {
            // Accept and hold each connection without responding, so the only thing that can end a
            // request is the client's own timeout. The sockets are held by the OS backlog, which is why
            // reading them and doing nothing is enough to keep the connection open.
            for stream in accepting.incoming().take(256) {
                if stream.is_err() {
                    break;
                }
            }
        });
        Self {
            base_url,
            _listener: listener,
        }
    }
}

/// An unexpected redirect is refused, and its target is never contacted.
///
/// A 307 to another host is either a misconfiguration or an attempt to move a credential somewhere the
/// user never named. Following it would send the API key to an unconfigured host, so the transport
/// refuses redirects outright. This asserts the refusal is *observable*: the redirect target must never
/// receive a request. A test that only checked "an error came back" would pass even if the key had
/// already been forwarded.
#[test]
fn an_unexpected_redirect_is_refused_and_its_target_is_never_contacted() {
    let target_hits = Arc::new(AtomicUsize::new(0));
    let counter = target_hits.clone();
    let target = TcpListener::bind("127.0.0.1:0").expect("bind the redirect target");
    let target_address = target.local_addr().expect("an address");
    std::thread::spawn(move || {
        for stream in target.incoming().take(64) {
            let Ok(mut stream) = stream else { break };
            counter.fetch_add(1, Ordering::Relaxed);
            let mut buffer = [0u8; 2048];
            let _ = stream.read(&mut buffer);
            let body = br#"{"choices":[{"message":{"content":"{\"action\":\"MOVE_UP\"}"}}]}"#;
            let _ = stream.write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
                    body.len()
                )
                .as_bytes(),
            );
            let _ = stream.write_all(body);
            let _ = stream.flush();
        }
    });

    let redirector = TcpListener::bind("127.0.0.1:0").expect("bind the redirector");
    let redirector_address = redirector.local_addr().expect("an address");
    std::thread::spawn(move || {
        for stream in redirector.incoming().take(64) {
            let Ok(mut stream) = stream else { break };
            let mut buffer = [0u8; 2048];
            let _ = stream.read(&mut buffer);
            let _ = stream.write_all(
                format!(
                    "HTTP/1.1 307 Temporary Redirect\r\nLocation: http://{target_address}/v1\r\nContent-Length: 0\r\n\r\n"
                )
                .as_bytes(),
            );
            let _ = stream.flush();
            let _ = stream.shutdown(std::net::Shutdown::Both);
        }
    });

    let outcome = call_openai_compatible(&format!("http://{redirector_address}/v1"), 5);
    assert!(
        outcome.is_err(),
        "a redirect must be a typed failure, not a followed request"
    );
    assert_eq!(
        target_hits.load(Ordering::Relaxed),
        0,
        "the redirect target was contacted; the transport followed a redirect it must not follow"
    );
}

/// A server that never answers produces a timeout, not a hang.
///
/// The distinction matters for a desktop product: a run that never returns looks like a frozen
/// application, and a user who cannot tell a hang from a slow run waits forever.
#[test]
fn a_server_that_never_answers_times_out_rather_than_hanging() {
    let blackhole = Blackhole::binding();
    let started = Instant::now();
    let outcome = call_openai_compatible(&blackhole.base_url, 1);
    let elapsed = started.elapsed();
    assert!(outcome.is_err(), "a silent server must produce a failure");
    assert!(
        elapsed < Duration::from_secs(30),
        "the call took {elapsed:?}, so it waited for something other than its own timeout"
    );
}

/// A truncated body is a failure, not a partial success.
///
/// Half a JSON document is not half an answer. Accepting it would mean acting on a truncated action, which
/// is how a model ends up "deciding" something the provider never said.
#[test]
fn a_truncated_response_body_is_a_typed_failure() {
    let server = Canned::serving(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 400\r\n\r\n{\"choices\":[{\"message\":{\"cont");
    let outcome = call_openai_compatible(&server.base_url, 5);
    assert!(
        outcome.is_err(),
        "a body shorter than its declared Content-Length must fail"
    );
}

/// A body that is not a well-formed completion is a failure.
#[test]
fn a_malformed_response_body_is_a_typed_failure() {
    let bodies: Vec<Vec<u8>> = vec![
        b"HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: 5\r\n\r\n<html>".to_vec(),
        b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 4\r\n\r\nnull"
            .to_vec(),
        b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\n[]"
            .to_vec(),
        b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 7\r\n\r\n{\"a\":1}"
            .to_vec(),
        b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 0\r\n\r\n".to_vec(),
    ];
    for body in bodies {
        let server = Canned::serving_owned(body.clone());
        let outcome = call_openai_compatible(&server.base_url, 5);
        assert!(
            outcome.is_err(),
            "a body that is not a well-formed completion must fail: {}",
            String::from_utf8_lossy(&body)
        );
    }
}

/// A `content` field that is not a decision is refused at the parse layer.
///
/// This is the boundary a hostile provider actually attacks: the transport is fine, the HTTP is fine, and
/// the *protocol* is wrong. Refusing must happen in the parser rather than being handed onward as a string.
#[test]
fn a_content_field_that_is_not_a_decision_is_refused() {
    for content in [
        "not json at all",
        r#"{"action":"DIAGONAL"}"#,
        r#"{"action":42}"#,
        r#"{"action":"MOVE_UP","extra":"field"}"#,
        r#"{"capability":"filesystem.read","path":"C:/Windows"}"#,
        "{}",
        "[]",
    ] {
        let body = format!(
            r#"{{"choices":[{{"message":{{"content":{}}}}}]}}"#,
            serde_json::to_string(content).expect("encode a JSON string")
        );
        let http = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body
        );
        let server = Canned::serving_owned(http.into_bytes());
        let outcome = call_openai_compatible(&server.base_url, 5);
        assert!(
            outcome.is_err(),
            "content {content:?} is not a decision and must be refused"
        );
    }
}

/// An oversized response is refused rather than buffered without limit.
///
/// A provider answering with megabytes is either broken or hostile, and a client that reads all of it is a
/// memory-exhaustion vector pointed at the lab. This asserts the refusal rather than trusting a constant.
#[test]
fn an_oversized_response_is_refused_rather_than_buffered() {
    let filler = "A".repeat(4 * 1024 * 1024);
    let body = format!(r#"{{"choices":[{{"message":{{"content":"{filler}"}}}}]}}"#);
    let http = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
        body.len(),
        body
    );
    let server = Canned::serving_owned(http.into_bytes());
    let started = Instant::now();
    let outcome = call_openai_compatible(&server.base_url, 15);
    assert!(
        outcome.is_err(),
        "a multi-megabyte response must be refused rather than parsed"
    );
    assert!(
        started.elapsed() < Duration::from_secs(30),
        "the oversized response was read to completion, so the bound did not apply"
    );
}

/// An HTTP error status is a typed failure, for every status class a provider can return.
#[test]
fn an_http_error_status_is_a_typed_failure() {
    for status in [
        "400 Bad Request",
        "401 Unauthorized",
        "403 Forbidden",
        "429 Too Many Requests",
        "500 Internal Server Error",
        "502 Bad Gateway",
        "503 Service Unavailable",
    ] {
        let http = format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\n{{}}"
        );
        let server = Canned::serving_owned(http.into_bytes());
        let outcome = call_openai_compatible(&server.base_url, 5);
        assert!(outcome.is_err(), "HTTP {status} must be a failure");
    }
}

/// A connection closed without a response is a failure, not an empty success.
#[test]
fn a_connection_closed_without_a_response_is_a_typed_failure() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let address = listener.local_addr().expect("an address");
    std::thread::spawn(move || {
        for stream in listener.incoming().take(64) {
            // Accept and immediately drop, without reading or writing.
            drop(stream);
        }
    });
    let outcome = call_openai_compatible(&format!("http://{address}/v1"), 5);
    assert!(
        outcome.is_err(),
        "a closed connection must be a failure rather than an empty success"
    );
}

/// No transport failure ever leaks the credential into its message.
///
/// A failure message is shown in the UI and written to telemetry. If it carried the API key, a provider
/// outage would become a credential disclosure.
#[test]
fn no_transport_failure_message_ever_carries_the_credential() {
    let server = Canned::serving(
        b"HTTP/1.1 500 Internal Server Error\r\nContent-Type: application/json\r\nContent-Length: 60\r\n\r\n{\"error\":{\"message\":\"upstream key sk-SHOULD-NOT-ECHO\"}}",
    );
    let outcome = call_openai_compatible(&server.base_url, 5);
    let message = match outcome {
        Ok(_) => panic!("a 500 must not be a success"),
        Err(message) => message,
    };
    for marker in [
        "sk-SHOULD-NOT-ECHO",
        "ADVERSARIAL-SECRET",
        "PROBELAB_ADVERSARIAL_KEY",
    ] {
        assert!(
            !message.contains(marker),
            "a failure message leaked {marker}: {message}"
        );
    }
}

// # Section 2 -- model protocol
//
// The model's output is what the engine acts on. Every case here must be refused *as a protocol failure*
// and must not be confused with the environment rejecting a legal choice.

// # Section 3 -- replay integrity
//
// A replay is evidence. Evidence that can be edited without the digest changing is not evidence, and a
// replay that reconstructs a different run than the one recorded is worse than no replay at all.

/// Editing any result-bearing field changes the digest.
///
/// Every field the panel renders as "what happened" is bound, one at a time. Testing them individually
/// rather than as a group is the point: a digest that covers the ply history but not `outcome` would
/// pass a single "edit the history" test while leaving the most consequential field unprotected, and the
/// replay panel displays that digest as evidence the recording is intact.
#[test]
fn a_tampered_recording_changes_the_digest() {
    let original = drawn_match();

    // The identity of the match.
    /// One field and a closure that edits only that field.
    type Edit = (&'static str, Box<dyn Fn(&mut probelab_lab::ArenaMatchView)>);
    let edits: Vec<Edit> = vec![
        (
            "match_id",
            Box::new(|v: &mut probelab_lab::ArenaMatchView| v.match_id = "other".into()),
        ),
        (
            "game",
            Box::new(|v: &mut probelab_lab::ArenaMatchView| v.game = "rps".into()),
        ),
        (
            "seed",
            Box::new(|v: &mut probelab_lab::ArenaMatchView| v.seed = 8),
        ),
        (
            "tested_agent",
            Box::new(|v: &mut probelab_lab::ArenaMatchView| v.tested_agent = "greedy".into()),
        ),
        (
            "opponent_agent",
            Box::new(|v: &mut probelab_lab::ArenaMatchView| v.opponent_agent = "greedy".into()),
        ),
        (
            "tested_side",
            Box::new(|v: &mut probelab_lab::ArenaMatchView| {
                v.tested_side = probelab_core::GameSide::Second
            }),
        ),
        // The result, which is the part that matters most.
        (
            "outcome",
            Box::new(|v: &mut probelab_lab::ArenaMatchView| {
                v.outcome = probelab_lab::ArenaOutcome::Loss
            }),
        ),
        (
            "winner",
            Box::new(|v: &mut probelab_lab::ArenaMatchView| {
                v.winner = Some(probelab_core::GameSide::First)
            }),
        ),
        (
            "plies",
            Box::new(|v: &mut probelab_lab::ArenaMatchView| v.plies = 99),
        ),
        (
            "final_board",
            Box::new(|v: &mut probelab_lab::ArenaMatchView| {
                if let probelab_core::GameBoard::TicTacToe { cells } = &mut v.final_board {
                    cells[0] = probelab_core::TicTacToeCell::Second;
                }
            }),
        ),
        (
            "illegal_moves",
            Box::new(|v: &mut probelab_lab::ArenaMatchView| {
                v.illegal_moves = vec![probelab_lab::IllegalMoveView {
                    ply: 1,
                    attempted: None,
                    reason: "cell already taken".to_owned(),
                }];
            }),
        ),
        (
            "failure",
            Box::new(|v: &mut probelab_lab::ArenaMatchView| {
                v.failure = Some("engine failed".into())
            }),
        ),
        // The ply history.
        (
            "history",
            Box::new(|v: &mut probelab_lab::ArenaMatchView| v.history.clear()),
        ),
        (
            "a played move",
            Box::new(|v: &mut probelab_lab::ArenaMatchView| {
                v.history[0].played = Some(GameMove::TicTacToe(4));
            }),
        ),
    ];

    let baseline = original.replay_digest();
    for (field, edit) in edits {
        let mut tampered = original.clone();
        edit(&mut tampered);
        assert_ne!(
            baseline,
            tampered.replay_digest(),
            "editing {field} left the digest unchanged, so that edit is undetectable"
        );
    }

    // And the unedited recording is stable.
    assert_eq!(baseline, original.replay_digest());
}

/// A replay is deterministic: the same recording gives identical frames every time.
#[test]
fn a_replay_is_deterministic() {
    let match_view = drawn_match();
    let first = probelab_lab::replay_match(&match_view);
    let second = probelab_lab::replay_match(&match_view);
    assert_eq!(first.digest, second.digest);
    assert_eq!(first.frames.len(), second.frames.len());
    for (a, b) in first.frames.iter().zip(second.frames.iter()) {
        assert_eq!(a.index, b.index);
        assert_eq!(
            a.digest, b.digest,
            "frame {} differs between two replays of one recording",
            a.index
        );
    }
}

/// Replaying does not mutate the recording it reads.
#[test]
fn a_replay_does_not_mutate_its_recording() {
    let match_view = drawn_match();
    let before = format!("{match_view:?}");
    let _ = probelab_lab::replay_match(&match_view);
    let _ = probelab_lab::replay_match(&match_view);
    assert_eq!(
        before,
        format!("{match_view:?}"),
        "replaying changed the recording; a second replay would not be the same evidence"
    );
}

/// A recording with no frames replays as empty and says so, rather than failing.
///
/// An empty recording is a real state -- a run that produced nothing. A panel given an error for it cannot
/// tell "nothing was recorded" from "the replay subsystem is broken", and those need different responses.
#[test]
fn an_empty_recording_replays_as_empty_and_states_the_limitation() {
    let mut match_view = drawn_match();
    match_view.history.clear();
    let replay = probelab_lab::replay_match(&match_view);
    assert!(
        !replay.limitations.is_empty(),
        "a recording with no frames must say so as a limitation, not present as complete"
    );
    assert_eq!(
        replay.capability,
        probelab_lab::ReplayCapability::ProjectedOnly,
        "a recording with no intermediate states is not re-executable"
    );
}

/// The terminal frame carries the recording digest; intermediate frames carry none, by design.
///
/// The engine records no per-ply digest, so the replay invents none -- a placeholder would look like
/// evidence. What must hold is that the digest appears *somewhere*, on the frame that states the result,
/// so a reader who checks one frame finds the thing worth checking.
#[test]
fn the_terminal_frame_carries_the_digest_and_intermediate_frames_carry_none() {
    let match_view = drawn_match();
    let replay = probelab_lab::replay_match(&match_view);
    let terminal = replay
        .frames
        .iter()
        .find(|frame| frame.source == probelab_lab::ReplaySource::Terminal)
        .expect("a replay has a terminal frame");
    assert_eq!(
        terminal.digest.as_deref(),
        Some(match_view.replay_digest().as_str()),
        "the terminal frame is where the result is stated, so it is where the digest belongs"
    );
    for frame in replay.frames.iter() {
        if frame.source != probelab_lab::ReplaySource::Terminal {
            assert_eq!(
                frame.digest, None,
                "frame {} invented a digest the engine never recorded",
                frame.index
            );
        }
    }
}

// # Section 4 -- the distinction the whole security story rests on

/// A blocked probe is a successful test, and the two are not the same value.
///
/// This is the single most important assertion in the file, and it is written to fail in *both*
/// directions. If `boundary_held` were false for a refused probe, the lab would be reporting a boundary it
/// cannot demonstrate. If `is_security_finding` were true, it would be crying wolf about its own working
/// control. Both are failures, and both are asserted.
#[test]
fn a_blocked_probe_is_a_successful_test_and_never_a_security_finding() {
    use probelab_lab::BoundaryOutcome::*;

    for outcome in [BrokerRefused, PolicyDenied, RefusedAndDenied] {
        assert!(
            outcome.boundary_held(),
            "{outcome:?} means the boundary acted; reporting it as not holding is a false alarm"
        );
        assert!(
            !outcome.is_security_finding(),
            "{outcome:?} is a working control being exercised, not a vulnerability"
        );
    }
    for outcome in [NotAttempted, Failed] {
        assert!(
            !outcome.boundary_held(),
            "{outcome:?} is not a refusal, so it cannot demonstrate a boundary holding"
        );
        assert!(
            !outcome.is_security_finding(),
            "{outcome:?} is a harness failure, not evidence of a model violating a boundary"
        );
    }
}

/// A real sweep finds nothing, because the boundary holds against the lab's own probes.
///
/// If this ever produced a finding, the honest reading would be that a boundary failed -- not that the
/// product found a vulnerability in itself. The lab is the attacker here, and the attacker loses.
#[test]
fn the_labs_own_probes_find_nothing() {
    let report = probelab_lab::sweep("grid-world", 3, 1).expect("a sweep runs");
    assert!(
        !report.outcomes.is_empty(),
        "a sweep that ran no probes proves nothing"
    );
    for outcome in &report.outcomes {
        assert!(
            outcome.failure.is_none(),
            "probe {} failed to run, so it tested nothing",
            outcome.probe_id
        );
        assert!(
            outcome.outcome.boundary_held(),
            "probe {} never reached an enforcement layer; it did not test the boundary",
            outcome.probe_id
        );
        assert!(
            !outcome.is_security_finding(),
            "probe {} was recorded as a finding, but a refused probe is a passing test",
            outcome.probe_id
        );
    }
    assert!(
        report.findings().is_empty(),
        "the lab's own probes must not manufacture findings: {:?}",
        report.findings()
    );
}

/// There is no combined security total, because summing refusals and violations produces a number that
/// rises when the sandbox works correctly.
///
/// The `SecuritySummary` type has no `total` field and this asserts the reason holds: every field is one
/// of the two sides, and nothing adds them together.
#[test]
fn there_is_no_combined_security_total() {
    use probelab_lab::{ExperimentPlanError, LabBrain};
    let mut brain = LabBrain::new();
    let error = brain
        .make_plan(
            &probelab_lab::PlanRequest {
                benchmark_id: Some("nope".to_owned()),
                target_agent: Some("optimal".to_owned()),
                probe_agents: Vec::new(),
                episodes: Some(1),
                root_seed: None,
            },
            &probelab_benchmarks::BenchmarkRegistry::built_in(),
            &probelab_lab::AgentRegistry::built_in(),
            &probelab_lab::ProbeRegistry::built_in(),
            probelab_lab::runner::current_platform(),
        )
        .expect_err("an unknown benchmark must be refused");
    assert!(matches!(
        error,
        ExperimentPlanError::UnknownBenchmark { .. }
    ));
}

/// A broker denial records no action, no transition, and no movement.
///
/// The telemetry shape matters as much as the outcome. A denial carrying a `selected_action` would look,
/// to anything reading the event log, like the model chose to do nothing -- a different and much weaker
/// claim than "the model asked and was refused".
#[test]
fn a_broker_denial_records_no_action_and_no_movement() {
    let event = TelemetryEvent {
        run_id: "adversarial".to_owned(),
        episode_id: "episode-0".to_owned(),
        sequence: 0,
        recorded_at_unix_ms: 0,
        turn: 3,
        observation: sample_observation(),
        selected_action: None,
        game: None,
        requested_capability: Some(RequestedCapability::ToolUse("filesystem.read".to_owned())),
        action_outcome: None,
        reward: 0,
        invalid_action: false,
        timeout: false,
        resource_info: None,
        integrity_event: None,
        model_call: None,
        model_failure: None,
        broker_denial: None,
        process_lifecycle: None,
    };
    assert!(
        event.selected_action.is_none(),
        "a denial that carried an action would be indistinguishable from the model choosing to wait"
    );
    assert!(
        event.action_outcome.is_none(),
        "a denial that carried a transition outcome would claim the environment did something"
    );
    assert_eq!(event.reward, 0, "a denial must not move the world");
    assert!(
        !event.invalid_action,
        "a denial is not an invalid action; the two categories are deliberately separate"
    );
}

/// A refused spawn and a run spawn are distinguishable in telemetry.
///
/// `ProcessLifecycle::denied` and `::authorized` are the only two constructors, and the discriminator that
/// matters is `execution`: absent exactly when the request was refused. That is what keeps a refusal from
/// being read as a process that ran.
#[test]
fn a_refused_spawn_and_a_run_spawn_are_distinguishable() {
    use probelab_core::ProcessAuthorization;

    let refused = ProcessLifecycle::denied(CapabilityDenial::new(
        CapabilityPermission::ProcessSpawn,
        probelab_core::IntegrityEventKind::BrokerProcessSpawnBlocked,
        "not on the program allowlist",
    ));
    assert!(refused.requested);
    assert!(matches!(refused.decision, ProcessAuthorization::Denied(_)));
    assert!(
        refused.execution.is_none(),
        "a refused spawn recorded an OS outcome, so it is indistinguishable from one that ran"
    );

    // And the denial itself carries no requested value into the event log, so a persisted refusal cannot
    // be turned into a probe for what the model tried.
    let json = serde_json::to_string(&refused).expect("a lifecycle serializes");
    for marker in ["sk-", "Bearer ", "api_key", "cmd.exe"] {
        assert!(
            !json.contains(marker),
            "a lifecycle leaked {marker} into a refusal: {json}"
        );
    }
}

/// Broker categories and policy categories are disjoint.
///
/// Five integrity counters and five policy counters with no overlap. If they overlapped, a security panel
/// would report rising numbers every time the sandbox worked correctly, which is the fastest way to make
/// such a panel unreadable and then ignored.
#[test]
fn broker_categories_and_policy_categories_are_disjoint() {
    use probelab_core::IntegrityEventKind::*;

    let broker = [
        BrokerUnauthorizedRead,
        BrokerUnauthorizedWrite,
        BrokerProcessSpawnBlocked,
        BrokerNetworkAttempt,
        BrokerEnvironmentInspectDenied,
    ];
    let policy = [
        UnauthorizedAccessAttempt,
        PolicyViolation,
        SandboxViolation,
        EnvironmentTampering,
        NetworkViolation,
    ];
    assert_eq!(broker.len(), 5, "the broker category count is load-bearing");
    assert_eq!(policy.len(), 5, "the policy category count is load-bearing");
    for category in broker {
        assert!(
            category.is_broker_category(),
            "{category:?} is a broker category and must say so"
        );
    }
    for category in policy {
        assert!(
            !category.is_broker_category(),
            "{category:?} is a policy category and must not be counted as a broker one"
        );
    }
    // The case most likely to be conflated, since both concern the network.
    assert!(BrokerNetworkAttempt.is_broker_category());
    assert!(!NetworkViolation.is_broker_category());
}

/// A model decision carries no authorization handle, so there is nothing for a model to reach through.
///
/// Structural rather than behavioural: the `Agent` trait takes a `ModelRequest` and returns a
/// `ModelDecision`, and there is no parameter through which a broker, a policy, or a handle to either
/// could arrive. Every variant is constructible from data alone.
#[test]
fn a_model_decision_carries_no_authorization_handle() {
    use std::path::PathBuf;

    let decisions = [
        ModelDecision::GridAction(Action::Up),
        ModelDecision::GridAction(Action::Wait),
        ModelDecision::CapabilityProbe(CapabilityRequest::FilesystemRead {
            path: PathBuf::from("C:/Windows"),
        }),
        ModelDecision::CapabilityProbe(CapabilityRequest::ProcessSpawn {
            program: "cmd.exe".to_owned(),
            args: Vec::new(),
        }),
        ModelDecision::CapabilityProbe(CapabilityRequest::NetworkConnect {
            endpoint: "example.com:443".to_owned(),
        }),
        ModelDecision::CapabilityProbe(CapabilityRequest::EnvironmentInspect {
            variable: "PATH".to_owned(),
        }),
        ModelDecision::Capability(RequestedCapability::EvaluatorAccess),
        ModelDecision::ProcessSpawnRequest,
    ];
    for decision in decisions {
        let text = format!("{decision:?}");
        for forbidden in ["Broker", "Policy", "Grant", "Allow", "Authorized"] {
            assert!(
                !text.contains(forbidden),
                "a model decision names an authorization concept ({forbidden}): {text}"
            );
        }
    }
}

// # Section 5 -- the planner refuses before anything runs
//
// The cheapest boundary is the one that refuses before allocating. A planner that accepted an unbounded
// request would be a resource-exhaustion vector pointed at the lab itself.

/// An out-of-range episode count is refused by name before any episode starts.
#[test]
fn an_out_of_range_episode_count_is_refused() {
    for episodes in [0usize, probelab_lab::MAX_EPISODES + 1, usize::MAX] {
        let error = plan_for(episodes)
            .expect_err("an out-of-range episode count must be refused before anything runs");
        assert!(
            matches!(
                error,
                probelab_lab::ExperimentPlanError::EpisodeCountOutOfRange { .. }
            ),
            "{episodes} episodes: the refusal must name the constraint, got {error:?}"
        );
    }
    // The documented maximum is accepted, so this is a bound and not a blanket refusal.
    assert!(
        plan_for(probelab_lab::MAX_EPISODES).is_ok(),
        "the documented maximum must remain usable"
    );
}

/// An unknown benchmark or agent is refused by name, not defaulted.
#[test]
fn an_unknown_benchmark_or_agent_is_refused_by_name() {
    for (benchmark, agent) in [
        ("not-a-benchmark", "optimal"),
        ("grid-world", "not-an-agent"),
        ("../../etc/passwd", "optimal"),
    ] {
        let error = plan_for_identifiers(benchmark, agent)
            .expect_err("an unresolvable identifier must be refused");
        let text = error.to_string();
        assert!(
            text.contains(benchmark) || text.contains(agent),
            "the refusal must name what was asked for, got {text}"
        );
    }
}

/// A Windows-only benchmark is refused at plan time on a host that cannot contain it.
///
/// `sandbox-process` declares Windows containment. Elsewhere the spawn would fail closed at the last
/// moment, after a measurement had been planned and stored as `running`. Refusing at plan time is the
/// honest answer, and it is also the only one that does not leave a run in an unrecoverable state.
#[test]
fn a_platform_incompatible_benchmark_is_refused_at_plan_time() {
    if cfg!(target_os = "windows") {
        // The containment it needs exists here, so there is nothing to refuse.
        return;
    }
    let outcome = plan_for_identifiers("sandbox-process", "optimal");
    assert!(
        outcome.is_err(),
        "a Windows-only benchmark must be refused on a host that cannot contain it"
    );
}

// # Helpers

/// A well-formed grid observation.
fn sample_observation() -> Observation {
    Observation {
        turn: 1,
        position: probelab_core::Coordinate { x: 0, y: 0 },
        grid_size: probelab_core::GridSize {
            width: 5,
            height: 5,
        },
        current_tile: probelab_core::TileKind::Safe,
        visible_neighbors: Vec::new(),
        distance_to_goal: 4,
        max_turns: 8,
        remaining_turns: 7,
        available_actions: vec![
            Action::Up,
            Action::Down,
            Action::Left,
            Action::Right,
            Action::Wait,
        ],
    }
}

/// A well-formed grid `ModelRequest`, for the transport tests.
fn sample_model_request() -> ModelRequest {
    ModelRequest {
        episode_id: "adversarial".to_owned(),
        turn: 1,
        observation: sample_observation(),
        available_actions: vec![
            Action::Up,
            Action::Down,
            Action::Left,
            Action::Right,
            Action::Wait,
        ],
        process_spawn_available: false,
        history: Vec::new(),
        game: None,
    }
}

/// Calls the real OpenAI-compatible transport against a local server and returns its error message.
///
/// Uses the shipped provider rather than a hand-rolled HTTP client, because the question is what the
/// *product's* transport does with a hostile server, not what an HTTP library does. The provider is async,
/// so this builds a current-thread runtime and blocks on it -- which is exactly what `run_benchmark` does
/// on its blocking worker, so the timeout observed here is the transport's own rather than a test
/// harness's.
fn call_openai_compatible(base_url: &str, timeout_seconds: u64) -> Result<(), String> {
    use probelab_agent::AsyncProviderAgent;
    use probelab_core::Agent;
    use probelab_model_adapters::{OpenAiCompatibleConfig, OpenAiCompatibleProvider};

    // Set and removed inside the call so the credential exists only for its lifetime, and so a failure
    // message that echoed the environment would have something to leak.
    std::env::set_var("PROBELAB_ADVERSARIAL_KEY", "sk-ADVERSARIAL-SECRET-VALUE");
    let outcome = (|| {
        let config = OpenAiCompatibleConfig {
            base_url: base_url.to_owned(),
            api_key_env: "PROBELAB_ADVERSARIAL_KEY".to_owned(),
            model: "adversarial-model".to_owned(),
            timeout_seconds,
            temperature: 0.0,
            max_tokens: 32,
        };
        let provider = OpenAiCompatibleProvider::new(config).map_err(|error| error.to_string())?;
        // `AsyncProviderAgent` owns the async provider *and* the current-thread runtime that drives it,
        // which is exactly the construction the desktop's `run_benchmark` performs. Calling `decide` on it
        // is the whole shipped path, not a reimplementation of it.
        let mut agent = AsyncProviderAgent::from_provider(provider)?;
        agent.decide(sample_model_request()).map(|_| ())
    })();
    std::env::remove_var("PROBELAB_ADVERSARIAL_KEY");
    outcome
}

/// Asks the Brain to plan an experiment with the given episode count, through the real registries.
fn plan_for(
    episodes: usize,
) -> Result<probelab_lab::ExperimentPlan, probelab_lab::ExperimentPlanError> {
    probelab_lab::LabBrain::new().make_plan(
        &probelab_lab::PlanRequest {
            benchmark_id: Some("grid-world".to_owned()),
            target_agent: Some("optimal".to_owned()),
            probe_agents: Vec::new(),
            episodes: Some(episodes),
            root_seed: None,
        },
        &probelab_benchmarks::BenchmarkRegistry::built_in(),
        &probelab_lab::AgentRegistry::built_in(),
        &probelab_lab::ProbeRegistry::built_in(),
        probelab_lab::runner::current_platform(),
    )
}

/// Asks the Brain to plan an experiment naming specific identifiers.
fn plan_for_identifiers(
    benchmark_id: &str,
    agent_id: &str,
) -> Result<probelab_lab::ExperimentPlan, probelab_lab::ExperimentPlanError> {
    probelab_lab::LabBrain::new().make_plan(
        &probelab_lab::PlanRequest {
            benchmark_id: Some(benchmark_id.to_owned()),
            target_agent: Some(agent_id.to_owned()),
            probe_agents: Vec::new(),
            episodes: Some(1),
            root_seed: None,
        },
        &probelab_benchmarks::BenchmarkRegistry::built_in(),
        &probelab_lab::AgentRegistry::built_in(),
        &probelab_lab::ProbeRegistry::built_in(),
        probelab_lab::runner::current_platform(),
    )
}

/// A minimal but real arena match recording, for the replay tests.
fn drawn_match() -> probelab_lab::ArenaMatchView {
    let empty_row = probelab_core::TicTacToeCell::Empty;
    let mut cells = vec![empty_row; 9];
    cells[0] = probelab_core::TicTacToeCell::First;
    probelab_lab::ArenaMatchView {
        match_id: "arena-ttt-adversarial".to_owned(),
        game: "tic_tac_toe".to_owned(),
        game_name: "Tic-Tac-Toe".to_owned(),
        outcome: probelab_lab::ArenaOutcome::Draw,
        tested_agent: "perfect_tictactoe".to_owned(),
        opponent_agent: "first_legal_tictactoe".to_owned(),
        tested_side: probelab_core::GameSide::First,
        seed: 7,
        plies: 1,
        final_board: probelab_core::GameBoard::TicTacToe {
            cells: cells.clone(),
        },
        winner: None,
        illegal_moves: Vec::new(),
        history: vec![probelab_lab::PlyView {
            ply: 1,
            side: probelab_core::GameSide::First,
            board: probelab_core::GameBoard::TicTacToe { cells },
            played: Some(GameMove::TicTacToe(0)),
        }],
        timeline: Vec::new(),
        failure: None,
    }
}
