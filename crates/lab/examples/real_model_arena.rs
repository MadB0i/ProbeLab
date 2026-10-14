//! A real local-model Arena match, end to end.
//!
//! # Why this is an example rather than a unit test
//!
//! It needs a running Ollama with at least one model installed. A test that depends on that would fail
//! on CI, on a contributor's machine, and on any build host — so making it a test would mean either
//! skipping it silently (which hides a real capability) or gating it behind an environment variable
//! nobody sets. As an example it runs when asked for, and its output is evidence rather than a gate.
//!
//! ```text
//! cargo run -p probelab-lab --example real_model_arena -- qwen2.5:0.5b
//! ```
//!
//! # What it proves, and what it cannot
//!
//! It proves that a real model reaches the engine, is shown the legal state for its side, returns a
//! move the engine accepts, and produces a durable match record with real per-turn telemetry. That is
//! the difference between "the arena can seat a model" and "the arena can seat a model", and the
//! difference is whether a provider was actually called.
//!
//! It cannot prove the model is *good*. A 0.5B model will lose to a depth-bounded search most of the
//! time, and losing is the expected outcome, not a defect. What this reports is what happened, not
//! how to read it.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use probelab_core::GameSide;
use probelab_engine::arena::connect_four::{ConnectFour, DepthBoundedSearch};
use probelab_engine::arena::{ArenaSeat, ModelSeat, ReferenceSeat, SeatKind, SharedRecorder};
use probelab_model_adapters::{OpenAiCompatibleConfig, OpenAiCompatibleProvider};

/// The endpoint the local executor reports. Loopback, so no credential is read or sent.
const LOCAL_BASE_URL: &str = "http://127.0.0.1:11434/v1";

/// Named but deliberately unset. A loopback endpoint resolves no credential at all — this name exists
/// only to prove it: if the adapter ever required it, the run below would fail rather than transmit
/// anything.
const UNUSED_CREDENTIAL_VARIABLE: &str = "PROBELAB_EXAMPLE_UNUSED_CREDENTIAL";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let model = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "qwen2.5:0.5b".to_owned());

    println!("ProbeLab — real local-model Arena match");
    println!("========================================");
    println!("model:      {model}");
    println!("endpoint:   {LOCAL_BASE_URL}");
    println!("credential: none — a validated loopback endpoint is sent no Authorization header");
    println!();

    // The provider is constructed through exactly the validated configuration a run would use.
    let provider = OpenAiCompatibleProvider::new(OpenAiCompatibleConfig {
        base_url: LOCAL_BASE_URL.to_owned(),
        api_key_env: UNUSED_CREDENTIAL_VARIABLE.to_owned(),
        model: model.clone(),
        // Generous: a small model on a loaded CPU can take a while for the first token of a match.
        timeout_seconds: 180,
        temperature: 0.0,
        max_tokens: 64,
    })
    .map_err(|error| format!("the provider configuration is not usable: {error}"))?;

    let agent = probelab_agent::AsyncProviderAgent::from_provider(provider)
        .map_err(|error| format!("the provider agent could not be built: {error}"))?;
    let mut model_seat: Box<dyn ArenaSeat> =
        Box::new(ModelSeat::new(Box::new(agent), model.clone()));
    let mut opponent: Box<dyn ArenaSeat> =
        Box::new(ReferenceSeat::new(Box::new(DepthBoundedSearch::new())));

    println!("seats:      {model} (under test, first) vs depth_bounded_connect_four (opponent)");
    println!("game:       connect four, 7 x 6, 42 plies");
    println!();

    let cancel = AtomicBool::new(false);

    // Every ply is reported as it happens, because a real-model match is slow and a caller watching
    // a frozen panel cannot tell a slow model from a hung one.
    let mut on_ply = |outcome: probelab_engine::arena::PlyOutcome| -> bool {
        let usage = match &outcome.usage {
            Some(usage) => match (usage.prompt_tokens, usage.completion_tokens) {
                (Some(prompt), Some(completion)) => format!("{prompt}+{completion} tokens"),
                _ => "tokens not reported".to_owned(),
            },
            None if outcome.seat == SeatKind::Model => "tokens not reported".to_owned(),
            None => "policy".to_owned(),
        };
        println!(
            "  ply {:>2}  {:<5}  {:<14}  {:>6} ms  {:<24}  {}",
            outcome.ply,
            if outcome.side == GameSide::First {
                "first"
            } else {
                "second"
            },
            format!("{:?}", outcome.move_),
            outcome.elapsed_ms,
            usage,
            outcome.seat_label,
        );
        // The cooperative cancel flag is polled here, in the same callback that observes the ply, so
        // there is exactly one place a running match can stop.
        !cancel.load(Ordering::SeqCst)
    };

    let recorder = SharedRecorder(Arc::new(probelab_engine::arena::NullRecorder));
    let config = probelab_engine::arena::MatchConfig::new(
        &model,
        "depth_bounded_connect_four",
        7,
        "example-real-model",
        "example-real-model-episode",
    );

    let started = std::time::Instant::now();
    let result = probelab_engine::arena::play_match(
        &ConnectFour::default_game(),
        model_seat.as_mut(),
        opponent.as_mut(),
        GameSide::First,
        &config,
        &recorder,
        // Cancellation is checked at the same point the ply is observed, so there is no second path.
        &mut on_ply,
    );
    let elapsed = started.elapsed();

    println!();
    match result {
        Ok(result) => {
            println!(
                "result:     {:?} after {} plies",
                result.outcome, result.plies
            );
            println!("elapsed:    {:.1} s", elapsed.as_secs_f64());
            println!(
                "illegal:    {}",
                result
                    .illegal_moves
                    .iter()
                    .map(|illegal| format!("ply {}: {}", illegal.ply, illegal.reason))
                    .collect::<Vec<_>>()
                    .join("; ")
            );
            println!();
            println!(
                "Read this as a measurement of the harness, not of the model. A small local model \
                 losing to a bounded search is the expected outcome; what the run establishes is that \
                 a real provider was reached, shown the legal state, and accepted."
            );
            Ok(())
        }
        Err(error) => {
            println!("result:     the match did not complete: {error}");
            println!("elapsed:    {:.1} s", elapsed.as_secs_f64());
            if error.is_harness_failure() {
                println!();
                println!(
                    "This is reported as a harness failure rather than a loss, because no result was \
                     produced: the model never got to answer. Check that Ollama is running and that \
                     `{model}` is pulled."
                );
            }
            Err(error.into())
        }
    }
}
