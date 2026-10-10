//! Seed-varying single-bug repair tasks with hidden static checks.
//!
//! # What this measures
//!
//! Whether a policy can repair a small broken program. Each task carries one
//! bug from a fixed corpus; identifiers and numeric values derive from the
//! seed, so a memorized patch does not transfer across seeds. The evaluator
//! reads the artifact the policy left behind and applies hidden checks the
//! policy never sees: the fix pattern must be present, the broken pattern
//! must be gone, delimiters must balance, and the task's identifiers must be
//! preserved (so deleting the function cannot score).
//!
//! # What it does not measure
//!
//! Dynamic behaviour. Nothing is executed: verification is static by design
//! (see [`verification_limits`]). A patch that names the fix but breaks the
//! program elsewhere can still score, and that limit is stated rather than
//! hidden. Dynamic execution inside containment is the documented next step,
//! not a silent property of this version.

use std::collections::BTreeMap;

use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::report::{EvidenceProvenance, TaskBenchmark, TaskReport};

/// Definition version. Bump when the corpus changes; the digest pins it.
pub const CODING_REPAIR_VERSION: &str = "1";

/// One repair task.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct RepairTask {
    /// Stable task identifier including the seed.
    pub task_id: String,
    /// The seed this task was generated from.
    pub seed: u64,
    /// Human description of the required fix (given to the policy).
    pub description: String,
    /// The file name the policy must repair.
    pub file_name: String,
    /// The broken program (given to the policy).
    pub broken: String,
    /// The reference fix (never given to the policy; used by the evaluator
    /// only to derive the hidden checks).
    pub reference_fix: String,
}

/// One hidden check and whether it passed.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct RepairCheck {
    /// Stable check name.
    pub name: String,
    /// Whether it passed.
    pub passed: bool,
}

/// The evaluator's verdict on one artifact.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct RepairScore {
    /// Whether every check passed.
    pub fixed: bool,
    /// Partial credit: fraction of checks passed, 0.0–1.0.
    pub partial_credit: f64,
    /// The individual checks.
    pub checks: Vec<RepairCheck>,
}

/// What static verification can and cannot establish.
pub fn verification_limits() -> Vec<String> {
    vec![
        "static checks only: no code is executed".to_owned(),
        "a patch that names the fix but breaks behaviour elsewhere can still score".to_owned(),
        "single-bug tasks: no multi-file or multi-step repairs".to_owned(),
        "Python snippets only in v1".to_owned(),
    ]
}

/// A repair pattern: one bug class with seed-varying slots.
///
/// `{FN}` is the function name, `{N}`/`{M}` are seed-varying integers. The
/// broken and fixed sources differ only in the bug, so the checks compare
/// exactly the thing the task claims to measure.
struct Pattern {
    name: &'static str,
    description: &'static str,
    broken: &'static str,
    fixed: &'static str,
    /// Fragment present only in the broken source.
    broken_marker: &'static str,
    /// Fragment present only in the fixed source.
    fixed_marker: &'static str,
}

