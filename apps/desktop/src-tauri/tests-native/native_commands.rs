//! Commands through the real Tauri runtime: state resolution and argument casing.
//!
//! # Location note: why this file lives outside `tests/`
//!
//! Cargo auto-builds everything under `tests/` as a test target, and any
//! binary that links the Tauri mock runtime exits with `0xC0000139` before
//! `main` on the Windows host where this file was written (empty stdout and
//! stderr; identical-closure binaries without the mock runtime run normally;
//! no shadowing DLL found, investigation stopped rather than pursued
//! speculatively). An auto-built target would therefore abort the whole
//! workspace suite on that host. So this file is preserved here —
//! implementation intact, assertions unweakened — and runs wherever the mock
//! runtime loads (including CI Linux) by copying it into `tests/` or by
//! adding an explicit `[[test]]` section:
//!
//! ```toml
//! [[test]]
//! name = "native_commands"
//! path = "tests-native/native_commands.rs"
//! ```
//!
//! Until then, the properties it pins are guarded by checks that run
//! everywhere: `every_command_reads_managed_state_that_exists` (compile-time
//! state-type pin) and the feature-matrix argument-key check.
//!
//! # What this file is for
//!
//! Unit tests call command functions directly, which skips the two things
//! Tauri itself does at invoke time: resolving `State<T>` from the managed
//! types, and matching argument keys by the command's case policy. Both have
//! been wrong here while the suite was green:
//!
//! * six commands took `State<BrainSurface>` / `State<ArenaSurface>` while
//!   the application manages only `AppState` — every invocation failed with
//!   "state not managed" in the built app;
//! * six call sites sent camelCase top-level argument keys while the commands
//!   declare `rename_all = "snake_case"` — every such invocation failed with
//!   a missing argument in the built app.
//!
//! These tests build a real (mock-runtime, headless) Tauri app, manage a
//! throwaway `AppState`, and invoke commands through `get_ipc_response` —
//! the same resolution and casing path the webview uses. A regression in
//! either property fails here instead of on a user's first click.
//!
//! # What this file is not
//!
//! A GUI test: no window renders and no pixels are asserted. If a full GUI
//! automation test becomes possible in this environment, it belongs in a new
//! file rather than weakening these assertions.
//!
//! # Environment limitation, stated precisely
//!
//! On the Windows host where this file was written, any test binary that
//! links the Tauri mock runtime exits with `0xC0000139`
//! (`STATUS_ENTRYPOINT_NOT_FOUND`) before `main` — with empty stdout and
//! stderr — while test binaries over the identical dependency closure that
//! do not reference the mock runtime run normally. A PATH scan showed no
//! shadowing DLLs (only `system32` entries, which bypass PATH via KnownDLLs,
//! plus JDK-bundled `vcruntime140.dll`, which cannot explain identical-closure
//! binaries loading fine), and no conflicting DLL was identified, so loader
//! investigation was stopped rather than pursued speculatively.
//!
//! Consequence: these tests compile here (which pins registration, names,
//! arities, and shapes) but cannot execute on this host. They execute
//! wherever the mock runtime loads — including CI Linux — and the properties
//! they pin are additionally guarded by `every_command_reads_managed_state_that_exists`
//! (compile-time, runs everywhere) and the feature-matrix argument-key check
//! (runs everywhere). No assertion here is weakened to pretend otherwise.
//!
//! # Why `run_experiment` is absent
//!
//! It takes `AppHandle`, which defaults to the WRY runtime and cannot resolve
//! through the mock runtime at all — a Tauri testing limitation, not a
//! product defect (the shipped app runs WRY, where it resolves). The
//! experiment read/export path below is seeded with a real `LabRunner` run
//! instead, so every native assertion still rests on measured data rather
//! than a hand-built fixture.

use probelab_desktop_lib::AppState;
use serde_json::json;
use std::convert::AsRef;

const WINDOW_LABEL: &str = "probelab-native-test";

