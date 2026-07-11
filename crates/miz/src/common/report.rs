//! Structured transaction plan + confirmation inversion. Core builds the plan
//! and asks a `Confirmer`; the bin supplies a TTY confirmer, a daemon a policy
//! one. Fleshed out in a later phase.

// wired in a later phase
#[allow(dead_code)]
pub struct TransactionPlan {
    pub targets: Vec<(String, String)>,
}

// wired in a later phase
#[allow(dead_code)]
pub trait Confirmer {
    fn confirm(&mut self, plan: &TransactionPlan) -> bool;
}