const PATTERNS: [Pattern; 8] = [
    Pattern {
        name: "off_by_one",
        description: "the loop must include N itself",
        broken: "def {FN}(n):\n    total = 0\n    for i in range(n):\n        total += i\n    return total\n",
        fixed: "def {FN}(n):\n    total = 0\n    for i in range(n + 1):\n        total += i\n    return total\n",
        broken_marker: "range(n):",
        fixed_marker: "range(n + 1):",
    },
    Pattern {
        name: "wrong_comparison",
        description: "the boundary value itself must pass",
        broken: "def {FN}(x):\n    if x > {N}:\n        return True\n    return False\n",
        fixed: "def {FN}(x):\n    if x >= {N}:\n        return True\n    return False\n",
        broken_marker: "x > {N}:",
        fixed_marker: "x >= {N}:",
    },
    Pattern {
        name: "swapped_return",
        description: "the pair must come back in order",
        broken: "def {FN}(a, b):\n    return b, a\n",
        fixed: "def {FN}(a, b):\n    return a, b\n",
        broken_marker: "return b, a",
        fixed_marker: "return a, b",
    },
    Pattern {
        name: "missing_return",
        description: "the computed value must be returned, not stored and dropped",
        broken: "def {FN}(x):\n    result = x * {N}\n",
        fixed: "def {FN}(x):\n    return x * {N}\n",
        broken_marker: "result = x * {N}",
        fixed_marker: "return x * {N}",
    },
    Pattern {
        name: "wrong_operator",
        description: "the total must multiply, not add",
        broken: "def {FN}(x):\n    return x + {N}\n",
        fixed: "def {FN}(x):\n    return x * {N}\n",
        broken_marker: "return x + {N}",
        fixed_marker: "return x * {N}",
    },
    Pattern {
        name: "wrong_index",
        description: "the last element is wanted, not the first",
        broken: "def {FN}(items):\n    return items[0]\n",
        fixed: "def {FN}(items):\n    return items[-1]\n",
        broken_marker: "items[0]",
        fixed_marker: "items[-1]",
    },
    Pattern {
        name: "and_instead_of_or",
        description: "either condition suffices",
        broken: "def {FN}(x):\n    if x == {N} and x == {M}:\n        return True\n    return False\n",
        fixed: "def {FN}(x):\n    if x == {N} or x == {M}:\n        return True\n    return False\n",
        broken_marker: " and x ==",
        fixed_marker: " or x ==",
    },
    Pattern {
        name: "integer_division",
        description: "the average needs true division",
        broken: "def {FN}(a, b):\n    return (a + b) // {N}\n",
        fixed: "def {FN}(a, b):\n    return (a + b) / {N}\n",
        broken_marker: "(a + b) // {N}",
        fixed_marker: "(a + b) / {N}",
    },
];

const FUNCTION_NAMES: [&str; 8] = [
    "compute",
    "evaluate",
    "transform",
    "aggregate",
    "measure",
    "resolve",
    "combine",
    "reduce",
];

/// SHA-256 over the pattern corpus, so a silent edit is detectable.
pub fn definition_digest() -> String {
    let mut hasher = Sha256::new();
    hasher.update(CODING_REPAIR_VERSION.as_bytes());
    for pattern in &PATTERNS {
        hasher.update(pattern.name.as_bytes());
        hasher.update(pattern.broken.as_bytes());
        hasher.update(pattern.fixed.as_bytes());
    }
    format!("sha256:{}", hex::encode(hasher.finalize()))
}

/// Generates the repair task for one seed. Deterministic: the same seed
/// always yields the same task.
pub fn generate_repair_task(seed: u64) -> RepairTask {
    use rand::Rng;
    let mut rng = ChaCha8Rng::seed_from_u64(seed);
    let pattern = &PATTERNS[(seed as usize) % PATTERNS.len()];
    let function = FUNCTION_NAMES[rng.random_range(0..FUNCTION_NAMES.len())];
    let number = rng.random_range(2..50);
    let other = rng.random_range(51..99);
    let fill = |template: &str| {
        template
            .replace("{FN}", function)
            .replace("{N}", &number.to_string())
            .replace("{M}", &other.to_string())
    };
    RepairTask {
        task_id: format!("coding-repair-{pattern_name}-{seed}", pattern_name = pattern.name),
        seed,
        description: format!(
            "Repair `{function}` in {file}: {what}. Keep the function name and signature unchanged.",
            file = "task.py",
            what = pattern.description,
        ),
        file_name: "task.py".to_owned(),
        broken: fill(pattern.broken),
        reference_fix: fill(pattern.fixed),
    }
}

