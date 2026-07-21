//! ## Declared roles
//!
//! `filter`, `mapper`, `orchestration`, `predicate`
//!
//! Ownership-aware resume resolution for ambiguous provider-native IDs.

use super::dtos::{
    ResumeServiceOutput, ResumeServiceRejection, ResumeServiceRequest, ResumeStorageFailure,
};
use crate::session_metadata::{SessionOwnership, resolve_session_ownership};
use oulipoly_state::repositories::ResumeRepository;
use oulipoly_state::{ResumeInputMatch, ResumeNativeCandidate, StateDb};
use std::collections::{BTreeMap, BTreeSet};

struct ProviderOwnershipProbe {
    provider_name: String,
    chain_ids: BTreeSet<String>,
    ownership: SessionOwnership,
}

struct OwnershipProbeSummary {
    owners: BTreeMap<String, BTreeSet<String>>,
    failures: Vec<ResumeStorageFailure>,
}

enum OwnershipFold {
    Selected {
        chain_id: String,
    },
    OwnerNotFound,
    OwnershipAmbiguous {
        owners: BTreeMap<String, BTreeSet<String>>,
    },
    OwnershipIndeterminate {
        failures: Vec<ResumeStorageFailure>,
    },
    OwnerChainAmbiguous {
        provider_name: String,
        chain_ids: Vec<String>,
    },
}

pub(super) fn resolve_resume(request: ResumeServiceRequest<'_>) -> ResumeServiceOutput {
    let input_match =
        match <StateDb as ResumeRepository>::classify_resume_input(request.state, request.input) {
            Ok(input_match) => input_match,
            Err(error) => return rejected(ResumeServiceRejection::State(error)),
        };
    match input_match {
        ResumeInputMatch::ExactChain { chain_id } => finalize_chain(&request, &chain_id),
        ResumeInputMatch::NativeSession { candidates } => {
            resolve_native_candidates(request, candidates)
        }
    }
}

fn resolve_native_candidates(
    request: ResumeServiceRequest<'_>,
    candidates: Vec<ResumeNativeCandidate>,
) -> ResumeServiceOutput {
    let distinct_chains = distinct_candidate_chain_ids(&candidates);
    if let Some(chain_id) = single_candidate_chain_id(&distinct_chains) {
        return finalize_chain(&request, chain_id);
    }

    let providers = candidate_provider_chains(&candidates);
    let probes = probe_candidate_providers(&request, &providers);
    let summary = partition_probe_outcomes(probes);
    match fold_ownership(summary) {
        OwnershipFold::Selected { chain_id } => finalize_chain(&request, &chain_id),
        fold => rejected(map_ownership_rejection(request.input, candidates, fold)),
    }
}

fn candidate_chain_ids(candidates: &[ResumeNativeCandidate]) -> Vec<String> {
    candidates
        .iter()
        .map(|candidate| candidate.chain_id.clone())
        .collect()
}

fn sort_and_deduplicate_chain_ids(mut chain_ids: Vec<String>) -> Vec<String> {
    chain_ids.sort();
    chain_ids.dedup();
    chain_ids
}

fn distinct_candidate_chain_ids(candidates: &[ResumeNativeCandidate]) -> Vec<String> {
    sort_and_deduplicate_chain_ids(candidate_chain_ids(candidates))
}

fn single_candidate_chain_id(chain_ids: &[String]) -> Option<&str> {
    match chain_ids {
        [chain_id] => Some(chain_id),
        _ => None,
    }
}

fn candidate_provider_chains(
    candidates: &[ResumeNativeCandidate],
) -> BTreeMap<String, BTreeSet<String>> {
    let mut providers = BTreeMap::<String, BTreeSet<String>>::new();
    for candidate in candidates {
        providers
            .entry(candidate.matching_provider.clone())
            .or_default()
            .insert(candidate.chain_id.clone());
    }
    providers
}

fn probe_candidate_providers(
    request: &ResumeServiceRequest<'_>,
    providers: &BTreeMap<String, BTreeSet<String>>,
) -> Vec<ProviderOwnershipProbe> {
    providers
        .iter()
        .map(|(provider_name, chain_ids)| {
            let ownership = probe_candidate_provider(request, provider_name);
            map_provider_ownership_probe(provider_name, chain_ids, ownership)
        })
        .collect()
}