/// One headless app with throwaway state, exactly as `run()` manages it:
/// a single `AppState`, nothing else.
///
/// `seed_experiment` runs a real one-episode grid-world experiment through
/// `LabRunner` and stores its read model first, so the read/export commands
/// below assert over measured data rather than fixtures.
fn test_app(
    tag: &str,
) -> (
    tauri::App<tauri::test::MockRuntime>,
    tauri::Webview<tauri::test::MockRuntime>,
) {
    test_app_with_seed(tag, false)
}

fn test_app_with_seed(
    tag: &str,
    seed_experiment: bool,
) -> (
    tauri::App<tauri::test::MockRuntime>,
    tauri::Webview<tauri::test::MockRuntime>,
) {
    let state = AppState::for_testing(tag).expect("throwaway test state");
    if seed_experiment {
        let mut brain = probelab_lab::LabBrain::new();
        let run = probelab_lab::LabRunner::new()
            .run(
                &mut brain,
                &probelab_lab::PlanRequest {
                    benchmark_id: Some("grid-world".to_owned()),
                    target_agent: Some("optimal".to_owned()),
                    probe_agents: Vec::new(),
                    episodes: Some(1),
                    root_seed: Some(1_000),
                },
                "native-seed-exp",
                &probelab_evaluator::GridWorldEvaluator::new(),
                std::sync::Arc::new(SilentRecorder),
            )
            .expect("a real one-episode run seeds the native assertions");
        let model = probelab_lab::BrainReadModel::from_run(
            &run,
            probelab_lab::BrainState::Completed,
            "idle",
        );
        state
            .brain
            .store("native-seed-exp".to_owned(), model)
            .expect("the seed experiment is stored");
    }
    let app = tauri::test::mock_builder()
        .invoke_handler(tauri::generate_handler![
            probelab_desktop_lib::brain_panel,
            probelab_desktop_lib::get_experiment,
            probelab_desktop_lib::list_experiments,
            probelab_desktop_lib::latest_arena_match,
            probelab_desktop_lib::replay_arena_match,
            probelab_desktop_lib::get_arena_match,
            probelab_desktop_lib::list_executors,
            probelab_desktop_lib::preflight_executor,
            probelab_desktop_lib::cancel_experiment,
            probelab_desktop_lib::export_experiment,
            probelab_desktop_lib::list_task_benchmarks,
            probelab_desktop_lib::run_task_eval,
            probelab_desktop_lib::guided_task_package,
            probelab_desktop_lib::score_guided_submission,
        ])
        .manage(state)
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("mock app builds");
    let webview = tauri::WebviewWindowBuilder::new(&app, WINDOW_LABEL, Default::default())
        .build()
        .expect("mock webview builds");
    let view = webview.as_ref().clone();
    (app, view)
}

/// A recorder that accepts telemetry and keeps none of it.
///
/// The native assertions below need recorded events to exist, not to be
/// readable back: persistence is covered by the telemetry suite, and this
/// file is about resolution, not storage.
#[derive(Clone)]
struct SilentRecorder;

impl probelab_core::TelemetryRecorder for SilentRecorder {
    fn record(&self, _event: &probelab_core::TelemetryEvent) -> Result<(), String> {
        Ok(())
    }
}