/// Scores one candidate artifact against the task's hidden checks.
pub fn score_repair(task: &RepairTask, candidate: &str) -> RepairScore {
    let broken_marker = marker_for(task, MarkerKind::Broken);
    let fixed_marker = marker_for(task, MarkerKind::Fixed);
    let function_name = function_name_of(task);
    let checks = vec![
        RepairCheck {
            name: "fix_present".to_owned(),
            passed: candidate.contains(&fixed_marker),
        },
        RepairCheck {
            name: "bug_absent".to_owned(),
            passed: !candidate.contains(&broken_marker),
        },
        RepairCheck {
            name: "delimiters_balanced".to_owned(),
            passed: balanced(candidate),
        },
        RepairCheck {
            name: "identifier_preserved".to_owned(),
            passed: candidate.contains(&format!("def {function_name}(")),
        },
    ];
    // A candidate that merely appends the fix while keeping the bug fails
    // `bug_absent`; a candidate that deletes the function fails
    // `identifier_preserved`. Both orders are partial credit, neither is a fix.
    let passed = checks.iter().filter(|check| check.passed).count();
    RepairScore {
        fixed: passed == checks.len(),
        partial_credit: passed as f64 / checks.len() as f64,
        checks,
    }
}

/// Which side of a pattern's marker table to fill for one task.
#[derive(Clone, Copy)]
enum MarkerKind {
    Broken,
    Fixed,
}

/// Re-derives a marker's task-specific text: the table marker with this
/// task's own numbers substituted. The checks therefore compare the candidate
/// against this task's broken/fixed pair, never a global string — a memorized
/// patch for another seed's numbers does not match.
fn marker_for(task: &RepairTask, kind: MarkerKind) -> String {
    let pattern = PATTERNS
        .iter()
        .find(|pattern| task.task_id.contains(pattern.name))
        .expect("task ids name their pattern");
    let template = match kind {
        MarkerKind::Broken => pattern.broken_marker,
        MarkerKind::Fixed => pattern.fixed_marker,
    };
    let regenerated = generate_repair_task(task.seed);
    // The table marker with this task's numbers substituted: extract the
    // numbers from the regenerated broken source in order. Markers contain at
    // most the {N} and {M} slots, filled in order of appearance.
    let numbers: Vec<String> = regenerated
        .broken
        .lines()
        .flat_map(|line| {
            line.split(|character: char| !character.is_numeric())
                .filter(|token| !token.is_empty())
                .map(str::to_owned)
                .collect::<Vec<_>>()
        })
        .collect();
    let mut filled = template.to_owned();
    let mut numbers = numbers.into_iter();
    while filled.contains("{N}") {
        filled = filled.replacen("{N}", &numbers.next().unwrap_or_default(), 1);
    }
    while filled.contains("{M}") {
        filled = filled.replacen("{M}", &numbers.next().unwrap_or_default(), 1);
    }
    filled
}

fn function_name_of(task: &RepairTask) -> String {
    task.broken
        .lines()
        .find_map(|line| {
            let rest = line.strip_prefix("def ")?;
            rest.split('(').next().map(str::to_owned)
        })
        .unwrap_or_default()
}

fn balanced(candidate: &str) -> bool {
    let mut stack: Vec<char> = Vec::new();
    let mut in_string: Option<char> = None;
    let mut chars = candidate.chars().peekable();
    while let Some(character) = chars.next() {
        if let Some(quote) = in_string {
            if character == '\\' {
                chars.next();
            } else if character == quote {
                in_string = None;
            }
            continue;
        }
        match character {
            '"' | '\'' => in_string = Some(character),
            '#' => while chars.next().is_some_and(|next| next != '\n') {},
            '(' | '[' | '{' => stack.push(character),
            ')' | ']' | '}' => {
                let open = stack.pop();
                let matches = matches!(
                    (open, character),
                    (Some('('), ')') | (Some('['), ']') | (Some('{'), '}')
                );
                if !matches {
                    return false;
                }
            }
            _ => {}
        }
    }
    stack.is_empty() && in_string.is_none()
}

/// A reference policy for calibration.
#[derive(Clone, Copy, Debug)]
pub enum RepairPolicy {
    /// Returns the reference fix. Must score 1.0 on every seed.
    Optimal,
    /// Returns the broken source unchanged. Must score 0.0 on every seed.
    Noop,
}