fn probe_candidate_provider(
    request: &ResumeServiceRequest<'_>,
    provider_name: &str,
) -> SessionOwnership {
    match request.providers_cfg.runtime_provider(provider_name) {
        Ok((provider, _)) => {
            resolve_session_ownership(provider.session_storage.as_ref(), request.input)
        }
        Err(reason) => SessionOwnership::Indeterminate(reason),
    }
}

fn map_provider_ownership_probe(
    provider_name: &str,
    chain_ids: &BTreeSet<String>,
    ownership: SessionOwnership,
) -> ProviderOwnershipProbe {
    ProviderOwnershipProbe {
        provider_name: provider_name.to_string(),
        chain_ids: chain_ids.clone(),
        ownership,
    }
}

fn partition_probe_outcomes(probes: Vec<ProviderOwnershipProbe>) -> OwnershipProbeSummary {
    let mut owners = BTreeMap::new();
    let mut failures = Vec::new();
    for probe in probes {
        match probe.ownership {
            SessionOwnership::Owned => {
                owners.insert(probe.provider_name, probe.chain_ids);
            }
            SessionOwnership::NotOwned => {}
            SessionOwnership::Indeterminate(reason) => {
                failures.push(map_storage_failure(probe.provider_name, reason));
            }
        }
    }
    OwnershipProbeSummary { owners, failures }
}

fn map_storage_failure(provider_name: String, reason: String) -> ResumeStorageFailure {
    ResumeStorageFailure {
        provider_name,
        reason,
    }
}

fn fold_ownership(summary: OwnershipProbeSummary) -> OwnershipFold {
    if summary.owners.len() > 1 {
        return OwnershipFold::OwnershipAmbiguous {
            owners: summary.owners,
        };
    }
    if !summary.failures.is_empty() {
        return OwnershipFold::OwnershipIndeterminate {
            failures: summary.failures,
        };
    }
    let Some((provider_name, chain_ids)) = summary.owners.into_iter().next() else {
        return OwnershipFold::OwnerNotFound;
    };
    if chain_ids.len() > 1 {
        return OwnershipFold::OwnerChainAmbiguous {
            provider_name,
            chain_ids: chain_ids.into_iter().collect(),
        };
    }
    OwnershipFold::Selected {
        chain_id: chain_ids
            .into_iter()
            .next()
            .expect("one owning provider chain must exist"),
    }
}

fn map_ownership_rejection(
    input: &str,
    candidates: Vec<ResumeNativeCandidate>,
    fold: OwnershipFold,
) -> ResumeServiceRejection {
    match fold {
        OwnershipFold::OwnerNotFound => ResumeServiceRejection::StorageOwnerNotFound {
            input: input.to_string(),
            candidates,
        },
        OwnershipFold::OwnershipAmbiguous { owners } => {
            ResumeServiceRejection::StorageOwnershipAmbiguous {
                input: input.to_string(),
                owners: owner_candidates(&owners),
            }
        }
        OwnershipFold::OwnershipIndeterminate { failures } => {
            ResumeServiceRejection::StorageOwnershipIndeterminate {
                input: input.to_string(),
                failures,
            }
        }
        OwnershipFold::OwnerChainAmbiguous {
            provider_name,
            chain_ids,
        } => ResumeServiceRejection::StorageOwnerChainAmbiguous {
            input: input.to_string(),
            provider_name,
            chain_ids,
        },
        OwnershipFold::Selected { .. } => {
            unreachable!("selected ownership folds are finalized before rejection mapping")
        }
    }
}

fn owner_candidates(owners: &BTreeMap<String, BTreeSet<String>>) -> Vec<ResumeNativeCandidate> {
    owners
        .iter()
        .flat_map(|(provider_name, chain_ids)| {
            chain_ids.iter().map(|chain_id| ResumeNativeCandidate {
                chain_id: chain_id.clone(),
                matching_provider: provider_name.clone(),
            })
        })
        .collect()
}

fn finalize_chain(request: &ResumeServiceRequest<'_>, chain_id: &str) -> ResumeServiceOutput {
    match <StateDb as ResumeRepository>::resolve_resume_chain(
        request.state,
        request.models,
        chain_id,
        request.model_override,
    ) {
        Ok(resolved) => ResumeServiceOutput::ResumeResolved { resolved },
        Err(error) => rejected(ResumeServiceRejection::State(error)),
    }
}

fn rejected(error: ResumeServiceRejection) -> ResumeServiceOutput {
    ResumeServiceOutput::ResumeRejected { error }
}
