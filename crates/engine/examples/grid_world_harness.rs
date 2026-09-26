//! Runs the grid-world capability measurement and prints the aggregate.
//!
//! This is the command-line surface of the multi-episode harness. It exists so a measurement can be
//! reproduced and inspected without the desktop app, which still runs a single episode and shows no
//! interval.
//!
//! It measures four policies over the evaluation pool: two scripted baselines that establish the
//! floor, the search-based reference solver that establishes the ceiling, and — when a model
//! environment variable is set — a connected OpenAI-compatible provider.
//!
//! ```text
//! cargo run -p probelab-engine --example grid_world_harness -- --episodes 70
//! ```

use std::sync::Arc;

use probelab_agent::{AsyncProviderAgent, ProviderAgent};
use probelab_benchmarks::GridWorldBenchmark;
use probelab_core::{Agent, Benchmark, TelemetryRecorder};
use probelab_engine::harness::{Harness, MeasurementPlan, SeedPools};
use probelab_engine::BenchmarkRunner;
use probelab_evaluator::GridWorldEvaluator;
use probelab_model_adapters::{
    LookaheadSolver, MockModelConfig, MockModelProvider, OpenAiCompatibleConfig,
    OpenAiCompatibleProvider,
};
use probelab_telemetry::SqliteTelemetryRecorder;

fn episodes_argument() -> usize {
    let mut arguments = std::env::args().skip(1);
    while let Some(argument) = arguments.next() {
        if argument == "--episodes" {
            if let Some(value) = arguments.next() {
                if let Ok(parsed) = value.parse::<usize>() {
                    return parsed;
                }
            }
        }
    }
    probelab_engine::harness::DEFAULT_EPISODES
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let benchmark = GridWorldBenchmark::built_in()?;
    let definition = benchmark.definition();
    let episodes = episodes_argument();

    let pools = SeedPools::new(
        definition.dev_seed_pool.clone(),
        definition.eval_seed_pool.clone(),
    )?;
    if !pools.are_disjoint() {
        return Err("dev and eval seed pools overlap".into());
    }

    let scratch = std::env::temp_dir().join(format!("probelab-harness-{}", std::process::id()));
    std::fs::create_dir_all(&scratch)?;
    let scratch_root = scratch.clone();
    let recorder_factory: probelab_engine::harness::RecorderFactory =
        Arc::new(move |run_id: &str| -> Arc<dyn TelemetryRecorder> {
            let database = scratch_root.join(format!("{run_id}.sqlite3"));
            Arc::new(SqliteTelemetryRecorder::with_callback(&database, |_| {}).expect("recorder"))
        });

    let harness = Harness::new(pools, recorder_factory);
    let runner = BenchmarkRunner::new();
    let evaluator = GridWorldEvaluator::new();
    let plan = MeasurementPlan::eval(episodes);
    let benchmark_ref: &dyn Benchmark = &benchmark;

    println!("grid-world capability measurement");
    println!(
        "  benchmark       {} v{}",
        definition.id, definition.version
    );
    println!("  episodes        {episodes}");
    println!(
        "  pool            eval ({} seeds, disjoint from the {} dev seeds)",
        definition.eval_seed_pool.len(),
        definition.dev_seed_pool.len()
    );
    println!("  catalog ceiling {} turns", definition.max_turns);
    println!();

    let report = |label: &str, score: &probelab_engine::harness::AggregateScore| {
        let rate = score.success_rate;
        println!(
            "  {label:<22} {:>3}/{:<3}  rate {:.3}  95% CI [{:.3}, {:.3}]  reward {:>7.2}  turns {:>5.2}  {:?}",
            score.successes,
            score.episodes,
            rate.point,
            rate.lower,
            rate.upper,
            score.mean_reward,
            score.mean_turns,
            score.status_counts,
        );
    };

    let mut random_factory = |_seed: u64| -> Box<dyn Agent> {
        Box::new(ProviderAgent::from_provider(MockModelProvider::new(
            MockModelConfig::Random,
            0,
        )))
    };
    report(
        "random (baseline)",
        &harness.measure(
            benchmark_ref,
            &evaluator,
            &runner,
            &plan,
            "random",
            &mut random_factory,
        )?,
    );

    let mut greedy_factory = |_seed: u64| -> Box<dyn Agent> {
        Box::new(ProviderAgent::from_provider(MockModelProvider::new(
            MockModelConfig::Optimal,
            0,
        )))
    };
    report(
        "greedy (baseline)",
        &harness.measure(
            benchmark_ref,
            &evaluator,
            &runner,
            &plan,
            "greedy",
            &mut greedy_factory,
        )?,
    );

    let mut lookahead_factory = |seed: u64| -> Box<dyn Agent> {
        Box::new(ProviderAgent::from_provider(LookaheadSolver::new(seed)))
    };
    report(
        "lookahead (reference)",
        &harness.measure(
            benchmark_ref,
            &evaluator,
            &runner,
            &plan,
            "lookahead",
            &mut lookahead_factory,
        )?,
    );

    if let Some(config) = connected_config() {
        let mut connected = |_seed: u64| -> Box<dyn Agent> {
            let provider = OpenAiCompatibleProvider::new(config.clone())
                .expect("validated provider configuration");
            Box::new(AsyncProviderAgent::from_provider(provider).expect("async provider agent"))
        };
        report(
            "connected model",
            &harness.measure(
                benchmark_ref,
                &evaluator,
                &runner,
                &plan,
                "connected",
                &mut connected,
            )?,
        );
    } else {
        println!();
        println!("  (set PROBELAB_MODEL_BASE_URL, PROBELAB_MODEL_NAME and");
        println!("   PROBELAB_MODEL_API_KEY_ENV to measure a connected provider)");
    }

    let _ = std::fs::remove_dir_all(&scratch);
    Ok(())
}

/// Reads a connected-provider configuration from the environment, if one is fully specified.
///
/// Credentials stay environment-only: this reads the *name* of the variable holding the key and the
/// provider reads the key itself. Nothing here prints or stores a secret.
fn connected_config() -> Option<OpenAiCompatibleConfig> {
    let base_url = std::env::var("PROBELAB_MODEL_BASE_URL").ok()?;
    let model = std::env::var("PROBELAB_MODEL_NAME").ok()?;
    let api_key_env = std::env::var("PROBELAB_MODEL_API_KEY_ENV")
        .unwrap_or_else(|_| "OPENAI_COMPATIBLE_API_KEY".to_owned());
    let config = OpenAiCompatibleConfig {
        base_url,
        api_key_env,
        model,
        timeout_seconds: 30,
        temperature: 0.0,
        max_tokens: 400,
    };
    config.validate().ok()?;
    Some(config)
}