/// Invokes a command exactly as the webview would, with a bounded wait.
///
/// `get_ipc_response` blocks forever on a command that never answers; an
/// async command that hung the mock runtime would hang the suite the same
/// way. The timeout turns that into a failure with the command's name on it.
fn invoke(
    webview: &tauri::Webview<tauri::test::MockRuntime>,
    cmd: &str,
    body: serde_json::Value,
) -> Result<tauri::ipc::InvokeResponseBody, serde_json::Value> {
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    webview.clone().on_message(
        tauri::webview::InvokeRequest {
            cmd: cmd.into(),
            callback: tauri::ipc::CallbackFn(0),
            error: tauri::ipc::CallbackFn(1),
            url: "http://tauri.localhost".parse().expect("url"),
            body: tauri::ipc::InvokeBody::Json(body),
            headers: Default::default(),
            invoke_key: tauri::test::INVOKE_KEY.to_string(),
        },
        Box::new(move |_window, _cmd, response, _callback, _error| {
            let _ = sender.send(response);
        }),
    );
    let response = receiver
        .recv_timeout(std::time::Duration::from_secs(120))
        .unwrap_or_else(|_| panic!("{cmd} never answered through the mock runtime"));
    match response {
        tauri::ipc::InvokeResponse::Ok(body) => Ok(body),
        tauri::ipc::InvokeResponse::Err(tauri::ipc::InvokeError(value)) => Err(value),
    }
}

fn ok(body: tauri::ipc::InvokeResponseBody) -> serde_json::Value {
    body.deserialize::<serde_json::Value>()
        .expect("a JSON response body")
}

/// The brain read path resolves managed state in the built app.
///
/// Before the fix this exact invocation failed with "state not managed for
/// field `surface`", because the command asked for a type nobody manages.
/// `brain_panel` is the application's most-called command; its resolution is
/// the load-bearing property of this file.
#[test]
fn brain_panel_resolves_through_the_managed_state() {
    let (_app, webview) = test_app("brain-panel");
    let value = ok(invoke(&webview, "brain_panel", json!({})).expect("brain_panel resolves"));
    assert_eq!(value["brainState"], json!("planning"));
    assert_eq!(value["experiment"], serde_json::Value::Null);
}

/// `list_experiments` starts empty rather than failing.
#[test]
fn list_experiments_resolves_and_starts_empty() {
    let (_app, webview) = test_app("list-experiments");
    let value =
        ok(invoke(&webview, "list_experiments", json!({})).expect("list_experiments resolves"));
    assert_eq!(value, json!([]));
}

/// Unknown experiments are a typed refusal, not a resolution failure.
///
/// A resolution failure and a not-found refusal look identical from the
/// frontend (both are errors), so this asserts the error's content: it must
/// name the experiment, which only the command body can do.
#[test]
fn get_experiment_refuses_unknown_names_from_the_command_body() {
    let (_app, webview) = test_app("get-experiment");
    let error = invoke(
        &webview,
        "get_experiment",
        json!({ "experiment_id": "ghost" }),
    )
    .expect_err("unknown experiments are refused");
    assert!(
        error.to_string().contains("ghost"),
        "the refusal must name the experiment: {error}"
    );
}

/// Snake_case argument keys resolve; camelCase keys do not.
///
/// The commands declare `rename_all = "snake_case"`, so Tauri matches
/// `experiment_id` exactly. The frontend once sent `experimentId`, which is
/// a missing argument at runtime — a second green-suite/red-app gap of the
/// same shape as the state one. This pins the contract in both directions.
#[test]
fn argument_keys_are_snake_case_as_the_commands_declare() {
    let (_app, webview) = test_app("arg-case");
    let ok_body = invoke(
        &webview,
        "get_experiment",
        json!({ "experiment_id": "ghost" }),
    );
    assert!(
        ok_body.is_err(),
        "snake_case reaches the command body (and is refused there, naming the experiment)"
    );
    let camel = invoke(
        &webview,
        "get_experiment",
        json!({ "experimentId": "ghost" }),
    );
    assert!(
        camel.is_err(),
        "camelCase must not resolve: it would silently become a different contract"
    );
    // And the two failures differ: the body names the experiment, the
    // argument layer cannot.
    let snake_error = ok_body.expect_err("snake_case reaches the body");
    assert!(snake_error.to_string().contains("ghost"));
}

