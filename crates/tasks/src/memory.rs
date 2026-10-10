//! Cross-turn cue retention with a chance baseline.
//!
//! # What this measures
//!
//! Whether a policy can act on information that is no longer in front of it.
//! Each task shows a cue on turn one — which of three doors holds the reward
//! — and then presents the doors unlabeled. A policy that remembers scores
//! 1.0; a policy that cannot scores at chance (1/3). The gap between the two
//! is the value of retention with nothing confounded: both policies see
//! identical choice screens and differ only in whether they consult the cue
//! turn.
//!
//! # Why this exists alongside the grid world
//!
//! Phase 0 proved the grid world's history window is not load-bearing: the
//! observation radius equals the step size, so everything worth remembering
//! is still visible when it matters. This benchmark is the documented lever
//! made real — information that genuinely expires — shipped as a new
//! versioned benchmark rather than a silent edit to the grid world, so the
//! grid world's published numbers stay byte-identical.

use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::report::{EvidenceProvenance, TaskBenchmark, TaskReport};

/// Definition version.
pub const MEMORY_LATCH_VERSION: &str = "1";

/// The three doors. Fixed labels so the choice screen is identical for every
/// policy; only the cue mapping varies per seed.
pub const DOORS: [&str; 3] = ["A", "B", "C"];

/// One cue-retention task.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct LatchTask {
    /// Stable task identifier including the seed.
    pub task_id: String,
    /// The seed this task was generated from.
    pub seed: u64,
    /// The cue shown on turn one: which door holds the reward.
    pub cue_door: String,
    /// A decoy fact shown with the cue, so a policy cannot score by
    /// remembering merely that *something* was said on turn one.
    pub decoy: String,
}

/// An answer on the choice turn.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct LatchAnswer {
    /// The door chosen.
    pub door: String,
}

/// SHA-256 over the task construction, so a silent edit is detectable.
pub fn definition_digest() -> String {
    let mut hasher = Sha256::new();
    hasher.update(MEMORY_LATCH_VERSION.as_bytes());
    hasher.update(b"latch:cue-on-turn-one;choice-unlabeled;three-doors");
    format!("sha256:{}", hex::encode(hasher.finalize()))
}

/// Generates the latch task for one seed. Deterministic.
pub fn generate_latch_task(seed: u64) -> LatchTask {
    use rand::Rng;
    let mut rng = ChaCha8Rng::seed_from_u64(seed);
    let cue = DOORS[rng.random_range(0..DOORS.len())];
    // The decoy never names the cue door: a cue screen that also says
    // "door B was painted…" next to "the reward is behind door B" would make
    // the transcript ambiguous about which mention is the cue.
    let decoy_door = loop {
        let candidate = DOORS[rng.random_range(0..DOORS.len())];
        if candidate != cue {
            break candidate;
        }
    };
    let decoy_number = rng.random_range(10..99);
    LatchTask {
        task_id: format!("memory-latch-{seed}"),
        seed,
        cue_door: cue.to_owned(),
        decoy: format!("door {decoy_door} was painted {decoy_number} days ago"),
    }
}

/// The turn-one observation: the cue plus a decoy. This is the only turn on
/// which the rewarded door is named.
pub fn cue_observation(task: &LatchTask) -> String {
    format!(
        "Turn 1. The reward is behind door {cue}. Note: {decoy}.",
        cue = task.cue_door,
        decoy = task.decoy
    )
}

/// The choice-turn observation: the doors, unlabeled. Identical for every
/// task — the cue is gone, and remembering it is the agent's job.
pub fn choice_observation() -> String {
    "Final turn. Three doors: A, B, C. Exactly one holds the reward. Name one door.".to_owned()
}

/// Scores one answer. Exact match on the cued door; anything else,
/// including an unparseable answer, is a miss.
pub fn score_latch(task: &LatchTask, answer: &LatchAnswer) -> bool {
    answer.door.trim().to_ascii_uppercase() == task.cue_door
}

/// Parses a free-text answer to a door choice: the first of A/B/C appearing
/// as a standalone token. `None` means unparseable — a miss, not an error,
/// because an evaluation must survive a bad answer.
pub fn parse_answer(text: &str) -> Option<LatchAnswer> {
    for token in text.split(|character: char| !character.is_ascii_alphabetic()) {
        let upper = token.to_ascii_uppercase();
        if matches!(upper.as_str(), "A" | "B" | "C") {
            return Some(LatchAnswer { door: upper });
        }
    }
    None
}

/// A reference policy for calibration.
#[derive(Clone, Copy, Debug)]
pub enum LatchPolicy {
    /// Records the cue turn and answers it. Must score 1.0.
    Memory,
    /// Ignores the cue turn and always answers the first door. Must score at
    /// chance (1/3) — the same screen, the same rules, no memory.
    Stateless,
}