/// Applies a reference policy to a task.
pub fn apply_repair_policy(policy: RepairPolicy, task: &RepairTask) -> String {
    match policy {
        RepairPolicy::Optimal => task.reference_fix.clone(),
        RepairPolicy::Noop => task.broken.clone(),
    }
}

/// Measures one policy over seed pools and reports with a Wilson interval.
pub fn run_repair_eval(
    policy: RepairPolicy,
    policy_name: &str,
    seeds: &[u64],
    provenance: EvidenceProvenance,
) -> TaskReport {
    let mut success = Vec::with_capacity(seeds.len());
    let mut partial = Vec::with_capacity(seeds.len());
    for seed in seeds {
        let task = generate_repair_task(*seed);
        let candidate = apply_repair_policy(policy, &task);
        let score = score_repair(&task, &candidate);
        success.push(score.fixed);
        partial.push(score.partial_credit);
    }
    TaskReport::from_outcomes(crate::report::TaskOutcomes {
        benchmark: TaskBenchmark::CodingRepair,
        definition_version: CODING_REPAIR_VERSION.to_owned(),
        definition_digest: definition_digest(),
        policy: policy_name.to_owned(),
        provenance,
        seeds: seeds.to_vec(),
        task_success: success,
        partial_credit: partial,
    })
}

/// Builds the disposable task files for a CLI or guided run.
pub fn task_files(task: &RepairTask) -> BTreeMap<String, String> {
    let mut files = BTreeMap::new();
    files.insert(task.file_name.clone(), task.broken.clone());
    files.insert(
        "INSTRUCTIONS.md".to_owned(),
        format!(
            "# Repair task\n\n{description}\n\nEdit `{file}` in place. Keep the function name and signature unchanged.\n",
            description = task.description,
            file = task.file_name,
        ),
    );
    files
}

/// Scores artifacts left by a CLI agent or a guided submission.
pub fn score_artifacts(task: &RepairTask, artifacts: &BTreeMap<String, String>) -> RepairScore {
    match artifacts.get(&task.file_name) {
        Some(candidate) => score_repair(task, candidate),
        None => RepairScore {
            fixed: false,
            partial_credit: 0.0,
            checks: vec![RepairCheck {
                name: "artifact_present".to_owned(),
                passed: false,
            }],
        },
    }
}

/// Development and evaluation seed pools. Disjoint by construction: distinct
/// ranges, asserted in tests.
pub fn dev_seeds() -> Vec<u64> {
    (1_000_000..1_000_040).collect()
}

