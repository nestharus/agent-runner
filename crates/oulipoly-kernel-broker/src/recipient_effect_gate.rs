//! Namespace identity controls only the remaining experimental routes.
//! Ordinary F operations must establish their own existing evidence binding.

pub(super) fn require_route(
    fixture: bool,
    bind: impl FnOnce() -> Result<bool, String>,
) -> Result<(), String> {
    let bound = bind()?;
    if !bound && !fixture {
        return Err("fresh recipient operation outside bound normal path".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nonfixture_bound_recipient_route_is_available() {
        // Production predicate branch: no user-namespace/fixture authorization.
        require_route(false, || Ok(true)).unwrap();
    }

    #[test]
    fn recipient_binding_refusals_survive_both_namespace_branches() {
        for fixture in [false, true] {
            for refusal in [
                "unbound released child",
                "private-prefix D/session",
                "copied old mailbox row",
            ] {
                assert_eq!(
                    require_route(fixture, || Err(refusal.into())).unwrap_err(),
                    refusal
                );
            }
        }
        assert!(require_route(false, || Ok(false)).is_err());
    }
}