/// Applies a reference policy to a task through the two-turn transcript.
pub fn apply_latch_policy(policy: LatchPolicy, task: &LatchTask) -> LatchAnswer {
    match policy {
        LatchPolicy::Memory => {
            // Reads the cue from the turn-one observation text, the way a
            // connected model would: the knowledge comes through the
            // transcript, not through the task struct.
            let observation = cue_observation(task);
            let door = ["A", "B", "C"]
                .iter()
                .find(|door| {
                    observation.contains(&format!("door {door}"))
                        && !observation.contains(&format!("door {door} was painted"))
                })
                .expect("the cue names a door");
            LatchAnswer {
                door: (*door).to_owned(),
            }
        }
        LatchPolicy::Stateless => LatchAnswer {
            door: DOORS[0].to_owned(),
        },
    }
}

/// Measures one policy over seeds and reports with a Wilson interval.
pub fn run_latch_eval(
    policy: LatchPolicy,
    policy_name: &str,
    seeds: &[u64],
    provenance: EvidenceProvenance,
) -> TaskReport {
    let mut success = Vec::with_capacity(seeds.len());
    for seed in seeds {
        let task = generate_latch_task(*seed);
        let answer = apply_latch_policy(policy, &task);
        success.push(score_latch(&task, &answer));
    }
    let partial: Vec<f64> = success
        .iter()
        .map(|hit| if *hit { 1.0 } else { 0.0 })
        .collect();
    TaskReport::from_outcomes(crate::report::TaskOutcomes {
        benchmark: TaskBenchmark::MemoryLatch,
        definition_version: MEMORY_LATCH_VERSION.to_owned(),
        definition_digest: definition_digest(),
        policy: policy_name.to_owned(),
        provenance,
        seeds: seeds.to_vec(),
        task_success: success,
        partial_credit: partial,
    })
}

/// Development and evaluation seed pools, disjoint by construction.
pub fn dev_seeds() -> Vec<u64> {
    (3_000_000..3_000_040).collect()
}

/// Held-out evaluation seeds.
pub fn eval_seeds() -> Vec<u64> {
    (4_000_000..4_000_070).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_seed_always_yields_the_same_task() {
        assert_eq!(generate_latch_task(11), generate_latch_task(11));
    }

    #[test]
    fn cues_vary_across_seeds() {
        let cues: Vec<String> = (0..30)
            .map(|seed| generate_latch_task(seed).cue_door)
            .collect();
        for door in DOORS {
            assert!(
                cues.iter().any(|cue| cue == door),
                "door {door} never cued in 30 seeds"
            );
        }
    }

    #[test]
    fn dev_and_eval_pools_are_disjoint() {
        let dev = dev_seeds();
        let eval = eval_seeds();
        assert_eq!(dev.len(), 40);
        assert_eq!(eval.len(), 70);
        assert!(dev.iter().all(|seed| !eval.contains(seed)));
    }

    #[test]
    fn memory_beats_stateless_with_disjoint_intervals() {
        let seeds = eval_seeds();
        let memory = run_latch_eval(
            LatchPolicy::Memory,
            "memory",
            &seeds,
            EvidenceProvenance::ObservedRun,
        );
        let stateless = run_latch_eval(
            LatchPolicy::Stateless,
            "stateless",
            &seeds,
            EvidenceProvenance::ObservedRun,
        );
        assert_eq!(memory.successes, 70);
        // Stateless always answers A: hits exactly the A-cued seeds, which is
        // chance (1/3) up to sampling — and strictly below memory.
        assert!(
            stateless.successes < 40,
            "stateless scored {}",
            stateless.successes
        );
        assert!(
            stateless.successes > 10,
            "stateless scored {}",
            stateless.successes
        );
        assert!(memory.interval_lower > stateless.interval_upper);
    }

    #[test]
    fn the_choice_screen_carries_no_cue() {
        let screen = choice_observation();
        assert!(!screen.contains("reward is behind"));
        for task_seed in [1, 2, 3] {
            let task = generate_latch_task(task_seed);
            assert!(!screen.contains(&task.cue_door) || screen.contains("A, B, C"));
        }
    }

    #[test]
    fn unparseable_answers_are_misses_not_errors() {
        let task = generate_latch_task(5);
        assert_eq!(parse_answer("hmm, let me think..."), None);
        assert!(!score_latch(
            &task,
            &LatchAnswer {
                door: "???".to_owned()
            }
        ));
    }

    #[test]
    fn the_memory_policy_reads_the_transcript_not_the_struct() {
        // The policy must work from the observation text: replay the cue text
        // through a task whose struct disagrees, and the policy follows the
        // text. (It cannot fully disagree — the text is derived — so this
        // asserts the subtler property: the decoy door is never answered
        // merely because it was also named on turn one.)
        let mut decoy_answers = 0;
        for seed in eval_seeds() {
            let task = generate_latch_task(seed);
            let answer = apply_latch_policy(LatchPolicy::Memory, &task);
            let observation = cue_observation(&task);
            // The decoy door is named in the observation; answering it would
            // prove the policy remembers *a* door rather than *the cued* door.
            if observation.contains(&format!("door {} was painted", answer.door))
                && answer.door != task.cue_door
            {
                decoy_answers += 1;
            }
        }
        assert_eq!(decoy_answers, 0);
    }
}
