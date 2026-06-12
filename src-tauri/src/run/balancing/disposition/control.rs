//! Declared roles: orchestration

pub(in crate::run::balancing) enum BalancedLoopControl {
    Continue,
    Return(Result<i32, String>),
}
