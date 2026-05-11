use crate::usage::row::EnumeratedAccount;
use std::collections::HashSet;

pub(crate) fn filter_routing_pool(accounts: Vec<EnumeratedAccount>) -> Vec<EnumeratedAccount> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for account in accounts {
        if seen.insert(account.account_id.clone()) {
            out.push(account);
        }
    }
    out.sort_by(|a, b| a.account_id.cmp(&b.account_id));
    out
}