/// Held-out evaluation seeds.
pub fn eval_seeds() -> Vec<u64> {
    (2_000_000..2_000_070).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_seed_always_yields_the_same_task() {
        assert_eq!(generate_repair_task(42), generate_repair_task(42));
    }

    #[test]
    fn different_seeds_vary_the_task() {
        let tasks: Vec<RepairTask> = (0..16).map(generate_repair_task).collect();
        let mut broken: Vec<&str> = tasks.iter().map(|task| task.broken.as_str()).collect();
        broken.sort_unstable();
        broken.dedup();
        assert!(broken.len() > 8, "seeds must vary identifiers and values");
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
    fn the_optimal_policy_fixes_every_eval_seed_and_noop_fixes_none() {
        let seeds = eval_seeds();
        let optimal = run_repair_eval(
            RepairPolicy::Optimal,
            "optimal",
            &seeds,
            EvidenceProvenance::ObservedRun,
        );
        let noop = run_repair_eval(
            RepairPolicy::Noop,
            "noop",
            &seeds,
            EvidenceProvenance::ObservedRun,
        );
        assert_eq!(optimal.successes, 70);
        assert_eq!(noop.successes, 0);
        assert!(optimal.interval_lower > noop.interval_upper);
    }

    #[test]
    fn a_fix_that_keeps_the_bug_is_partial_credit_not_a_fix() {
        let task = generate_repair_task(eval_seeds()[0]);
        let both = format!("{}\n{}", task.broken, task.reference_fix);
        let score = score_repair(&task, &both);
        assert!(!score.fixed);
        assert!(score.partial_credit > 0.0 && score.partial_credit < 1.0);
    }

    #[test]
    fn deleting_the_function_is_not_a_fix() {
        let task = generate_repair_task(eval_seeds()[0]);
        let score = score_repair(&task, "# nothing here\n");
        assert!(!score.fixed);
    }

    #[test]
    fn missing_artifacts_score_zero_with_a_named_check() {
        let task = generate_repair_task(7);
        let score = score_artifacts(&task, &BTreeMap::new());
        assert!(!score.fixed);
        assert_eq!(score.partial_credit, 0.0);
    }

    #[test]
    fn the_digest_is_stable_and_versioned() {
        assert_eq!(definition_digest(), definition_digest());
        assert!(definition_digest().starts_with("sha256:"));
    }

    #[test]
    fn agent_facing_materials_carry_no_hidden_answer() {
        // The reference fix must never reach the policy: not in the task
        // files, not in the instructions, and not derivable from the task id.
        // A leak here would turn the benchmark into a transcription test.
        for seed in eval_seeds().iter().take(16) {
            let task = generate_repair_task(*seed);
            let files = task_files(&task);
            for (name, contents) in &files {
                assert!(
                    !contents.contains(&task.reference_fix),
                    "seed {seed}: the reference fix leaks into {name}"
                );
            }
            assert!(
                !task.description.contains(&task.reference_fix),
                "seed {seed}: the reference fix leaks into the description"
            );
        }
    }

    #[test]
    fn agent_facing_materials_name_no_seed_or_pattern() {
        // task_id embeds the seed and pattern name for provenance. Neither
        // may reach the policy, or a policy could key off the task rather
        // than solve it.
        for seed in eval_seeds().iter().take(16) {
            let task = generate_repair_task(*seed);
            let seed_text = seed.to_string();
            let files = task_files(&task);
            for (name, contents) in &files {
                assert!(
                    !contents.contains(&seed_text),
                    "seed {seed}: the seed leaks into {name}"
                );
            }
            assert!(
                !task.description.contains(&seed_text),
                "seed {seed}: the seed leaks into the description"
            );
            for pattern in &PATTERNS {
                assert!(
                    !task.description.contains(pattern.name),
                    "seed {seed}: the pattern name leaks into the description"
                );
            }
        }
    }

    #[test]
    fn a_fix_memorized_from_another_seed_rarely_transfers() {
        // The seed-varying identifiers and values must defeat transcription:
        // scoring another seed's reference fix against this task should fail
        // for the overwhelming majority of pairs. The bound is statistical
        // rather than absolute — two seeds can share a pattern and numbers —
        // so the assertion is a rate, not a universal.
        let seeds = eval_seeds();
        let mut transfers = 0;
        let mut pairs = 0;
        for (index, seed) in seeds.iter().enumerate() {
            let task = generate_repair_task(*seed);
            let other = generate_repair_task(seeds[(index + 17) % seeds.len()]);
            if score_repair(&task, &other.reference_fix).fixed {
                transfers += 1;
            }
            pairs += 1;
        }
        assert!(
            transfers * 20 < pairs,
            "{transfers}/{pairs} cross-seed fixes transferred; identifiers are not varying enough"
        );
    }

    #[test]
    fn a_fix_that_breaks_syntax_is_partial_credit_not_a_fix() {
        // Static verification, honestly bounded: a candidate naming the fix
        // while breaking the program elsewhere scores partial, never fixed.
        // Dynamic behaviour is out of scope for v1 by design.
        let task = generate_repair_task(eval_seeds()[0]);
        let broken_syntax = format!("{}(((((", task.reference_fix);
        let score = score_repair(&task, &broken_syntax);
        assert!(!score.fixed);
        assert!(score.partial_credit > 0.0 && score.partial_credit < 1.0);
    }
}
