use serde::Serialize;
use std::collections::HashSet;

#[derive(Clone, Copy)]
enum EffectKind {
    Read,
    Increment,
}

#[derive(Default)]
struct ExternalSystem {
    value: usize,
    attempts: usize,
    applied_writes: usize,
    applied_ids: HashSet<&'static str>,
}

impl ExternalSystem {
    fn execute(&mut self, kind: EffectKind) {
        self.attempts += 1;
        match kind {
            EffectKind::Read => {}
            EffectKind::Increment => {
                self.value += 1;
                self.applied_writes += 1;
            }
        }
    }

    fn execute_idempotent_increment(&mut self, call_id: &'static str) {
        self.attempts += 1;
        if self.applied_ids.insert(call_id) {
            self.value += 1;
            self.applied_writes += 1;
        }
    }
}

#[derive(Debug, PartialEq, Serialize)]
struct ExperimentResult {
    treatment: &'static str,
    effect: &'static str,
    completed: bool,
    outcome_known: bool,
    effect_attempts: usize,
    applied_writes: usize,
    external_value: usize,
    verifier_passed: bool,
}

fn naive_replay(kind: EffectKind) -> ExperimentResult {
    let mut external = ExternalSystem::default();
    external.execute(kind);
    // Crash after the external effect and before an outcome is retained.
    external.execute(kind);
    result("naive_replay", kind, true, true, external)
}

fn intent_guard(kind: EffectKind) -> ExperimentResult {
    let mut external = ExternalSystem::default();
    // The durable intent exists before this uncertain external effect.
    external.execute(kind);
    // Recovery sees an intent without settlement.
    match kind {
        EffectKind::Read => {
            external.execute(kind);
            result("intent_guard", kind, true, true, external)
        }
        EffectKind::Increment => result("intent_guard", kind, false, false, external),
    }
}

fn idempotent_replay() -> ExperimentResult {
    let mut external = ExternalSystem::default();
    external.execute_idempotent_increment("call-1");
    // Recovery repeats the same identity after losing the settlement.
    external.execute_idempotent_increment("call-1");
    result(
        "idempotent_replay",
        EffectKind::Increment,
        true,
        true,
        external,
    )
}

fn result(
    treatment: &'static str,
    kind: EffectKind,
    completed: bool,
    outcome_known: bool,
    external: ExternalSystem,
) -> ExperimentResult {
    let (effect, verifier_passed) = match kind {
        EffectKind::Read => ("read", external.value == 0),
        EffectKind::Increment => ("increment", completed && external.value == 1),
    };
    ExperimentResult {
        treatment,
        effect,
        completed,
        outcome_known,
        effect_attempts: external.attempts,
        applied_writes: external.applied_writes,
        external_value: external.value,
        verifier_passed,
    }
}

#[test]
fn compares_recovery_after_uncertain_effect() {
    let results = [
        naive_replay(EffectKind::Read),
        naive_replay(EffectKind::Increment),
        intent_guard(EffectKind::Read),
        intent_guard(EffectKind::Increment),
        idempotent_replay(),
    ];

    println!("{}", serde_json::to_string_pretty(&results).unwrap());
    assert_eq!(
        results,
        [
            ExperimentResult {
                treatment: "naive_replay",
                effect: "read",
                completed: true,
                outcome_known: true,
                effect_attempts: 2,
                applied_writes: 0,
                external_value: 0,
                verifier_passed: true,
            },
            ExperimentResult {
                treatment: "naive_replay",
                effect: "increment",
                completed: true,
                outcome_known: true,
                effect_attempts: 2,
                applied_writes: 2,
                external_value: 2,
                verifier_passed: false,
            },
            ExperimentResult {
                treatment: "intent_guard",
                effect: "read",
                completed: true,
                outcome_known: true,
                effect_attempts: 2,
                applied_writes: 0,
                external_value: 0,
                verifier_passed: true,
            },
            ExperimentResult {
                treatment: "intent_guard",
                effect: "increment",
                completed: false,
                outcome_known: false,
                effect_attempts: 1,
                applied_writes: 1,
                external_value: 1,
                verifier_passed: false,
            },
            ExperimentResult {
                treatment: "idempotent_replay",
                effect: "increment",
                completed: true,
                outcome_known: true,
                effect_attempts: 2,
                applied_writes: 1,
                external_value: 1,
                verifier_passed: true,
            },
        ]
    );
}