/// The arena read path resolves: empty arena reads as null, unknown matches
/// are refused by name.
#[test]
fn arena_reads_resolve_through_the_managed_state() {
    let (_app, webview) = test_app("arena-reads");
    let latest = ok(invoke(&webview, "latest_arena_match", json!({})).expect("latest resolves"));
    assert_eq!(latest, serde_json::Value::Null);
    let error = invoke(
        &webview,
        "replay_arena_match",
        json!({ "match_id": "ghost" }),
    )
    .expect_err("unknown matches are refused");
    assert!(error.to_string().contains("ghost"));
}

/// Discovery and the task catalog resolve with the shapes the panels render.
#[test]
fn readiness_and_task_catalogs_resolve() {
    let (_app, webview) = test_app("catalogs");
    let executors = ok(invoke(&webview, "list_executors", json!({})).expect("executors resolve"));
    assert_eq!(executors.as_array().expect("array").len(), 9);
    let benchmarks =
        ok(invoke(&webview, "list_task_benchmarks", json!({})).expect("benchmarks resolve"));
    assert_eq!(benchmarks.as_array().expect("array").len(), 2);
}

/// Cancelling a run that does not exist reports false rather than failing.
#[test]
fn cancelling_an_unknown_run_reports_false() {
    let (_app, webview) = test_app("cancel-ghost");
    let value = ok(invoke(
        &webview,
        "cancel_experiment",
        json!({ "experiment_id": "ghost" }),
    )
    .expect("cancel resolves"));
    assert_eq!(value, json!(false));
}

/// A reference task evaluation runs natively and reports an observed run.
#[test]
fn reference_task_evaluation_runs_natively() {
    let (_app, webview) = test_app("task-eval");
    let value = ok(invoke(
        &webview,
        "run_task_eval",
        json!({
            "request": {
                "benchmarkId": "coding-repair",
                "policy": "reference-optimal",
                "seeds": [2000000],
                "timeoutSeconds": 30,
            }
        }),
    )
    .expect("task evaluation runs"));
    assert_eq!(value["successes"], json!(1));
    assert_eq!(value["provenance"], json!("observed_run"));
}

/// CLI-agent task policies are refused natively with the reason attached.
#[test]
fn cli_task_policies_are_refused_natively() {
    let (_app, webview) = test_app("cli-refused");
    let error = invoke(
        &webview,
        "run_task_eval",
        json!({
            "request": {
                "benchmarkId": "coding-repair",
                "policy": "cli:codex-cli",
                "seeds": [2000000],
                "timeoutSeconds": 30,
            }
        }),
    )
    .expect_err("CLI policies are refused");
    assert!(
        error.to_string().contains("guided package"),
        "the refusal must direct to the supported path: {error}"
    );
}

/// Read, list, and export over a real measured run, all natively.
///
/// The seed is a genuine one-episode `LabRunner` measurement stored through
/// the same surface the product writes, so `get_experiment`, `list_experiments`,
/// and `export_experiment` assert over evidence rather than fixtures — with
/// every step crossing real Tauri state resolution and argument matching.
#[test]
fn experiment_read_list_export_round_trip_natively() {
    let (_app, webview) = test_app_with_seed("experiment-round-trip", true);

    let read_back = ok(invoke(
        &webview,
        "get_experiment",
        json!({ "experiment_id": "native-seed-exp" }),
    )
    .expect("the measured experiment reads back"));
    assert_eq!(
        read_back["experiment"]["experimentId"],
        json!("native-seed-exp")
    );
    assert_eq!(read_back["experiment"]["episodesCompleted"], json!(1));

    let listed = ok(invoke(&webview, "list_experiments", json!({})).expect("history lists"));
    assert!(
        listed
            .as_array()
            .expect("array")
            .iter()
            .any(|id| id == &json!("native-seed-exp")),
        "the measured run is in the history: {listed}"
    );

    let exported = ok(invoke(
        &webview,
        "export_experiment",
        json!({ "experiment_id": "native-seed-exp", "format": "csv" }),
    )
    .expect("the measured experiment exports"));
    assert!(
        exported["contents"]
            .as_str()
            .expect("csv text")
            .contains("episode"),
        "the export carries per-episode rows"
    );
}
