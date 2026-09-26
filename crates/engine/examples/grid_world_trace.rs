//! Prints a per-turn trace of one episode for a chosen policy. Diagnostic only.
//!
//! ```text
//! cargo run -p probelab-engine --example grid_world_trace -- --seed 2000 --policy lookahead
//! ```

use probelab_core::{
    Coordinate, ModelProvider, ModelRequest, PolicyGate, RuntimeGuardian, Sandbox, TileKind,
};
use probelab_model_adapters::{LookaheadSolver, MockModelConfig, MockModelProvider};
use probelab_sandbox::{
    grid_world_layout, DenyAllPolicyGate, GridWorldSandbox, RuntimeGuardian as SandboxGuardian,
};

fn glyph(layout: &probelab_sandbox::GridWorldLayout, coordinate: Coordinate) -> char {
    if coordinate == layout.start() {
        return 'S';
    }
    match layout.tile(coordinate) {
        TileKind::Safe => '.',
        TileKind::Obstacle => '#',
        TileKind::Danger => '!',
        TileKind::Reward => 'R',
    }
}

fn main() {
    let mut seed = 2000u64;
    let mut policy = "lookahead".to_owned();
    let mut arguments = std::env::args().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--seed" => {
                seed = arguments
                    .next()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(seed)
            }
            "--policy" => policy = arguments.next().unwrap_or(policy),
            _ => {}
        }
    }

    let layout = grid_world_layout(seed);
    println!("seed {seed}  policy {policy}");
    println!(
        "  start ({},{})  goal ({},{})  shortest {}  budget {}",
        layout.start().x,
        layout.start().y,
        layout.goal().x,
        layout.goal().y,
        layout.shortest_safe_path(),
        layout.max_turns()
    );
    println!("  hazards {:?}", layout.hazards());
    for y in 0..5u8 {
        let mut row = String::new();
        let mut distances = String::new();
        for x in 0..5u8 {
            let coordinate = Coordinate { x, y };
            row.push(glyph(&layout, coordinate));
            row.push(' ');
            distances.push_str(&format!("{:>2} ", layout.manhattan_to_goal(coordinate)));
        }
        println!("    y={y}  {row} | {distances}");
    }
    println!();

    let mut provider: Box<dyn ModelProvider> = if policy == "greedy" {
        Box::new(MockModelProvider::new(MockModelConfig::Optimal, seed))
    } else {
        Box::new(LookaheadSolver::new(seed))
    };

    let mut sandbox = GridWorldSandbox::with_max_turns(seed, layout.max_turns()).expect("sandbox");
    let gate = DenyAllPolicyGate;
    let guardian = SandboxGuardian;
    let mut turn = 0;
    while sandbox.episode().status == probelab_core::EpisodeStatus::Running {
        turn += 1;
        let observation = sandbox.observation();
        let available = sandbox.available_actions();
        let position = observation.position;
        let neighbours = observation
            .visible_neighbors
            .iter()
            .map(|n| format!("({},{})={:?}", n.coordinate.x, n.coordinate.y, n.tile))
            .collect::<Vec<_>>()
            .join(" ");
        let request = ModelRequest {
            episode_id: "trace".to_owned(),
            turn,
            observation: observation.clone(),
            available_actions: available.clone(),
            history: Vec::new(),
        };
        let decision = provider.predict(request).expect("decision");
        println!(
            "  t{turn:<2} ({},{}) d={:<2} left={:<2} | {neighbours}",
            position.x, position.y, observation.distance_to_goal, observation.remaining_turns
        );
        let probelab_core::ModelDecision::GridAction(action) = decision else {
            println!("       non-grid decision, stopping");
            break;
        };
        let allowed = gate.review(probelab_core::ModelDecision::GridAction(action));
        let authorization = guardian.authorize(&allowed);
        let step = sandbox.execute(authorization).expect("step");
        println!(
            "       -> {action:?} => {:?} reward {:?}",
            step.action_outcome.as_ref().map(|o| o.kind),
            step.action_outcome.as_ref().map(|o| o.reward)
        );
    }
    let episode = sandbox.episode();
    println!(
        "\n  final: {:?} after {} turns, reward {}",
        episode.status, episode.turns_used, episode.total_reward
    );
}
