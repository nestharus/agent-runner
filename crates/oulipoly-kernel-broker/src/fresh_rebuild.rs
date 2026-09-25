//! Detached importer for the retained v3 candidate / v4 policy evidence.
//! It only reads legacy artifacts. Unknown older layouts refuse explicitly.
use super::*;
use crate::linux_main::fresh_index::{
    Account, AccountUpdate, Artifact, EffectIntent, EffectKind, Index, MarkerTimes,
    OfflineDecision, OfflineSnapshot, PhysicalQ, ProviderGrant, SourceKey, TerminalMarkerKind,
};
use std::collections::{BTreeMap, HashSet};

fn invalid(message: &'static str) -> io::Error {
    io::Error::other(message)
}
fn artifact(root: &Path, relative: impl AsRef<Path>) -> io::Result<Artifact> {
    Artifact::from_existing(root, relative.as_ref()).map_err(io::Error::other)
}
fn account<'a>(snapshot: &'a mut OfflineSnapshot, key: &str) -> &'a mut Account {
    snapshot
        .accounts
        .entry(key.to_owned())
        .or_insert_with(|| Account {
            generation: String::new(),
            physical_key: key.to_owned(),
            revision: 0,
            grants: BTreeMap::new(),
            effects: BTreeMap::new(),
            observed_invocations: 0,
            markers: MarkerTimes::default(),
            source_q: BTreeMap::new(),
            recent_failure_nanos: Vec::new(),
        })
}
fn sha_text(value: &impl Serialize) -> io::Result<String> {
    Ok(format!("{:x}", Sha256::digest(serde_json::to_vec(value)?)))
}
fn hash_shape(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit())
}
fn source_key(candidate: &RouteCandidate, environment: &str) -> io::Result<SourceKey> {
    if !hash_shape(environment) {
        return Err(invalid("offline effect environment digest invalid"));
    }
    Ok(SourceKey {
        commands_sha256: sha_text(&(&candidate.quota_script, &candidate.auth_refresh_command))?,
        environment_sha256: environment.to_owned(),
    })
}
pub(super) fn validate_candidate(
    root: &Path,
    source: &Path,
    candidate: &RouteCandidate,
) -> io::Result<()> {
    if candidate.version != 3
        || candidate.account.is_empty()
        || candidate.account_identity.is_empty()
        || !hash_shape(&candidate.plan_sha256)
        || !hash_shape(&candidate.environment_sha256)
        || !hash_shape(&candidate.config_sha256)
        || candidate.total == 0
        || candidate.index >= candidate.total
    {
        return Err(invalid("offline candidate format unsupported"));
    }
    let registered: RouteSource = exact_file(
        root,
        &format!("{}.route-source.json", candidate.binding.handoff_id),
    )?
    .ok_or_else(|| invalid("offline route source absent"))?;
    let source_meta = source.metadata()?;
    if !source_meta.is_dir()
        || registered.version != 1
        || registered.binding != candidate.binding
        || registered.config_sha256 != candidate.config_sha256
        || registered.directory_device != source_meta.dev()
        || registered.directory_inode != source_meta.ino()
    {
        return Err(invalid(
            "offline source directory or config identity changed",
        ));
    }
    let pool = oulipoly_runtime::executor::cli::fresh_remote::load_fresh_headless_pool(
        source,
        &candidate.model,
    )
    .map_err(io::Error::other)?;
    let member = pool.model.providers.get(candidate.index);
    let effects = pool.account_effects.get(candidate.index);
    if pool.config_sha256 != candidate.config_sha256
        || pool.model.providers.len() != candidate.total
        || member.map(|m| m.name.as_str()) != Some(candidate.account.as_str())
        || pool.account_identities[candidate.index].as_deref()
            != Some(candidate.account_identity.as_str())
        || effects
            != Some(&(
                candidate.quota_script.clone(),
                candidate.auth_refresh_command.clone(),
            ))
        || member.map(FreshTerminalRecognizer::for_provider)
            != Some(candidate.terminal_recognizer.clone())
    {
        return Err(invalid(
            "offline candidate differs from exact config source",
        ));
    }
    Ok(())
}
fn check_terminal(
    root: &Path,
    decision: &RouteDecision,
    candidate: &RouteCandidate,
    grant: &Grant,
    allow_uncertain_q: bool,
) -> io::Result<Option<(TerminalOutcome, i64)>> {
    let q_name = format!("{}.drain.json", grant.id);
    if !root.join(&q_name).exists() {
        return Ok(None);
    }
    let Observation::Drained {
        status,
        mut stdout,
        mut stderr,
        cancelled,
        ..
    } = observe(root, &grant.id)?
    else {
        return if allow_uncertain_q {
            Ok(None)
        } else {
            Err(invalid(
                "offline provider Q lacks independent physical certification",
            ))
        };
    };
    let Some(terminal): Option<TerminalRecord> =
        exact_file(root, &format!("{}.terminal.json", grant.id))?
    else {
        return if allow_uncertain_q {
            Ok(None)
        } else {
            Err(invalid("offline provider Q terminal record absent"))
        };
    };
    let q_sha = sha_file(&File::open(root.join(&q_name))?)?.0;
    let q_time = file_unix_nanos(&root.join(q_name))?;
    let mut stdout_bytes = Vec::new();
    let mut stderr_bytes = Vec::new();
    stdout.read_to_end(&mut stdout_bytes)?;
    stderr.read_to_end(&mut stderr_bytes)?;
    let signal = candidate.terminal_recognizer.classify(
        &candidate.account,
        &stdout_bytes,
        &stderr_bytes,
        status,
    );
    let outcome =
        classify_terminal_outcome(signal.kind, &stdout_bytes, &stderr_bytes, status, cancelled);
    let signal_kind = if outcome == TerminalOutcome::ModelAtCapacity {
        "ModelAtCapacity".to_owned()
    } else {
        format!("{:?}", signal.kind)
    };
    if terminal.version != 1
        || terminal.binding != decision.binding
        || terminal.selection != decision.selection
        || terminal.grant_id != grant.id
        || terminal.physical_q_sha256 != q_sha
        || terminal.physical_q_unix_nanos != q_time
        || terminal.signal_kind != signal_kind
        || terminal.outcome != outcome
    {
        return Err(invalid("offline typed terminal certification changed"));
    }
    Ok(Some((
        outcome,
        i64::try_from(q_time).map_err(io::Error::other)?,
    )))
}

pub(crate) fn offline_snapshot(root: &Path, source: &Path) -> io::Result<OfflineSnapshot> {
    offline_snapshot_inner(root, source, false)
}

pub(crate) fn offline_snapshot_v3(root: &Path, source: &Path) -> io::Result<OfflineSnapshot> {
    offline_snapshot_inner(root, source, true)
}

fn offline_snapshot_inner(
    root: &Path,
    source: &Path,
    allow_uncertain_q: bool,
) -> io::Result<OfflineSnapshot> {
    let mut snapshot = OfflineSnapshot::default();
    let mut decisions = HashSet::new();
    let mut grants = HashSet::new();
    let mut consumed = HashSet::new();
    let mut terminals = HashSet::new();
    let mut entries = std::fs::read_dir(root)?
        .map(|item| item.map(|e| e.file_name().to_string_lossy().into_owned()))
        .collect::<io::Result<Vec<_>>>()?;
    entries.sort();
    for name in &entries {
        let known = matches!(
            name.as_str(),
            "index-v1" | "account-effects" | "manual-quota" | "route-selection.lock"
        ) || [
            ".route-source.json",
            ".route-selection.json",
            ".fresh-grant.json",
            ".consumed.json",
            ".attach.json",
            ".exit.json",
            ".drain.json",
            ".pid1-wait.json",
            ".terminal.json",
            ".cancel.json",
            ".stdout",
            ".stderr",
        ]
        .iter()
        .any(|suffix| name.ends_with(suffix))
            || (name.contains(".route-") && name.ends_with(".json"));
        if !known {
            return Err(invalid("unsupported pre-index broker evidence format"));
        }
    }
    for name in &entries {
        let Some(handoff) = name.strip_suffix(".route-selection.json") else {
            continue;
        };
        let decision: RouteDecision =
            exact_file(root, name)?.ok_or_else(|| invalid("offline decision vanished"))?;
        if decision.version != 1
            || decision.selection.policy_version != FRESH_ROUTE_POLICY_VERSION
            || decision.binding.handoff_id != handoff
            || decision.total == 0
            || decision.selection.index >= decision.total
            || decision.pin.is_some() != (decision.sequence == 0)
            || !decision
                .selection
                .eligible_accounts
                .contains(&decision.selection.account)
        {
            return Err(invalid(
                "offline decision version, identity or sequence invalid",
            ));
        }
        let mut candidate_names = HashSet::new();
        let mut physical_keys = HashSet::new();
        let mut selected = None;
        for index in 0..decision.total {
            let candidate: RouteCandidate = exact_file(root, &candidate_name(handoff, index))?
                .ok_or_else(|| invalid("offline decision candidate missing"))?;
            validate_candidate(root, source, &candidate)?;
            if candidate.binding != decision.binding
                || candidate.model != decision.selection.model
                || candidate.config_sha256 != decision.selection.config_sha256
                || candidate.total != decision.total
                || candidate.index != index
                || candidate.pin != decision.pin
                || !candidate_names.insert(candidate.account.clone())
                || !physical_keys.insert(candidate.account_identity.clone())
            {
                return Err(invalid("offline decision candidate roster changed"));
            }
            if index == decision.selection.index {
                selected = Some(candidate);
            }
        }
        let candidate = selected.ok_or_else(|| invalid("offline selected candidate absent"))?;
        if snapshot
            .source_models
            .insert(candidate.model.clone(), candidate.config_sha256.clone())
            .is_some_and(|prior| prior != candidate.config_sha256)
        {
            return Err(invalid(
                "offline model has multiple retained config digests",
            ));
        }
        if candidate.account != decision.selection.account
            || candidate.account_identity != decision.selection.account_identity
            || candidate.plan_sha256 != decision.selection.plan_sha256
        {
            return Err(invalid("offline decision account or plan changed"));
        }
        decisions.insert(handoff.to_owned());
        snapshot.decisions.push(OfflineDecision {
            handoff: handoff.to_owned(),
            key: crate::linux_main::fresh_index::CursorKey {
                model: candidate.model.clone(),
                config_sha256: candidate.config_sha256.clone(),
            },
            candidate_identity: candidate.account_identity.clone(),
            candidate_index: candidate.index,
            pin: decision.pin.is_some(),
            sequence: decision.sequence,
            receipt: artifact(root, name)?,
        });
        let grant_name = format!("{handoff}.fresh-grant.json");
        if let Some(grant) = exact_file::<Grant>(root, &grant_name)? {
            if grant.version != 1
                || grant.binding != decision.binding
                || grant.plan_sha256 != candidate.plan_sha256
                || !grants.insert(grant.id.clone())
            {
                return Err(invalid(
                    "offline provider grant binding changed or duplicated",
                ));
            }
            let k_name = format!("{}.consumed.json", grant.id);
            let k = exact_file::<Grant>(root, &k_name)?;
            if k.as_ref().is_some_and(|k| k != &grant) {
                return Err(invalid("offline provider K differs from grant"));
            }
            let consumed_k = if k.is_some() {
                consumed.insert(grant.id.clone());
                Some(artifact(root, &k_name)?)
            } else {
                None
            };
            if root.join(format!("{}.drain.json", grant.id)).exists() && consumed_k.is_none() {
                return Err(invalid("offline provider Q precedes K"));
            }
            let a = account(&mut snapshot, &candidate.account_identity);
            if a.grants
                .insert(
                    grant.id.clone(),
                    ProviderGrant {
                        decision_handoff: handoff.to_owned(),
                        grant: artifact(root, &grant_name)?,
                        candidate: Some(artifact(root, candidate_name(handoff, candidate.index))?),
                        consumed_k,
                        certified_q: None,
                    },
                )
                .is_some()
            {
                return Err(invalid("offline provider grant duplicated"));
            }
            if k.is_some() {
                a.observed_invocations += 1;
                if let Some((outcome, time)) =
                    check_terminal(root, &decision, &candidate, &grant, allow_uncertain_q)?
                {
                    terminals.insert(grant.id.clone());
                    a.grants.get_mut(&grant.id).unwrap().certified_q = Some(PhysicalQ {
                        physical_k: artifact(root, &k_name)?,
                        q: artifact(root, format!("{}.drain.json", grant.id))?,
                        terminal: Some(artifact(root, format!("{}.terminal.json", grant.id))?),
                        completed_unix_nanos: time,
                    });
                    match outcome {
                        TerminalOutcome::QuotaRejected => {
                            a.markers.quota_rejection_nanos = Some(
                                a.markers
                                    .quota_rejection_nanos
                                    .unwrap_or(i64::MIN)
                                    .max(time),
                            )
                        }
                        TerminalOutcome::AuthRejected => {
                            a.markers.auth_rejection_nanos =
                                Some(a.markers.auth_rejection_nanos.unwrap_or(i64::MIN).max(time))
                        }
                        TerminalOutcome::ModelAtCapacity => {
                            a.markers.model_capacity_nanos =
                                Some(a.markers.model_capacity_nanos.unwrap_or(i64::MIN).max(time))
                        }
                        _ => {}
                    }
                }
            }
        }
    }
    for name in &entries {
        if let Some(handoff) = name.strip_suffix(".fresh-grant.json") {
            if !decisions.contains(handoff) {
                return Err(invalid("offline provider grant has no decision"));
            }
        }
        if let Some(id) = name.strip_suffix(".consumed.json") {
            if !consumed.contains(id) {
                return Err(invalid("offline provider K has no selected grant"));
            }
        }
        if let Some(id) = name.strip_suffix(".terminal.json") {
            if !terminals.contains(id) {
                return Err(invalid("offline terminal lacks certified Q"));
            }
        }
        if let Some(id) = name.strip_suffix(".drain.json") {
            if !consumed.contains(id) {
                return Err(invalid("offline provider Q has no selected K"));
            }
        }
    }
    let mut effect_handoffs = HashSet::new();
    let effect_dirs = match std::fs::read_dir(root.join("account-effects")) {
        Ok(entries) => Some(entries),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(error),
    };
    if let Some(effect_dirs) = effect_dirs {
        for entry in effect_dirs {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                let intent = effect_intent(&entry.path())?
                    .ok_or_else(|| invalid("offline effect directory has no intent"))?;
                effect_handoffs.insert(intent.binding.handoff_id);
            }
        }
    }
    for name in &entries {
        let candidate_handoff = name.split_once(".route-").map(|(handoff, _)| handoff);
        let handoff = name
            .strip_suffix(".route-source.json")
            .or(candidate_handoff);
        if let Some(handoff) = handoff {
            if !decisions.contains(handoff) && !effect_handoffs.contains(handoff) {
                if !allow_uncertain_q {
                    return Err(invalid(
                        "offline candidate has no decision or effect intent",
                    ));
                }
                // A private v3 root may have registered a candidate before
                // the broker stopped, without ever beginning an effect. The
                // source-validated candidate is inert: no route or K can be
                // inferred from it, and the original root must submit the
                // quota request again after restart.
                if candidate_handoff.is_some() && !name.ends_with(".route-source.json") {
                    let candidate: RouteCandidate = exact_file(root, name)?
                        .ok_or_else(|| invalid("v3 inert candidate vanished"))?;
                    validate_candidate(root, source, &candidate)?;
                }
            }
        }
    }
    collect_effects(root, source, &mut snapshot)?;
    if allow_uncertain_q {
        super::super::manual_quota::offline_collect_mode(root, source, &mut snapshot, true)?;
    } else {
        super::super::manual_quota::offline_collect(root, source, &mut snapshot)?;
    }
    Ok(snapshot)
}

/// Complete the read-only v3 projection from independently observed physical
/// effect Q and the retained typed result. An absent or uncertain result stays
/// debt; no result file is materialized by a rebuild.
pub(crate) fn complete_v3_effects(root: &Path, snapshot: &mut OfflineSnapshot) -> io::Result<()> {
    for (physical_key, account) in &mut snapshot.accounts {
        for (id, indexed) in &mut account.effects {
            if indexed.kind == EffectKind::ManualQuota || indexed.reuse.is_some() {
                continue;
            }
            let relative = Path::new(&indexed.intent.path);
            let dir = root.join(
                relative
                    .parent()
                    .ok_or_else(|| invalid("v3 effect path invalid"))?,
            );
            let intent = effect_intent(&dir)?.ok_or_else(|| invalid("v3 effect intent absent"))?;
            if intent.id != *id {
                return Err(invalid("v3 effect identity changed"));
            }
            let candidate = effect_candidate(root, &intent.binding, &intent.request)?;
            if candidate.account_identity != *physical_key {
                return Err(invalid("v3 effect physical account changed"));
            }
            let grant = exact_file::<Grant>(
                &dir,
                &format!("{}.fresh-grant.json", intent.binding.handoff_id),
            )?;
            let Some(grant) = grant else {
                if indexed.consumed_k.is_some() {
                    return Err(invalid("v3 effect K without grant"));
                }
                continue;
            };
            let q_name = format!("{}.drain.json", grant.id);
            if !dir.join(&q_name).exists() {
                continue;
            }
            let Some(k) = indexed.consumed_k.clone() else {
                return Err(invalid("v3 effect Q without K"));
            };
            let readback = effect_readback_from_dir_mode(&dir, &intent, false)?;
            if readback.state != "drained" {
                // A filename without independent physical readback remains debt.
                continue;
            }
            let q_path = dir.join(&q_name);
            indexed.certified_q = Some(PhysicalQ {
                physical_k: k,
                q: artifact(root, q_path.strip_prefix(root).map_err(io::Error::other)?)?,
                terminal: None,
                completed_unix_nanos: i64::try_from(file_unix_nanos(&q_path)?)
                    .map_err(io::Error::other)?,
            });
            if dir.join("result.json").exists() {
                indexed.result = Some(artifact(
                    root,
                    dir.join("result.json")
                        .strip_prefix(root)
                        .map_err(io::Error::other)?,
                )?);
            }
        }
    }
    Ok(())
}

fn collect_effects(root: &Path, source: &Path, snapshot: &mut OfflineSnapshot) -> io::Result<()> {
    let parent = root.join("account-effects");
    let entries = match std::fs::read_dir(&parent) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    for entry in entries {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let dir = entry.path();
        let intent = effect_intent(&dir)?
            .ok_or_else(|| invalid("offline effect directory has no intent"))?;
        if intent.version != 1 || intent.id.is_empty() || !hash_shape(&intent.environment_sha256) {
            return Err(invalid("offline effect intent unsupported"));
        }
        let expected = effect_directory(root, &intent.binding, &intent.request);
        if expected != dir {
            return Err(invalid("offline effect directory identity changed"));
        }
        if dir.join("reuse.json").exists() && dir.join("manual-reuse.json").exists() {
            return Err(invalid("offline effect has conflicting reuse references"));
        }
        if let Some(reuse) = exact_file::<QuotaReuse>(&dir, "reuse.json")? {
            if intent.plan_sha256 != format!("reused:{}", reuse.source_effect_id) {
                return Err(invalid("offline quota reuse plan reference changed"));
            }
        } else if intent.auth_source.is_none()
            && !dir.join("manual-reuse.json").exists()
            && !hash_shape(&intent.plan_sha256)
        {
            return Err(invalid("offline physical effect plan digest invalid"));
        }
        let candidate = effect_candidate(root, &intent.binding, &intent.request)?;
        validate_candidate(root, source, &candidate)?;
        if snapshot
            .source_models
            .insert(candidate.model.clone(), candidate.config_sha256.clone())
            .is_some_and(|prior| prior != candidate.config_sha256)
        {
            return Err(invalid("offline effect model has multiple config digests"));
        }
        if candidate.account != intent.request.account
            || candidate.config_sha256 != intent.request.config_sha256
            || candidate.model != intent.request.model
        {
            return Err(invalid("offline effect candidate changed"));
        }
        // Exact readback checks quota/auth/manual and peer reuse provenance.
        // It is read-only during rebuild, including when physical Q exists.
        let readback = effect_readback_from_dir_mode(&dir, &intent, false)?;
        let grant = exact_file::<Grant>(
            &dir,
            &format!("{}.fresh-grant.json", intent.binding.handoff_id),
        )?;
        let k = if let Some(grant) = grant.as_ref() {
            if grant.version != 1
                || grant.binding != intent.binding
                || grant.plan_sha256 != intent.plan_sha256
            {
                return Err(invalid("offline effect grant/plan changed"));
            }
            let k_name = format!("{}.consumed.json", grant.id);
            let consumed: Option<Grant> = exact_file(&dir, &k_name)?;
            if consumed.as_ref().is_some_and(|k| k != grant) {
                return Err(invalid("offline effect K differs from grant"));
            }
            if dir.join(format!("{}.drain.json", grant.id)).exists() {
                if consumed.is_none() || readback.state != "drained" {
                    return Err(invalid("offline effect Q lacks certified K/readback"));
                }
            }
            consumed
                .map(|_| artifact(root, dir.strip_prefix(root).unwrap().join(k_name)))
                .transpose()?
        } else {
            if readback.state == "drained"
                && intent.auth_source.is_none()
                && !dir.join("reuse.json").exists()
                && !dir.join("manual-reuse.json").exists()
            {
                return Err(invalid("offline effect Q has no physical grant"));
            }
            None
        };
        let kind = match intent.request.kind {
            FreshAccountEffectKind::QuotaFirst | FreshAccountEffectKind::QuotaRetry => {
                EffectKind::Quota
            }
            FreshAccountEffectKind::AuthRefresh => EffectKind::Auth,
        };
        let source_key = source_key(&candidate, &intent.environment_sha256)?;
        let relative = dir.strip_prefix(root).map_err(io::Error::other)?;
        let effect = EffectIntent {
            kind,
            source: source_key,
            decision_handoff: intent.binding.handoff_id.clone(),
            route_source: Some(artifact(
                root,
                format!("{}.route-source.json", intent.binding.handoff_id),
            )?),
            candidate: Some(artifact(
                root,
                candidate_name(&intent.binding.handoff_id, candidate.index),
            )?),
            intent: artifact(root, relative.join("intent.json"))?,
            reuse: if intent.auth_source.is_some() {
                Some(artifact(root, relative.join("intent.json"))?)
            } else {
                ["reuse.json", "manual-reuse.json"]
                    .iter()
                    .find(|name| dir.join(name).exists())
                    .map(|name| artifact(root, relative.join(name)))
                    .transpose()?
            },
            consumed_k: k,
            // Publication stays read-only. Live admission certifies exact
            // physical Q and result together after independent readback.
            certified_q: None,
            result: None,
        };
        let a = account(snapshot, &candidate.account_identity);
        if a.effects.insert(intent.id.clone(), effect).is_some() {
            return Err(invalid("offline effect ID duplicated"));
        }
    }
    Ok(())
}

/// After publication, settle only Q that is independently observable now.
/// An absent Q, unknown physical state, or nonhealthy effect stays unresolved.
pub(crate) fn reconcile_offline_account(
    index: &Index,
    key: &str,
    source: &Path,
) -> io::Result<Account> {
    let root = index.evidence_root();
    let mut current = index.account(key).map_err(io::Error::other)?;
    let grants = current.grants.clone();
    for (id, indexed) in grants {
        let Some(k) = indexed.consumed_k else {
            continue;
        };
        let decision: RouteDecision = exact_file(root, &decision_name(&indexed.decision_handoff))?
            .ok_or_else(|| invalid("reconcile decision absent"))?;
        let candidate: RouteCandidate = exact_file(
            root,
            &candidate_name(&indexed.decision_handoff, decision.selection.index),
        )?
        .ok_or_else(|| invalid("reconcile candidate absent"))?;
        validate_candidate(root, source, &candidate)?;
        if candidate.account_identity != key {
            return Err(invalid("reconcile physical account changed"));
        }
        let grant: Grant = exact_file(
            root,
            &format!("{}.fresh-grant.json", indexed.decision_handoff),
        )?
        .ok_or_else(|| invalid("reconcile grant absent"))?;
        if grant.id != id || artifact(root, format!("{}.consumed.json", id))? != k {
            return Err(invalid("reconcile provider K changed"));
        }
        let Some((outcome, completed_unix_nanos)) =
            check_terminal(root, &decision, &candidate, &grant, false)?
        else {
            continue;
        };
        let q = PhysicalQ {
            physical_k: k,
            q: artifact(root, format!("{}.drain.json", id))?,
            terminal: Some(artifact(root, format!("{}.terminal.json", id))?),
            completed_unix_nanos,
        };
        if let Some(previous) = &indexed.certified_q {
            if previous != &q {
                return Err(invalid("reconcile provider Q changed"));
            }
            continue;
        }
        let marker = match outcome {
            TerminalOutcome::QuotaRejected => Some(TerminalMarkerKind::Quota),
            TerminalOutcome::AuthRejected => Some(TerminalMarkerKind::Auth),
            TerminalOutcome::ModelAtCapacity => Some(TerminalMarkerKind::ModelCapacity),
            _ => None,
        };
        current = index
            .update_account(
                key,
                current.revision,
                AccountUpdate::SettleGrant {
                    id,
                    q,
                    failed: outcome != TerminalOutcome::Clean,
                    marker,
                },
            )
            .map_err(io::Error::other)?;
    }
    let effects = current.effects.clone();
    for (id, indexed) in effects {
        if indexed.kind != EffectKind::ManualQuota {
            // The live writer retains physical effect IDs and certifies Q
            // together with the materialized result at broker admission.
            continue;
        }
        if indexed.certified_q.is_some() {
            // Frozen collection already certified the exact manual Q and
            // retained its result identity for live admission readback.
            continue;
        }
        let Some(k) = indexed.consumed_k else {
            continue;
        };
        let intent_path = Path::new(&indexed.intent.path);
        let relative_dir = intent_path
            .parent()
            .ok_or_else(|| invalid("reconcile effect path invalid"))?;
        let dir = root.join(relative_dir);
        let (q_relative, healthy, completed_unix_nanos) =
            if relative_dir.starts_with("manual-quota") {
                let Some((time, healthy)) =
                    super::super::manual_quota::offline_certified_q(root, &id, source)?
                else {
                    continue;
                };
                (
                    relative_dir.join("q.json"),
                    healthy,
                    i64::try_from(time).map_err(io::Error::other)?,
                )
            } else {
                let intent: AccountEffectIntent = effect_intent(&dir)?
                    .ok_or_else(|| invalid("reconcile effect intent absent"))?;
                if intent.id != id {
                    return Err(invalid("reconcile effect ID changed"));
                }
                let candidate = effect_candidate(root, &intent.binding, &intent.request)?;
                validate_candidate(root, source, &candidate)?;
                if candidate.account_identity != key {
                    return Err(invalid("reconcile effect account changed"));
                }
                let grant: Grant = exact_file(
                    &dir,
                    &format!("{}.fresh-grant.json", intent.binding.handoff_id),
                )?
                .ok_or_else(|| invalid("reconcile effect grant absent"))?;
                let q_relative = relative_dir.join(format!("{}.drain.json", grant.id));
                if !root.join(&q_relative).exists() {
                    continue;
                }
                let Observation::Drained { .. } = observe(&dir, &grant.id)? else {
                    return Err(invalid("reconcile effect Q not physically certified"));
                };
                let result = effect_readback_from_dir_mode(&dir, &intent, false)?;
                if result.state != "drained" {
                    return Err(invalid("reconcile effect result not certified"));
                }
                let healthy = match indexed.kind {
                    EffectKind::Quota => result.outcome.as_deref() == Some("valid_windows"),
                    EffectKind::Auth => result.outcome.as_deref() == Some("refreshed"),
                    EffectKind::ManualQuota => false,
                };
                let time = i64::try_from(file_unix_nanos(&root.join(&q_relative))?)
                    .map_err(io::Error::other)?;
                (q_relative, healthy, time)
            };
        let q_artifact = artifact(root, q_relative)?;
        let q = PhysicalQ {
            physical_k: k,
            q: q_artifact.clone(),
            terminal: None,
            completed_unix_nanos,
        };
        current = index
            .update_account(
                key,
                current.revision,
                AccountUpdate::SettleEffect {
                    id,
                    q,
                    result: Some(q_artifact),
                    marker: healthy.then_some(false),
                },
            )
            .map_err(io::Error::other)?;
    }
    Ok(current)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::linux_main::fresh_index::{
        Index, KeyedGeneration, broker_admission_lease, rebuild_keyed_offline,
        rebuild_keyed_offline_test_hook,
    };
    use oulipoly_kernel_broker::protocol::ManualQuotaRequest;

    fn other_process_can_freeze(path: &Path) -> bool {
        let name = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
        let child = unsafe { libc::fork() };
        assert!(child >= 0);
        if child == 0 {
            let fd = unsafe { libc::open(name.as_ptr(), libc::O_RDWR | libc::O_CLOEXEC) };
            let lock = libc::flock {
                l_type: libc::F_WRLCK as _,
                l_whence: libc::SEEK_SET as _,
                l_start: 0,
                l_len: 0,
                l_pid: 0,
            };
            let acquired = fd >= 0 && unsafe { libc::fcntl(fd, libc::F_SETLK, &lock) } == 0;
            unsafe { libc::_exit(if acquired { 0 } else { 1 }) }
        }
        let mut status = 0;
        assert_eq!(unsafe { libc::waitpid(child, &mut status, 0) }, child);
        libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 0
    }

    struct Fixture {
        temp: tempfile::TempDir,
        root: PathBuf,
        source: PathBuf,
        socket: PathBuf,
        binding: Binding,
        grant: Grant,
        candidate: RouteCandidate,
    }
    impl Fixture {
        fn new() -> Self {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().join("v30/fresh-provider");
            let source = temp.path().join("source");
            let socket = temp.path().join("broker.sock");
            std::fs::create_dir_all(&root).unwrap();
            std::fs::create_dir_all(source.join("models")).unwrap();
            let quota_script = r#"printf '{"used_percent":20,"resets_at":"2099-01-01T00:00:00Z"}'"#;
            std::fs::write(source.join("providers.toml"), format!(
                "[first]\ncommand = '/bin/true'\nquota_account_id = 'physical-first'\nquota_script = {}\nauth_refresh_command = 'true'\n",
                serde_json::to_string(quota_script).unwrap(),
            )).unwrap();
            std::fs::write(
                source.join("models/work.toml"),
                "[[providers]]\nname = 'first'\n",
            )
            .unwrap();
            let pool = oulipoly_runtime::executor::cli::fresh_remote::load_fresh_headless_pool(
                &source, "work",
            )
            .unwrap();
            let binding = Binding {
                root_id: uuid::Uuid::new_v4().to_string(),
                handoff_id: uuid::Uuid::new_v4().to_string(),
                invocation_uuid: uuid::Uuid::new_v4().to_string(),
                session_id: "v30:test".into(),
                owner_generation: uuid::Uuid::new_v4().to_string(),
                actor_pid: 100,
                actor_starttime: 1,
                actor_boot_id: uuid::Uuid::new_v4().to_string(),
                actor_pidns_dev: 1,
                actor_pidns_ino: 2,
                root_pid: 99,
                root_starttime: 1,
                root_pidns_dev: 1,
                root_pidns_ino: 2,
            };
            let candidate = RouteCandidate {
                version: 3,
                binding: binding.clone(),
                model: "work".into(),
                config_sha256: pool.config_sha256.clone(),
                account: "first".into(),
                account_identity: "physical-first".into(),
                index: 0,
                total: 1,
                pin: None,
                plan_sha256: "a".repeat(64),
                environment_sha256: "0".repeat(64),
                quota_script: pool.account_effects[0].0.clone(),
                auth_refresh_command: pool.account_effects[0].1.clone(),
                terminal_recognizer: FreshTerminalRecognizer::for_provider(
                    &pool.model.providers[0],
                ),
            };
            let grant = Grant {
                version: 1,
                id: uuid::Uuid::new_v4().to_string(),
                binding: binding.clone(),
                plan_sha256: candidate.plan_sha256.clone(),
            };
            let mut fixture = Self {
                temp,
                root,
                source,
                socket,
                binding,
                grant,
                candidate,
            };
            fixture.register_candidate();
            fixture.decision(1, false);
            fixture
        }
        fn register_candidate(&mut self) {
            let meta = self.source.metadata().unwrap();
            durable_new(
                &self.root,
                &format!("{}.route-source.json", self.binding.handoff_id),
                &RouteSource {
                    version: 1,
                    binding: self.binding.clone(),
                    config_sha256: self.candidate.config_sha256.clone(),
                    directory_device: meta.dev(),
                    directory_inode: meta.ino(),
                },
            )
            .unwrap();
            durable_new(
                &self.root,
                &candidate_name(&self.binding.handoff_id, 0),
                &self.candidate,
            )
            .unwrap();
        }
        fn decision(&self, sequence: u64, pin: bool) {
            let mut candidate = self.candidate.clone();
            candidate.pin = pin.then(|| "first".into());
            if pin {
                std::fs::remove_file(self.root.join(candidate_name(&self.binding.handoff_id, 0)))
                    .unwrap();
                durable_new(
                    &self.root,
                    &candidate_name(&self.binding.handoff_id, 0),
                    &candidate,
                )
                .unwrap();
            }
            durable_new(
                &self.root,
                &decision_name(&self.binding.handoff_id),
                &RouteDecision {
                    version: 1,
                    binding: self.binding.clone(),
                    total: 1,
                    pin: pin.then(|| "first".into()),
                    environment_sha256: None,
                    sequence,
                    selection: FreshRouteSelection {
                        model: "work".into(),
                        config_sha256: self.candidate.config_sha256.clone(),
                        account: "first".into(),
                        account_identity: "physical-first".into(),
                        index: 0,
                        plan_sha256: self.candidate.plan_sha256.clone(),
                        observed_live: 0,
                        observed_failures: 0,
                        observed_invocations: 0,
                        policy_version: FRESH_ROUTE_POLICY_VERSION.into(),
                        eligible_accounts: vec!["first".into()],
                        quota_remaining_basis_points: Some(8000),
                    },
                },
            )
            .unwrap();
        }
        fn prepared(&self) {
            durable_new(
                &self.root,
                &format!("{}.fresh-grant.json", self.binding.handoff_id),
                &self.grant,
            )
            .unwrap();
        }
        fn consumed(&self) {
            self.prepared();
            durable_new(
                &self.root,
                &format!("{}.consumed.json", self.grant.id),
                &self.grant,
            )
            .unwrap();
        }
        fn certified_q(&self, stdout_bytes: &[u8]) {
            let work = uuid::Uuid::new_v4().to_string();
            durable_new(
                &self.root,
                &format!("{}.attach.json", self.grant.id),
                &Attach {
                    version: 1,
                    grant_id: self.grant.id.clone(),
                    work_id: work.clone(),
                    pid1: 999_999_999,
                    pid1_starttime: 1,
                    pidns_dev: 1,
                    pidns_ino: 2,
                    pid1_parent_namespace_pid: 98,
                    provider_pid: 97,
                    provider_starttime: 1,
                    provider_local_pid: 3,
                },
            )
            .unwrap();
            durable_new(
                &self.root,
                &format!("{}.exit.json", self.grant.id),
                &ProviderExit {
                    version: 1,
                    grant_id: self.grant.id.clone(),
                    work_id: work.clone(),
                    provider_local_pid: 3,
                    wait_status: 0,
                },
            )
            .unwrap();
            let stdout_path = self.root.join(format!("{}.stdout", self.grant.id));
            let stderr_path = self.root.join(format!("{}.stderr", self.grant.id));
            std::fs::write(&stdout_path, stdout_bytes).unwrap();
            std::fs::write(&stderr_path, b"").unwrap();
            let stdout = output(&File::open(&stdout_path).unwrap()).unwrap();
            let stderr = output(&File::open(&stderr_path).unwrap()).unwrap();
            durable_new(
                &self.root,
                &format!("{}.drain.json", self.grant.id),
                &Drain {
                    version: 1,
                    grant_id: self.grant.id.clone(),
                    work_id: work.clone(),
                    stdout,
                    stderr,
                    cancelled: false,
                    zero_remaining: true,
                },
            )
            .unwrap();
            durable_new(
                &self.root,
                &format!("{}.pid1-wait.json", self.grant.id),
                &Pid1Wait {
                    version: 1,
                    grant_id: self.grant.id.clone(),
                    work_id: work,
                    pid1_parent_namespace_pid: 98,
                    wait_status: 0,
                    reaped: true,
                },
            )
            .unwrap();
            let decision: RouteDecision =
                exact_file(&self.root, &decision_name(&self.binding.handoff_id))
                    .unwrap()
                    .unwrap();
            terminal_record(
                &self.root,
                &decision,
                &self.candidate,
                &self.grant,
                0,
                File::open(stdout_path).unwrap(),
                File::open(stderr_path).unwrap(),
                false,
            )
            .unwrap();
        }
        fn effect_q(directory: &Path, grant: &Grant, stdout_bytes: &[u8]) {
            let work = uuid::Uuid::new_v4().to_string();
            durable_new(
                directory,
                &format!("{}.attach.json", grant.id),
                &Attach {
                    version: 1,
                    grant_id: grant.id.clone(),
                    work_id: work.clone(),
                    pid1: 999_999_999,
                    pid1_starttime: 1,
                    pidns_dev: 1,
                    pidns_ino: 2,
                    pid1_parent_namespace_pid: 98,
                    provider_pid: 97,
                    provider_starttime: 1,
                    provider_local_pid: 3,
                },
            )
            .unwrap();
            durable_new(
                directory,
                &format!("{}.exit.json", grant.id),
                &ProviderExit {
                    version: 1,
                    grant_id: grant.id.clone(),
                    work_id: work.clone(),
                    provider_local_pid: 3,
                    wait_status: 0,
                },
            )
            .unwrap();
            let stdout_path = directory.join(format!("{}.stdout", grant.id));
            let stderr_path = directory.join(format!("{}.stderr", grant.id));
            std::fs::write(&stdout_path, stdout_bytes).unwrap();
            std::fs::write(&stderr_path, b"").unwrap();
            let stdout = output(&File::open(&stdout_path).unwrap()).unwrap();
            let stderr = output(&File::open(&stderr_path).unwrap()).unwrap();
            durable_new(
                directory,
                &format!("{}.drain.json", grant.id),
                &Drain {
                    version: 1,
                    grant_id: grant.id.clone(),
                    work_id: work.clone(),
                    stdout,
                    stderr,
                    cancelled: false,
                    zero_remaining: true,
                },
            )
            .unwrap();
            durable_new(
                directory,
                &format!("{}.pid1-wait.json", grant.id),
                &Pid1Wait {
                    version: 1,
                    grant_id: grant.id.clone(),
                    work_id: work,
                    pid1_parent_namespace_pid: 98,
                    wait_status: 0,
                    reaped: true,
                },
            )
            .unwrap();
        }
        fn ready(&self) {
            drop(broker_admission_lease(&self.root).unwrap());
        }
        fn rebuild(&self) -> crate::linux_main::fresh_index::Result<Index> {
            Index::rebuild_offline(&self.root, &self.socket, &self.source)
        }
    }

    #[test]
    fn nonempty_preindex_prepared_grant_cutover_and_old_wal_isolation() {
        let fixture = Fixture::new();
        fixture.prepared();
        let wal = fixture.temp.path().join("state.db-wal");
        std::fs::write(&wal, b"old WAL stays byte exact").unwrap();
        fixture.ready();
        let index = fixture.rebuild().unwrap();
        assert_eq!(
            index
                .cursor(&crate::linux_main::fresh_index::CursorKey {
                    model: "work".into(),
                    config_sha256: fixture.candidate.config_sha256.clone(),
                })
                .unwrap()
                .sequence,
            1
        );
        let account = index.account("physical-first").unwrap();
        assert_eq!(account.observed_invocations, 0);
        assert!(account.grants[&fixture.grant.id].consumed_k.is_none());
        assert_eq!(std::fs::read(wal).unwrap(), b"old WAL stays byte exact");
        let prior = index.generation().to_owned();
        let next = fixture.rebuild().unwrap();
        assert_ne!(next.generation(), prior);
        assert!(
            fixture
                .root
                .join("index-v1/generations")
                .join(prior)
                .is_dir()
        );
        assert_eq!(
            Index::open(&fixture.root).unwrap().generation(),
            next.generation()
        );
    }

    #[test]
    fn v3_empty_genesis_and_pre_k_debt_are_private_and_idempotent() {
        let fixture = Fixture::new();
        fixture.prepared();
        let wal = fixture.temp.path().join("state.db-wal");
        std::fs::write(&wal, b"old WAL stays byte exact").unwrap();
        fixture.ready();
        rebuild_keyed_offline(&fixture.root, &fixture.socket, &fixture.source).unwrap();
        let first = KeyedGeneration::open(&fixture.root).unwrap();
        let account = first.account("physical-first").unwrap();
        let (revision, pending, source) = account.compact_source("absent").unwrap();
        assert!(revision > 0);
        assert_eq!(pending, 1);
        assert!(source.is_none());
        assert!(account.get("grant", &fixture.grant.id).unwrap().is_some());
        assert!(
            account
                .get("pending", &format!("grant:{}", fixture.grant.id))
                .unwrap()
                .is_some()
        );
        assert!(Index::open(&fixture.root).is_err());
        assert_eq!(std::fs::read(&wal).unwrap(), b"old WAL stays byte exact");
        rebuild_keyed_offline(&fixture.root, &fixture.socket, &fixture.source).unwrap();
        let second = KeyedGeneration::open(&fixture.root).unwrap();
        assert_ne!(first.generation, second.generation);
        assert_eq!(
            second
                .account("physical-first")
                .unwrap()
                .summary()
                .unwrap()
                .1,
            1
        );
        assert_eq!(std::fs::read(&wal).unwrap(), b"old WAL stays byte exact");
        std::fs::remove_dir_all(
            fixture
                .root
                .join("index-v1/generations")
                .join(&second.generation)
                .join("keyed-accounts")
                .join(sha_text(&"physical-first").unwrap()),
        )
        .unwrap();
        assert!(KeyedGeneration::open(&fixture.root).is_err());
    }

    #[test]
    fn v3_provider_readback_admission_is_exact_and_refuses_late_physical_changes() {
        let fixture = Fixture::new();
        fixture.prepared();
        let wal = fixture.temp.path().join("state.db-wal");
        std::fs::write(&wal, b"old WAL stays byte exact").unwrap();
        fixture.ready();
        rebuild_keyed_offline(&fixture.root, &fixture.socket, &fixture.source).unwrap();
        let lease = broker_admission_lease(&fixture.root).unwrap();
        let generation =
            KeyedGeneration::admit_provider_readback(&fixture.root, &lease, &fixture.source)
                .unwrap();
        let (verified, keyed_io) = crate::linux_main::fresh_index::measure_keyed_io(|| {
            let _physical_io =
                crate::linux_main::fresh_index::ReaderIoGuard::start("v3-provider-readback");
            require_v3_provider_binding(&generation, &fixture.root, &fixture.binding).unwrap()
        });
        let physical_io = crate::linux_main::fresh_index::last_reader_io().unwrap();
        eprintln!("v3 exact provider readback keyed={keyed_io:?} physical={physical_io:?}");
        assert_eq!(verified, fixture.grant.id);
        assert_eq!(
            keyed_io.directory_entries + physical_io.directory_entries,
            0
        );
        assert_eq!(keyed_io.bytes_written + physical_io.bytes_written, 0);
        assert_eq!(
            require_v3_provider_binding(&generation, &fixture.root, &fixture.binding).unwrap(),
            fixture.grant.id,
        );
        assert!(Index::open(&fixture.root).is_err());
        assert_eq!(std::fs::read(&wal).unwrap(), b"old WAL stays byte exact");
        // A previously held child may publish K after the frozen scan. The
        // admitted generation cannot call that an empty or settled source.
        durable_new(
            &fixture.root,
            &format!("{}.consumed.json", fixture.grant.id),
            &fixture.grant,
        )
        .unwrap();
        assert!(require_v3_provider_binding(&generation, &fixture.root, &fixture.binding).is_err());
        assert!(
            KeyedGeneration::admit_provider_readback(&fixture.root, &lease, &fixture.source)
                .is_err()
        );
        drop(lease);
        assert!(rebuild_keyed_offline(&fixture.root, &fixture.socket, &fixture.source).is_ok());
        let lease = broker_admission_lease(&fixture.root).unwrap();
        let generation =
            KeyedGeneration::admit_provider_readback(&fixture.root, &lease, &fixture.source)
                .unwrap();
        assert_eq!(
            require_v3_provider_binding(&generation, &fixture.root, &fixture.binding).unwrap(),
            fixture.grant.id,
        );
        assert_eq!(std::fs::read(&wal).unwrap(), b"old WAL stays byte exact");
    }

    #[test]
    fn v3_provider_readback_keeps_typed_capacity_and_late_q_unknown() {
        let fixture = Fixture::new();
        fixture.consumed();
        fixture.certified_q(b"{\"type\":\"error\",\"error\":{\"code\":\"model_at_capacity\"}}\n");
        fixture.ready();
        rebuild_keyed_offline(&fixture.root, &fixture.socket, &fixture.source).unwrap();
        let lease = broker_admission_lease(&fixture.root).unwrap();
        let generation =
            KeyedGeneration::admit_provider_readback(&fixture.root, &lease, &fixture.source)
                .unwrap();
        assert_eq!(
            require_v3_provider_binding(&generation, &fixture.root, &fixture.binding).unwrap(),
            fixture.grant.id,
        );
        let account = generation.account("physical-first").unwrap();
        let marker = sha_text(&("work", fixture.candidate.config_sha256.as_str())).unwrap();
        assert!(account.get("model-capacity", &marker).unwrap().is_some());
        let q = fixture
            .root
            .join(format!("{}.drain.json", fixture.grant.id));
        std::fs::write(q, b"changed Q").unwrap();
        assert!(require_v3_provider_binding(&generation, &fixture.root, &fixture.binding).is_err());
        assert!(
            KeyedGeneration::admit_provider_readback(&fixture.root, &lease, &fixture.source)
                .is_err()
        );
    }

    #[test]
    fn v3_provider_admission_requires_published_generation_and_accepts_empty_rebuild() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("v30/fresh-provider");
        let source = temp.path().join("config");
        let socket = temp.path().join("broker.sock");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir(&source).unwrap();
        let lease = broker_admission_lease(&root).unwrap();
        assert!(KeyedGeneration::admit_provider_readback(&root, &lease, &source).is_err());
        drop(lease);
        rebuild_keyed_offline(&root, &socket, &source).unwrap();
        let lease = broker_admission_lease(&root).unwrap();
        let generation = KeyedGeneration::admit_provider_readback(&root, &lease, &source).unwrap();
        assert!(!generation.generation.is_empty());
        assert!(Index::open(&root).is_err());

        let fixture = Fixture::new();
        fixture.prepared();
        fixture.ready();
        fixture.rebuild().unwrap();
        let lease = broker_admission_lease(&fixture.root).unwrap();
        assert!(
            KeyedGeneration::admit_provider_readback(&fixture.root, &lease, &fixture.source)
                .is_err()
        );
    }

    #[test]
    fn v3_pre_manifest_failure_keeps_v2_and_source_damage_refuses() {
        let fixture = Fixture::new();
        fixture.prepared();
        fixture.ready();
        let v2 = fixture.rebuild().unwrap();
        let prior = v2.generation().to_owned();
        assert!(
            rebuild_keyed_offline_test_hook(
                &fixture.root,
                &fixture.socket,
                &fixture.source,
                || Err(crate::linux_main::fresh_index::IndexError::Conflict(
                    "simulated manifest crash"
                )),
            )
            .is_err()
        );
        assert_eq!(Index::open(&fixture.root).unwrap().generation(), prior);
        std::fs::write(fixture.source.join("models/work.toml"), b"broken").unwrap();
        assert!(rebuild_keyed_offline(&fixture.root, &fixture.socket, &fixture.source).is_err());
        assert_eq!(Index::open(&fixture.root).unwrap().generation(), prior);
    }

    #[test]
    fn v3_imports_typed_quota_auth_and_manual_physical_q() {
        let fixture = Fixture::new();
        fixture.prepared();
        let mut checkpoints = Vec::new();
        for (id, kind, output_bytes) in [
            (
                "quota",
                FreshAccountEffectKind::QuotaFirst,
                b"{\"used_percent\":20,\"resets_at\":\"2099-01-01T00:00:00Z\"}".as_slice(),
            ),
            ("auth", FreshAccountEffectKind::AuthRefresh, b"".as_slice()),
        ] {
            let request = FreshAccountEffectRequest {
                d_key: uuid::Uuid::new_v4().to_string(),
                model: "work".into(),
                config_sha256: fixture.candidate.config_sha256.clone(),
                account: "first".into(),
                index: 0,
                kind,
                environment: vec![],
            };
            let dir = effect_directory(&fixture.root, &fixture.binding, &request);
            std::fs::create_dir_all(&dir).unwrap();
            let grant = Grant {
                version: 1,
                id: uuid::Uuid::new_v4().to_string(),
                binding: fixture.binding.clone(),
                plan_sha256: "b".repeat(64),
            };
            let intent = AccountEffectIntent {
                version: 1,
                id: id.into(),
                binding: fixture.binding.clone(),
                request: request.clone(),
                environment_sha256: environment_digest(&request).unwrap(),
                plan_sha256: grant.plan_sha256.clone(),
                auth_source: None,
            };
            durable_new(&dir, "intent.json", &intent).unwrap();
            durable_new(
                &dir,
                &format!("{}.fresh-grant.json", fixture.binding.handoff_id),
                &grant,
            )
            .unwrap();
            durable_new(&dir, &format!("{}.consumed.json", grant.id), &grant).unwrap();
            Fixture::effect_q(&dir, &grant, output_bytes);
            effect_readback_from_dir(&dir, &intent).unwrap();
            checkpoints.push((id, request));
        }
        let operation_id = uuid::Uuid::new_v4().to_string();
        let manual = ManualQuotaRequest {
            operation_id: operation_id.clone(),
            model: "work".into(),
            account: "first".into(),
            config_sha256: fixture.candidate.config_sha256.clone(),
            environment: vec![],
        };
        crate::linux_main::manual_quota::begin(
            &fixture.root,
            &File::open(&fixture.source).unwrap(),
            &manual,
            unsafe { libc::geteuid() },
            unsafe { libc::getegid() },
        )
        .unwrap();
        crate::linux_main::manual_quota::worker_with_environment(
            &fixture.root.join("manual-quota").join(&operation_id),
            &[],
        )
        .unwrap();
        fixture.ready();
        rebuild_keyed_offline(&fixture.root, &fixture.socket, &fixture.source).unwrap();
        let generation = KeyedGeneration::open(&fixture.root).unwrap();
        for (id, request) in &checkpoints {
            let (physical, keyed_io) = crate::linux_main::fresh_index::measure_keyed_io(|| {
                let _io =
                    crate::linux_main::fresh_index::ReaderIoGuard::start("v3-effect-checkpoint");
                generation.latest_effect_checkpoint(&fixture.source, &fixture.binding, request, id)
            });
            let physical = physical.unwrap();
            if *id == "auth" {
                let physical = physical.expect("settled latest auth effect");
                assert_eq!(physical.effect_id, *id);
                assert_eq!(physical.state, "drained");
            } else {
                // The later manual quota Q owns this source's latest quota.
                assert!(physical.is_none());
            }
            eprintln!("v3 {id} physical effect checkpoint keyed={keyed_io:?}");
        }
        let account = generation.account("physical-first").unwrap();
        assert!(account.get("effect", "quota").unwrap().is_some());
        assert!(account.get("effect", "auth").unwrap().is_some());
        assert!(account.get("manual", &operation_id).unwrap().is_some());
        let snapshot = offline_snapshot(&fixture.root, &fixture.source).unwrap();
        let source_key = &snapshot.accounts["physical-first"].effects["quota"].source;
        let source = account
            .get("source", &sha_text(source_key).unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(source["quota"]["outcome"], "valid_windows");
        assert_eq!(source["auth"]["outcome"], "refreshed");
        assert_eq!(source["quota"]["windows"].as_array().unwrap().len(), 1);
        assert_eq!(account.summary().unwrap().1, 1); // prepared provider grant
        let auth_dir = effect_directory(&fixture.root, &fixture.binding, &checkpoints[1].1);
        std::fs::remove_file(auth_dir.join("result.json")).unwrap();
        assert!(
            generation
                .latest_effect_checkpoint(
                    &fixture.source,
                    &fixture.binding,
                    &checkpoints[1].1,
                    "auth",
                )
                .is_err()
        );
    }

    #[test]
    fn v3_effect_checkpoint_keeps_late_physical_q_as_pending_debt() {
        let fixture = Fixture::new();
        let wal = fixture.temp.path().join("state.db-wal");
        std::fs::write(&wal, b"old WAL stays byte exact").unwrap();
        let request = FreshAccountEffectRequest {
            d_key: uuid::Uuid::new_v4().to_string(),
            model: "work".into(),
            config_sha256: fixture.candidate.config_sha256.clone(),
            account: "first".into(),
            index: 0,
            kind: FreshAccountEffectKind::QuotaFirst,
            environment: vec![],
        };
        let dir = effect_directory(&fixture.root, &fixture.binding, &request);
        std::fs::create_dir_all(&dir).unwrap();
        let grant = Grant {
            version: 1,
            id: uuid::Uuid::new_v4().to_string(),
            binding: fixture.binding.clone(),
            plan_sha256: "b".repeat(64),
        };
        durable_new(
            &dir,
            "intent.json",
            &AccountEffectIntent {
                version: 1,
                id: "quota".into(),
                binding: fixture.binding.clone(),
                request: request.clone(),
                environment_sha256: environment_digest(&request).unwrap(),
                plan_sha256: grant.plan_sha256.clone(),
                auth_source: None,
            },
        )
        .unwrap();
        durable_new(
            &dir,
            &format!("{}.fresh-grant.json", fixture.binding.handoff_id),
            &grant,
        )
        .unwrap();
        durable_new(&dir, &format!("{}.consumed.json", grant.id), &grant).unwrap();
        fixture.ready();
        rebuild_keyed_offline(&fixture.root, &fixture.socket, &fixture.source).unwrap();
        let generation = KeyedGeneration::open(&fixture.root).unwrap();
        assert!(
            generation
                .latest_effect_checkpoint(&fixture.source, &fixture.binding, &request, "quota")
                .unwrap()
                .is_none()
        );
        Fixture::effect_q(
            &dir,
            &grant,
            b"{\"used_percent\":20,\"resets_at\":\"2099-01-01T00:00:00Z\"}",
        );
        assert!(
            generation
                .latest_effect_checkpoint(&fixture.source, &fixture.binding, &request, "quota")
                .unwrap()
                .is_none()
        );
        assert_eq!(
            generation
                .account("physical-first")
                .unwrap()
                .summary()
                .unwrap()
                .1,
            1
        );
        rebuild_keyed_offline(&fixture.root, &fixture.socket, &fixture.source).unwrap();
        let without_result = KeyedGeneration::open(&fixture.root).unwrap();
        assert!(
            without_result
                .latest_effect_checkpoint(&fixture.source, &fixture.binding, &request, "quota")
                .unwrap()
                .is_none()
        );
        let intent = effect_intent(&dir).unwrap().unwrap();
        effect_readback_from_dir(&dir, &intent).unwrap();
        rebuild_keyed_offline(&fixture.root, &fixture.socket, &fixture.source).unwrap();
        let settled = KeyedGeneration::open(&fixture.root).unwrap();
        let result = settled
            .latest_effect_checkpoint(&fixture.source, &fixture.binding, &request, "quota")
            .unwrap()
            .expect("offline rebuilt exact K/Q/result");
        assert_eq!(result.outcome.as_deref(), Some("valid_windows"));
        assert_eq!(
            settled
                .account("physical-first")
                .unwrap()
                .summary()
                .unwrap()
                .1,
            0
        );
        assert_eq!(std::fs::read(&wal).unwrap(), b"old WAL stays byte exact");
    }

    #[test]
    fn v3_auth_alias_uses_one_physical_q_across_models() {
        let fixture = Fixture::new();
        std::fs::write(
            fixture.source.join("models/other.toml"),
            "[[providers]]\nname = 'first'\n",
        )
        .unwrap();
        let quota = FreshAccountEffectRequest {
            d_key: uuid::Uuid::new_v4().to_string(),
            model: "work".into(),
            config_sha256: fixture.candidate.config_sha256.clone(),
            account: "first".into(),
            index: 0,
            kind: FreshAccountEffectKind::QuotaFirst,
            environment: vec![],
        };
        let make_physical =
            |request: &FreshAccountEffectRequest, id: &str, output: Option<&[u8]>| {
                let dir = effect_directory(&fixture.root, &fixture.binding, request);
                std::fs::create_dir_all(&dir).unwrap();
                let grant = Grant {
                    version: 1,
                    id: uuid::Uuid::new_v4().to_string(),
                    binding: fixture.binding.clone(),
                    plan_sha256: "b".repeat(64),
                };
                let intent = AccountEffectIntent {
                    version: 1,
                    id: id.into(),
                    binding: fixture.binding.clone(),
                    request: redacted_effect_request(request),
                    environment_sha256: environment_digest(request).unwrap(),
                    plan_sha256: grant.plan_sha256.clone(),
                    auth_source: None,
                };
                durable_new(&dir, "intent.json", &intent).unwrap();
                durable_new(
                    &dir,
                    &format!("{}.fresh-grant.json", fixture.binding.handoff_id),
                    &grant,
                )
                .unwrap();
                durable_new(&dir, &format!("{}.consumed.json", grant.id), &grant).unwrap();
                if let Some(output) = output {
                    Fixture::effect_q(&dir, &grant, output);
                    effect_readback_from_dir(&dir, &intent).unwrap();
                }
                (dir, grant, intent)
            };
        make_physical(&quota, "quota-source", Some(b"invalid quota"));
        std::thread::sleep(std::time::Duration::from_millis(2));
        let auth = FreshAccountEffectRequest {
            kind: FreshAccountEffectKind::AuthRefresh,
            ..quota.clone()
        };
        let (source_dir, source_grant, source_intent) = make_physical(&auth, "auth-source", None);
        let pool = oulipoly_runtime::executor::cli::fresh_remote::load_fresh_headless_pool(
            &fixture.source,
            "other",
        )
        .unwrap();
        let binding = Binding {
            root_id: uuid::Uuid::new_v4().to_string(),
            handoff_id: uuid::Uuid::new_v4().to_string(),
            ..fixture.binding.clone()
        };
        let candidate = RouteCandidate {
            binding: binding.clone(),
            model: "other".into(),
            config_sha256: pool.config_sha256.clone(),
            ..fixture.candidate.clone()
        };
        let meta = fixture.source.metadata().unwrap();
        durable_new(
            &fixture.root,
            &format!("{}.route-source.json", binding.handoff_id),
            &RouteSource {
                version: 1,
                binding: binding.clone(),
                config_sha256: candidate.config_sha256.clone(),
                directory_device: meta.dev(),
                directory_inode: meta.ino(),
            },
        )
        .unwrap();
        durable_new(
            &fixture.root,
            &candidate_name(&binding.handoff_id, 0),
            &candidate,
        )
        .unwrap();
        fixture.ready();
        rebuild_keyed_offline(&fixture.root, &fixture.socket, &fixture.source).unwrap();
        let lease = broker_admission_lease(&fixture.root).unwrap();
        let generation =
            KeyedGeneration::admit_provider_readback(&fixture.root, &lease, &fixture.source)
                .unwrap();
        let source_key =
            source_key(&fixture.candidate, &environment_digest(&auth).unwrap()).unwrap();
        let peer = generation
            .auth_peer_intent("physical-first", &source_key)
            .unwrap()
            .unwrap();
        assert!(peer.path.ends_with("intent.json"));
        let follower = FreshAccountEffectRequest {
            d_key: uuid::Uuid::new_v4().to_string(),
            model: "other".into(),
            config_sha256: candidate.config_sha256.clone(),
            ..auth
        };
        let dir = effect_directory(&fixture.root, &binding, &follower);
        std::fs::create_dir_all(&dir).unwrap();
        let intent = AccountEffectIntent {
            version: 1,
            id: "auth-follower".into(),
            binding: binding.clone(),
            request: redacted_effect_request(&follower),
            environment_sha256: environment_digest(&follower).unwrap(),
            plan_sha256: "coalesced:auth-source".into(),
            auth_source: Some(AuthReuse {
                source_directory: source_dir
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned(),
                source_effect_id: "auth-source".into(),
            }),
        };
        durable_new(&dir, "intent.json", &intent).unwrap();
        generation
            .announce_auth_alias(&binding, &follower, &intent.id)
            .unwrap();
        assert_eq!(
            generation
                .observe_auth_alias(&binding, &follower, &intent.id)
                .unwrap()
                .state,
            "unknown"
        );
        Fixture::effect_q(&source_dir, &source_grant, b"");
        assert_eq!(
            generation
                .observe_auth_alias(&binding, &follower, &intent.id)
                .unwrap()
                .state,
            "unknown"
        );
        assert_eq!(
            generation
                .settle_quota_effect(
                    &fixture.binding,
                    &FreshAccountEffectRequest {
                        kind: FreshAccountEffectKind::AuthRefresh,
                        ..quota.clone()
                    },
                    &source_intent.id
                )
                .unwrap()
                .unwrap()
                .outcome
                .as_deref(),
            Some("refreshed")
        );
        let readback = generation
            .observe_auth_alias(&binding, &follower, &intent.id)
            .unwrap();
        assert_eq!(readback.outcome.as_deref(), Some("refreshed"));
        assert_eq!(readback.peer_effect_id.as_deref(), Some("auth-source"));
        assert!(!dir.join("result.json").exists());
        assert!(
            generation
                .auth_peer_intent("physical-first", &source_key)
                .unwrap()
                .is_some()
        );
        let changed_env = FreshAccountEffectRequest {
            environment: vec![("CHANGED".into(), "1".into())],
            ..follower.clone()
        };
        let changed_source =
            super::source_key(&candidate, &environment_digest(&changed_env).unwrap()).unwrap();
        assert!(
            generation
                .require_auth_source("physical-first", &changed_source)
                .is_err()
        );
        assert!(
            generation
                .require_auth_source("different", &source_key)
                .is_err()
        );
        assert!(
            generation
                .observe_auth_alias(&binding, &changed_env, &intent.id)
                .is_err()
        );
        let changed_account = FreshAccountEffectRequest {
            account: "different".into(),
            ..follower.clone()
        };
        assert!(
            generation
                .observe_auth_alias(&binding, &changed_account, &intent.id)
                .is_err()
        );
        let config = fixture.source.join("providers.toml");
        let original = std::fs::read(&config).unwrap();
        std::fs::write(
            &config,
            "[first]\ncommand = '/bin/true'\nquota_account_id = 'different'\n",
        )
        .unwrap();
        assert!(
            generation
                .observe_auth_alias(&binding, &follower, &intent.id)
                .is_err()
        );
        std::fs::write(&config, original).unwrap();
        KeyedGeneration::admit_provider_readback(&fixture.root, &lease, &fixture.source).unwrap();
    }

    #[test]
    fn v3_effect_checkpoint_and_quota_writer_io_stay_exact_with_hundreds_of_sources() {
        fn measured(
            count: usize,
            auth_mode: bool,
        ) -> Vec<(
            crate::linux_main::fresh_index::KeyedIoCount,
            crate::linux_main::fresh_index::ReaderIo,
        )> {
            let fixture = Fixture::new();
            let source_meta = fixture.source.metadata().unwrap();
            let mut target = None;
            for n in 0..count {
                let binding = Binding {
                    handoff_id: uuid::Uuid::new_v4().to_string(),
                    ..fixture.binding.clone()
                };
                let candidate = RouteCandidate {
                    binding: binding.clone(),
                    ..fixture.candidate.clone()
                };
                durable_new(
                    &fixture.root,
                    &format!("{}.route-source.json", binding.handoff_id),
                    &RouteSource {
                        version: 1,
                        binding: binding.clone(),
                        config_sha256: candidate.config_sha256.clone(),
                        directory_device: source_meta.dev(),
                        directory_inode: source_meta.ino(),
                    },
                )
                .unwrap();
                durable_new(
                    &fixture.root,
                    &candidate_name(&binding.handoff_id, 0),
                    &candidate,
                )
                .unwrap();
                let request = FreshAccountEffectRequest {
                    d_key: uuid::Uuid::new_v4().to_string(),
                    model: "work".into(),
                    config_sha256: candidate.config_sha256.clone(),
                    account: "first".into(),
                    index: 0,
                    kind: FreshAccountEffectKind::QuotaFirst,
                    environment: vec![("SOURCE".into(), n.to_string())],
                };
                let dir = effect_directory(&fixture.root, &binding, &request);
                std::fs::create_dir_all(&dir).unwrap();
                let grant = Grant {
                    version: 1,
                    id: uuid::Uuid::new_v4().to_string(),
                    binding: binding.clone(),
                    plan_sha256: "b".repeat(64),
                };
                let id = format!("effect-{n}");
                let intent = AccountEffectIntent {
                    version: 1,
                    id: id.clone(),
                    binding: binding.clone(),
                    request: redacted_effect_request(&request),
                    environment_sha256: environment_digest(&request).unwrap(),
                    plan_sha256: grant.plan_sha256.clone(),
                    auth_source: None,
                };
                durable_new(&dir, "intent.json", &intent).unwrap();
                durable_new(
                    &dir,
                    &format!("{}.fresh-grant.json", binding.handoff_id),
                    &grant,
                )
                .unwrap();
                durable_new(&dir, &format!("{}.consumed.json", grant.id), &grant).unwrap();
                Fixture::effect_q(
                    &dir,
                    &grant,
                    if auth_mode && n + 1 == count {
                        b"invalid quota"
                    } else {
                        b"{\"used_percent\":20,\"resets_at\":\"2099-01-01T00:00:00Z\"}"
                    },
                );
                effect_readback_from_dir(&dir, &intent).unwrap();
                target = Some((binding, request, id));
            }
            fixture.ready();
            rebuild_keyed_offline(&fixture.root, &fixture.socket, &fixture.source).unwrap();
            let lease = broker_admission_lease(&fixture.root).unwrap();
            let generation =
                KeyedGeneration::admit_provider_readback(&fixture.root, &lease, &fixture.source)
                    .unwrap();
            let (binding, request, id) = target.unwrap();
            let (result, keyed_io) = crate::linux_main::fresh_index::measure_keyed_io(|| {
                let _io =
                    crate::linux_main::fresh_index::ReaderIoGuard::start("v3-physical-effect-read");
                generation.latest_effect_checkpoint(&fixture.source, &binding, &request, &id)
            });
            assert_eq!(
                result.unwrap().unwrap().outcome.as_deref(),
                Some(if auth_mode {
                    "invalid"
                } else {
                    "valid_windows"
                })
            );
            let read = (
                keyed_io,
                crate::linux_main::fresh_index::last_reader_io().unwrap(),
            );
            let writer_binding = if auth_mode {
                binding.clone()
            } else {
                Binding {
                    handoff_id: uuid::Uuid::new_v4().to_string(),
                    ..fixture.binding.clone()
                }
            };
            let candidate = RouteCandidate {
                binding: writer_binding.clone(),
                ..fixture.candidate.clone()
            };
            if !auth_mode {
                durable_new(
                    &fixture.root,
                    &format!("{}.route-source.json", writer_binding.handoff_id),
                    &RouteSource {
                        version: 1,
                        binding: writer_binding.clone(),
                        config_sha256: candidate.config_sha256.clone(),
                        directory_device: source_meta.dev(),
                        directory_inode: source_meta.ino(),
                    },
                )
                .unwrap();
                durable_new(
                    &fixture.root,
                    &candidate_name(&writer_binding.handoff_id, 0),
                    &candidate,
                )
                .unwrap();
            }
            let writer_request = if auth_mode {
                FreshAccountEffectRequest {
                    kind: FreshAccountEffectKind::AuthRefresh,
                    ..request.clone()
                }
            } else {
                FreshAccountEffectRequest {
                    d_key: uuid::Uuid::new_v4().to_string(),
                    model: "work".into(),
                    config_sha256: candidate.config_sha256.clone(),
                    account: "first".into(),
                    index: 0,
                    kind: FreshAccountEffectKind::QuotaFirst,
                    environment: vec![("SOURCE".into(), "writer".into())],
                }
            };
            let dir = effect_directory(&fixture.root, &writer_binding, &writer_request);
            std::fs::create_dir_all(&dir).unwrap();
            let grant = Grant {
                version: 1,
                id: uuid::Uuid::new_v4().to_string(),
                binding: writer_binding.clone(),
                plan_sha256: "b".repeat(64),
            };
            let intent = AccountEffectIntent {
                version: 1,
                id: uuid::Uuid::new_v4().to_string(),
                binding: writer_binding.clone(),
                request: redacted_effect_request(&writer_request),
                environment_sha256: environment_digest(&writer_request).unwrap(),
                plan_sha256: grant.plan_sha256.clone(),
                auth_source: None,
            };
            durable_new(&dir, "intent.json", &intent).unwrap();
            let measured_step = |label: &'static str, run: &mut dyn FnMut()| {
                let (_, keyed) = crate::linux_main::fresh_index::measure_keyed_io(|| {
                    let _guard = crate::linux_main::fresh_index::ReaderIoGuard::start(label);
                    run();
                });
                (
                    keyed,
                    crate::linux_main::fresh_index::last_reader_io().unwrap(),
                )
            };
            let mut revision = 0;
            let announce = measured_step("v3-quota-announce-write", &mut || {
                revision = generation
                    .announce_quota_effect(&writer_binding, &writer_request, &intent.id)
                    .unwrap();
            });
            durable_new(
                &dir,
                &format!("{}.fresh-grant.json", writer_binding.handoff_id),
                &grant,
            )
            .unwrap();
            durable_new(&dir, &format!("{}.consumed.json", grant.id), &grant).unwrap();
            let consume = measured_step("v3-quota-K-write", &mut || {
                generation
                    .record_quota_k(&writer_binding, &writer_request, &intent.id, Some(revision))
                    .unwrap();
            });
            Fixture::effect_q(
                &dir,
                &grant,
                if auth_mode {
                    b""
                } else {
                    b"{\"used_percent\":20,\"resets_at\":\"2099-01-01T00:00:00Z\"}"
                },
            );
            let settle = measured_step("v3-quota-Q-settle-write", &mut || {
                let result = generation
                    .settle_quota_effect(&writer_binding, &writer_request, &intent.id)
                    .unwrap();
                assert_eq!(
                    result.unwrap().outcome.as_deref(),
                    Some(if auth_mode {
                        "refreshed"
                    } else {
                        "valid_windows"
                    })
                );
            });
            let mut measurements = vec![read, announce, consume, settle];
            if auth_mode {
                measurements.push(measured_step("v3-auth-exact-read", &mut || {
                    assert_eq!(
                        generation
                            .latest_effect_checkpoint(
                                &fixture.source,
                                &writer_binding,
                                &writer_request,
                                &intent.id
                            )
                            .unwrap()
                            .unwrap()
                            .outcome
                            .as_deref(),
                        Some("refreshed")
                    );
                }));
            }
            measurements
        }
        let small = measured(1, false);
        let large = measured(205, false);
        eprintln!("v3 physical effect read/announce/K/settle 1={small:?} 205={large:?}");
        for (one, many) in small.iter().zip(&large) {
            assert_eq!(one.0.open_attempts, many.0.open_attempts);
            assert_eq!(one.0.opened, many.0.opened);
            assert_eq!(one.1.open_attempts, many.1.open_attempts);
            assert_eq!(one.1.opened, many.1.opened);
            assert_eq!(one.0.directory_entries + one.1.directory_entries, 0);
            assert_eq!(many.0.directory_entries + many.1.directory_entries, 0);
        }
        assert_eq!(small[0].0.bytes_written + small[0].1.bytes_written, 0);
        assert_eq!(large[0].0.bytes_written + large[0].1.bytes_written, 0);
        for step in 1..=3 {
            assert!(small[step].0.bytes_written + small[step].1.bytes_written > 0);
            assert!(large[step].0.bytes_written + large[step].1.bytes_written > 0);
        }
        let small_auth = measured(1, true);
        let large_auth = measured(205, true);
        eprintln!(
            "v3 auth physical read/announce/K/settle/read 1={small_auth:?} 205={large_auth:?}"
        );
        for (one, many) in small_auth.iter().zip(&large_auth) {
            assert_eq!(one.0.open_attempts, many.0.open_attempts);
            assert_eq!(one.0.opened, many.0.opened);
            assert_eq!(one.1.open_attempts, many.1.open_attempts);
            assert_eq!(one.1.opened, many.1.opened);
            assert_eq!(one.0.directory_entries + one.1.directory_entries, 0);
            assert_eq!(many.0.directory_entries + many.1.directory_entries, 0);
        }
        assert_eq!(
            small_auth[0].0.bytes_written + small_auth[0].1.bytes_written,
            0
        );
        assert_eq!(
            large_auth[4].0.bytes_written + large_auth[4].1.bytes_written,
            0
        );
        for step in 1..=3 {
            assert!(small_auth[step].0.bytes_written + small_auth[step].1.bytes_written > 0);
            assert!(large_auth[step].0.bytes_written + large_auth[step].1.bytes_written > 0);
        }
    }

    #[test]
    fn v3_physical_quota_result_keeps_fresh_stale_full_and_invalid_distinct() {
        for (output, outcome, eligibility) in [
            (
                b"{\"used_percent\":20,\"resets_at\":\"2099-01-01T00:00:00Z\"}".as_slice(),
                "valid_windows",
                "Eligible",
            ),
            (
                b"{\"used_percent\":100,\"resets_at\":\"2099-01-01T00:00:00Z\"}".as_slice(),
                "valid_windows",
                "Excluded",
            ),
            (b"invalid quota Q".as_slice(), "invalid", "ProbeRequired"),
        ] {
            let fixture = Fixture::new();
            let request = FreshAccountEffectRequest {
                d_key: uuid::Uuid::new_v4().to_string(),
                model: "work".into(),
                config_sha256: fixture.candidate.config_sha256.clone(),
                account: "first".into(),
                index: 0,
                kind: FreshAccountEffectKind::QuotaFirst,
                environment: vec![],
            };
            let dir = effect_directory(&fixture.root, &fixture.binding, &request);
            std::fs::create_dir_all(&dir).unwrap();
            let grant = Grant {
                version: 1,
                id: uuid::Uuid::new_v4().to_string(),
                binding: fixture.binding.clone(),
                plan_sha256: "b".repeat(64),
            };
            let intent = AccountEffectIntent {
                version: 1,
                id: "quota".into(),
                binding: fixture.binding.clone(),
                request: request.clone(),
                environment_sha256: environment_digest(&request).unwrap(),
                plan_sha256: grant.plan_sha256.clone(),
                auth_source: None,
            };
            durable_new(&dir, "intent.json", &intent).unwrap();
            durable_new(
                &dir,
                &format!("{}.fresh-grant.json", fixture.binding.handoff_id),
                &grant,
            )
            .unwrap();
            durable_new(&dir, &format!("{}.consumed.json", grant.id), &grant).unwrap();
            Fixture::effect_q(&dir, &grant, output);
            effect_readback_from_dir(&dir, &intent).unwrap();
            fixture.ready();
            rebuild_keyed_offline(&fixture.root, &fixture.socket, &fixture.source).unwrap();
            let generation = KeyedGeneration::open(&fixture.root).unwrap();
            let result = generation
                .latest_effect_checkpoint(&fixture.source, &fixture.binding, &request, "quota")
                .unwrap()
                .unwrap();
            assert_eq!(result.outcome.as_deref(), Some(outcome));
            let indexed: EffectIntent = serde_json::from_value(
                generation
                    .account("physical-first")
                    .unwrap()
                    .get("effect", "quota")
                    .unwrap()
                    .unwrap(),
            )
            .unwrap();
            let now = Utc::now().timestamp();
            let actual = generation
                .route_facts(
                    "physical-first",
                    Some(&indexed.source),
                    "work",
                    &fixture.candidate.config_sha256,
                    now,
                )
                .unwrap();
            assert!(format!("{actual:?}").starts_with(eligibility));
            if outcome == "valid_windows" && eligibility == "Eligible" {
                let stale = generation
                    .route_facts(
                        "physical-first",
                        Some(&indexed.source),
                        "work",
                        &fixture.candidate.config_sha256,
                        now + 5 * 60 * 60 + 1,
                    )
                    .unwrap();
                assert_eq!(format!("{stale:?}"), "ProbeRequired");
            }
        }
    }

    #[test]
    fn v3_uncertain_provider_q_remains_keyed_debt() {
        let fixture = Fixture::new();
        fixture.consumed();
        fixture.certified_q(b"done");
        std::fs::remove_file(
            fixture
                .root
                .join(format!("{}.terminal.json", fixture.grant.id)),
        )
        .unwrap();
        fixture.ready();
        rebuild_keyed_offline(&fixture.root, &fixture.socket, &fixture.source).unwrap();
        let account = KeyedGeneration::open(&fixture.root)
            .unwrap()
            .account("physical-first")
            .unwrap();
        assert_eq!(account.summary().unwrap().1, 1);
        assert!(
            account
                .get("pending", &format!("grant:{}", fixture.grant.id))
                .unwrap()
                .is_some()
        );
        assert!(
            account
                .get("uncertain-q", &format!("grant:{}", fixture.grant.id))
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn v3_refuses_missing_v2_announcement_without_replacing_manifest() {
        let fixture = Fixture::new();
        fixture.prepared();
        fixture.ready();
        let v2 = fixture.rebuild().unwrap();
        let prior = v2.generation().to_owned();
        std::fs::remove_file(
            fixture
                .root
                .join(format!("{}.fresh-grant.json", fixture.binding.handoff_id)),
        )
        .unwrap();
        assert!(rebuild_keyed_offline(&fixture.root, &fixture.socket, &fixture.source).is_err());
        assert_eq!(Index::open(&fixture.root).unwrap().generation(), prior);
    }

    #[test]
    fn v1_manifest_refuses_live_open_until_frozen_v2_rebuild() {
        let fixture = Fixture::new();
        fixture.prepared();
        fixture.ready();
        let first = fixture.rebuild().unwrap();
        assert_eq!(
            first
                .compact_account("physical-first")
                .unwrap()
                .pending
                .len(),
            1
        );
        Index::downgrade_manifest_for_migration_test(&fixture.root).unwrap();
        assert!(matches!(
            Index::open(&fixture.root),
            Err(crate::linux_main::fresh_index::IndexError::RebuildRequired(
                _
            ))
        ));
        let migrated = fixture.rebuild().unwrap();
        assert_ne!(first.generation(), migrated.generation());
        assert_eq!(
            migrated
                .compact_account("physical-first")
                .unwrap()
                .pending
                .len(),
            1
        );
    }

    #[test]
    fn consumed_k_without_q_and_q_arriving_after_scan_stay_unresolved() {
        let fixture = Fixture::new();
        fixture.consumed();
        fixture.ready();
        let index = Index::rebuild_offline_test_hook(
            &fixture.root,
            &fixture.socket,
            &fixture.source,
            || {
                std::fs::write(
                    fixture
                        .root
                        .join(format!("{}.drain.json", fixture.grant.id)),
                    b"uncertified Q",
                )
                .unwrap();
            },
        )
        .unwrap();
        let account = index.account("physical-first").unwrap();
        assert_eq!(account.observed_invocations, 1);
        assert!(account.grants[&fixture.grant.id].consumed_k.is_some());
        assert!(account.grants[&fixture.grant.id].certified_q.is_none());
        assert!(
            index
                .reconcile_offline_account("physical-first", &fixture.source)
                .is_err()
        );
        std::fs::remove_file(
            fixture
                .root
                .join(format!("{}.drain.json", fixture.grant.id)),
        )
        .unwrap();
        assert!(
            index
                .reconcile_offline_account("physical-first", &fixture.source)
                .unwrap()
                .grants
                .contains_key(&fixture.grant.id)
        );
    }

    #[test]
    fn certified_physical_q_stays_debt_until_exact_reconcile_and_typed_marker() {
        let fixture = Fixture::new();
        fixture.consumed();
        fixture.certified_q(b"{\"type\":\"error\",\"error\":{\"code\":\"model_at_capacity\"}}\n");
        fixture.ready();
        let index = fixture.rebuild().unwrap();
        let before = index.account("physical-first").unwrap();
        assert!(before.grants.contains_key(&fixture.grant.id));
        assert!(before.grants[&fixture.grant.id].certified_q.is_some());
        assert!(before.markers.model_capacity_nanos.is_some());
        let after = index
            .reconcile_offline_account("physical-first", &fixture.source)
            .unwrap();
        assert_eq!(
            after.grants[&fixture.grant.id].certified_q,
            before.grants[&fixture.grant.id].certified_q
        );
        assert_eq!(after.observed_invocations, 1);
        assert_eq!(
            after.markers.model_capacity_nanos,
            before.markers.model_capacity_nanos
        );
        assert!(
            Index::open(&fixture.root)
                .unwrap()
                .account("physical-first")
                .unwrap()
                .grants[&fixture.grant.id]
                .certified_q
                .is_some()
        );
    }

    #[test]
    fn forged_terminal_or_q_refuses_publication() {
        let fixture = Fixture::new();
        fixture.consumed();
        fixture.certified_q(b"ok\n");
        fixture.ready();
        let terminal_path = fixture
            .root
            .join(format!("{}.terminal.json", fixture.grant.id));
        let mut terminal: TerminalRecord = exact_file(
            &fixture.root,
            terminal_path.file_name().unwrap().to_str().unwrap(),
        )
        .unwrap()
        .unwrap();
        terminal.signal_kind = "forged".into();
        std::fs::write(&terminal_path, serde_json::to_vec(&terminal).unwrap()).unwrap();
        assert!(fixture.rebuild().is_err());
        assert!(!fixture.root.join("index-v1/manifest.json").exists());
    }

    #[test]
    fn invalid_history_never_publishes_manifest() {
        let fixture = Fixture::new();
        fixture.prepared();
        fixture.ready();
        let decision_path = fixture
            .root
            .join(decision_name(&fixture.binding.handoff_id));
        std::fs::write(&decision_path, b"{broken").unwrap();
        assert!(fixture.rebuild().is_err());
        assert!(!fixture.root.join("index-v1/manifest.json").exists());
        let valid = Fixture::new();
        valid.prepared();
        valid.ready();
        std::fs::remove_file(valid.root.join(decision_name(&valid.binding.handoff_id))).unwrap();
        assert!(valid.rebuild().is_err());
        assert!(!valid.root.join("index-v1/manifest.json").exists());
        let missing_without_grant = Fixture::new();
        missing_without_grant.ready();
        std::fs::remove_file(
            missing_without_grant
                .root
                .join(decision_name(&missing_without_grant.binding.handoff_id)),
        )
        .unwrap();
        assert!(missing_without_grant.rebuild().is_err());
        let changed = Fixture::new();
        changed.prepared();
        changed.ready();
        let mut candidate = changed.candidate.clone();
        candidate.account_identity = "other".into();
        std::fs::write(
            changed
                .root
                .join(candidate_name(&changed.binding.handoff_id, 0)),
            serde_json::to_vec(&candidate).unwrap(),
        )
        .unwrap();
        assert!(changed.rebuild().is_err());
        assert!(!changed.root.join("index-v1/manifest.json").exists());
    }

    #[test]
    fn duplicate_sequence_and_changed_source_refuse_before_cutover() {
        let fixture = Fixture::new();
        fixture.ready();
        let second = uuid::Uuid::new_v4().to_string();
        let mut binding = fixture.binding.clone();
        binding.handoff_id = second.clone();
        let mut candidate = fixture.candidate.clone();
        candidate.binding = binding.clone();
        let meta = fixture.source.metadata().unwrap();
        durable_new(
            &fixture.root,
            &format!("{second}.route-source.json"),
            &RouteSource {
                version: 1,
                binding: binding.clone(),
                config_sha256: candidate.config_sha256.clone(),
                directory_device: meta.dev(),
                directory_inode: meta.ino(),
            },
        )
        .unwrap();
        durable_new(&fixture.root, &candidate_name(&second, 0), &candidate).unwrap();
        let mut decision: RouteDecision =
            exact_file(&fixture.root, &decision_name(&fixture.binding.handoff_id))
                .unwrap()
                .unwrap();
        decision.binding = binding;
        durable_new(&fixture.root, &decision_name(&second), &decision).unwrap();
        assert!(fixture.rebuild().is_err());
        assert!(!fixture.root.join("index-v1/manifest.json").exists());
        std::fs::remove_file(fixture.root.join(decision_name(&second))).unwrap();
        std::fs::remove_file(fixture.root.join(candidate_name(&second, 0))).unwrap();
        std::fs::remove_file(fixture.root.join(format!("{second}.route-source.json"))).unwrap();
        std::fs::write(
            fixture.source.join("models/work.toml"),
            "[[providers]]\nname = 'first'\n# changed\n",
        )
        .unwrap();
        assert!(fixture.rebuild().is_err());
    }

    #[test]
    fn source_edit_between_scan_and_manifest_refuses() {
        let fixture = Fixture::new();
        fixture.ready();
        let result = Index::rebuild_offline_test_hook(
            &fixture.root,
            &fixture.socket,
            &fixture.source,
            || {
                std::fs::write(
                    fixture.source.join("models/work.toml"),
                    "[[providers]]\nname = 'first'\n# changed during rebuild\n",
                )
                .unwrap();
            },
        );
        assert!(result.is_err());
        assert!(!fixture.root.join("index-v1/manifest.json").exists());
    }

    #[test]
    fn admission_lease_refuses_concurrent_writer_and_rebuild() {
        let fixture = Fixture::new();
        assert!(fixture.rebuild().is_err());
        assert!(!fixture.root.join("index-v1").exists());
        let live = broker_admission_lease(&fixture.root).unwrap();
        let lock_path = fixture.root.join("index-v1/admission.lock");
        assert!(!other_process_can_freeze(&lock_path));
        assert!(fixture.rebuild().is_err());
        assert!(!other_process_can_freeze(&lock_path));
        assert!(!fixture.root.join("index-v1/manifest.json").exists());
        drop(live);
        assert!(other_process_can_freeze(&lock_path));
        let listener = std::os::unix::net::UnixListener::bind(&fixture.socket).unwrap();
        assert!(fixture.rebuild().is_err());
        drop(listener);
        std::fs::remove_file(&fixture.socket).unwrap();
        let index = Index::rebuild_offline_test_hook(
            &fixture.root,
            &fixture.socket,
            &fixture.source,
            || {
                assert!(broker_admission_lease(&fixture.root).is_err());
            },
        )
        .unwrap();
        assert!(Index::open(&fixture.root).is_ok());
        assert_eq!(
            index.generation(),
            Index::open(&fixture.root).unwrap().generation()
        );
    }

    #[test]
    fn uncommitted_generation_is_invisible_and_published_damage_refuses_open() {
        let fixture = Fixture::new();
        fixture.ready();
        let orphan = fixture
            .root
            .join("index-v1/generations/crashed-before-manifest");
        std::fs::create_dir_all(&orphan).unwrap();
        assert!(Index::open(&fixture.root).is_err());
        let index = fixture.rebuild().unwrap();
        let generation = index.generation().to_owned();
        assert!(orphan.is_dir());
        assert!(Index::open(&fixture.root).is_ok());
        std::fs::remove_dir_all(
            fixture
                .root
                .join("index-v1/generations")
                .join(&generation)
                .join("accounts"),
        )
        .unwrap();
        assert!(Index::open(&fixture.root).is_err());
        assert!(
            fixture
                .root
                .join(decision_name(&fixture.binding.handoff_id))
                .exists()
        );
    }

    #[test]
    fn manual_unresolved_k_is_retained() {
        let fixture = Fixture::new();
        let operation_id = uuid::Uuid::new_v4().to_string();
        let request = ManualQuotaRequest {
            operation_id: operation_id.clone(),
            model: "work".into(),
            account: "first".into(),
            config_sha256: fixture.candidate.config_sha256.clone(),
            environment: vec![],
        };
        let manual = crate::linux_main::manual_quota::begin(
            &fixture.root,
            &File::open(&fixture.source).unwrap(),
            &request,
            1000,
            1000,
        )
        .unwrap();
        let effect_request = FreshAccountEffectRequest {
            d_key: uuid::Uuid::new_v4().to_string(),
            model: "work".into(),
            config_sha256: fixture.candidate.config_sha256.clone(),
            account: "first".into(),
            index: 0,
            kind: FreshAccountEffectKind::QuotaFirst,
            environment: vec![],
        };
        let effect_dir = effect_directory(&fixture.root, &fixture.binding, &effect_request);
        std::fs::create_dir_all(&effect_dir).unwrap();
        let source_effect_id = manual.effect_id.unwrap();
        durable_new(
            &effect_dir,
            "intent.json",
            &AccountEffectIntent {
                version: 1,
                id: uuid::Uuid::new_v4().to_string(),
                binding: fixture.binding.clone(),
                request: effect_request,
                environment_sha256: environment_digest(&FreshAccountEffectRequest {
                    d_key: String::new(),
                    model: "work".into(),
                    config_sha256: fixture.candidate.config_sha256.clone(),
                    account: "first".into(),
                    index: 0,
                    kind: FreshAccountEffectKind::QuotaFirst,
                    environment: vec![],
                })
                .unwrap(),
                plan_sha256: format!("manual:{source_effect_id}"),
                auth_source: None,
            },
        )
        .unwrap();
        durable_new(
            &effect_dir,
            "manual-reuse.json",
            &ManualQuotaReuse {
                operation_id: operation_id.clone(),
                source_effect_id,
            },
        )
        .unwrap();
        fixture.ready();
        let index = fixture.rebuild().unwrap();
        let account = index.account("physical-first").unwrap();
        assert!(account.effects[&operation_id].consumed_k.is_some());
        assert_eq!(account.effects[&operation_id].kind, EffectKind::ManualQuota);
        assert_eq!(account.effects.len(), 2);
        std::fs::write(effect_dir.join("manual-reuse.json"), b"changed").unwrap();
        assert!(index.account("physical-first").is_err());
    }

    #[test]
    fn manual_physical_q_is_certified_then_reconciled_from_unresolved_k() {
        let fixture = Fixture::new();
        let operation_id = uuid::Uuid::new_v4().to_string();
        let request = ManualQuotaRequest {
            operation_id: operation_id.clone(),
            model: "work".into(),
            account: "first".into(),
            config_sha256: fixture.candidate.config_sha256.clone(),
            environment: vec![],
        };
        crate::linux_main::manual_quota::begin(
            &fixture.root,
            &File::open(&fixture.source).unwrap(),
            &request,
            unsafe { libc::geteuid() },
            unsafe { libc::getegid() },
        )
        .unwrap();
        crate::linux_main::manual_quota::worker_with_environment(
            &fixture.root.join("manual-quota").join(&operation_id),
            &[],
        )
        .unwrap();
        fixture.ready();
        let index = fixture.rebuild().unwrap();
        let before = index.account("physical-first").unwrap();
        assert!(before.effects[&operation_id].consumed_k.is_some());
        assert!(before.effects[&operation_id].certified_q.is_some());
        let after = index
            .reconcile_offline_account("physical-first", &fixture.source)
            .unwrap();
        assert!(after.effects[&operation_id].certified_q.is_some());
        assert!(after.effects[&operation_id].result.is_some());
        assert_eq!(after.source_q.len(), 1);
        assert!(
            after
                .source_q
                .values()
                .next()
                .unwrap()
                .latest_quota_q
                .is_some()
        );
    }

    #[test]
    fn manual_q_after_frozen_rebuild_keeps_exact_identity() {
        let fixture = Fixture::new();
        let operation_id = uuid::Uuid::new_v4().to_string();
        let request = ManualQuotaRequest {
            operation_id: operation_id.clone(),
            model: "work".into(),
            account: "first".into(),
            config_sha256: fixture.candidate.config_sha256.clone(),
            environment: vec![],
        };
        crate::linux_main::manual_quota::begin(
            &fixture.root,
            &File::open(&fixture.source).unwrap(),
            &request,
            unsafe { libc::geteuid() },
            unsafe { libc::getegid() },
        )
        .unwrap();
        fixture.ready();
        let index = fixture.rebuild().unwrap();
        assert!(
            index.account("physical-first").unwrap().effects[&operation_id]
                .certified_q
                .is_none()
        );
        crate::linux_main::manual_quota::worker_with_environment(
            &fixture.root.join("manual-quota").join(&operation_id),
            &[],
        )
        .unwrap();
        let account = index
            .reconcile_offline_account("physical-first", &fixture.source)
            .unwrap();
        let effect = &account.effects[&operation_id];
        assert!(effect.certified_q.is_some());
        assert_eq!(
            effect.result.as_ref(),
            effect.certified_q.as_ref().map(|q| &q.q)
        );
        assert_eq!(account.source_q.len(), 1);
    }

    #[test]
    fn quota_and_auth_physical_q_are_certified_and_reconciled() {
        let fixture = Fixture::new();
        for (kind, id, plan, output_bytes) in [
            (
                FreshAccountEffectKind::QuotaFirst,
                "quota",
                "b".repeat(64),
                b"{\"used_percent\":20,\"resets_at\":\"2099-01-01T00:00:00Z\"}".as_slice(),
            ),
            (
                FreshAccountEffectKind::AuthRefresh,
                "auth",
                "c".repeat(64),
                b"".as_slice(),
            ),
        ] {
            let request = FreshAccountEffectRequest {
                d_key: uuid::Uuid::new_v4().to_string(),
                model: "work".into(),
                config_sha256: fixture.candidate.config_sha256.clone(),
                account: "first".into(),
                index: 0,
                kind,
                environment: vec![],
            };
            let dir = effect_directory(&fixture.root, &fixture.binding, &request);
            std::fs::create_dir_all(&dir).unwrap();
            let grant = Grant {
                version: 1,
                id: uuid::Uuid::new_v4().to_string(),
                binding: fixture.binding.clone(),
                plan_sha256: plan.clone(),
            };
            durable_new(
                &dir,
                "intent.json",
                &AccountEffectIntent {
                    version: 1,
                    id: id.into(),
                    binding: fixture.binding.clone(),
                    request: request.clone(),
                    environment_sha256: environment_digest(&request).unwrap(),
                    plan_sha256: plan,
                    auth_source: None,
                },
            )
            .unwrap();
            durable_new(
                &dir,
                &format!("{}.fresh-grant.json", fixture.binding.handoff_id),
                &grant,
            )
            .unwrap();
            durable_new(&dir, &format!("{}.consumed.json", grant.id), &grant).unwrap();
            Fixture::effect_q(&dir, &grant, output_bytes);
        }
        fixture.ready();
        let index = fixture.rebuild().unwrap();
        let before = index.account("physical-first").unwrap();
        assert_eq!(before.effects.len(), 2);
        assert!(
            before
                .effects
                .values()
                .all(|effect| effect.consumed_k.is_some() && effect.certified_q.is_none())
        );
        let after = index
            .reconcile_offline_account("physical-first", &fixture.source)
            .unwrap();
        assert_eq!(after.effects.len(), 2);
        assert!(after.source_q.is_empty());
        let lease = broker_admission_lease(&fixture.root).unwrap();
        let live = Index::admit_live_routes(&fixture.root, &lease).unwrap();
        let after = live.account("physical-first").unwrap();
        assert!(after.effects.values().all(|effect| effect.result.is_some()));
        assert_eq!(after.source_q.len(), 1);
        let q = after.source_q.values().next().unwrap();
        assert!(q.latest_quota_q.is_some());
        assert!(q.latest_auth_q.is_some());
    }

    #[test]
    fn pinned_decision_does_not_advance_and_quota_auth_references_survive() {
        let fixture = Fixture::new();
        std::fs::remove_file(
            fixture
                .root
                .join(decision_name(&fixture.binding.handoff_id)),
        )
        .unwrap();
        fixture.decision(0, true);
        let make_request = |kind| FreshAccountEffectRequest {
            d_key: uuid::Uuid::new_v4().to_string(),
            model: "work".into(),
            config_sha256: fixture.candidate.config_sha256.clone(),
            account: "first".into(),
            index: 0,
            kind,
            environment: vec![],
        };
        let first = make_request(FreshAccountEffectKind::QuotaFirst);
        let first_dir = effect_directory(&fixture.root, &fixture.binding, &first);
        std::fs::create_dir_all(&first_dir).unwrap();
        durable_new(
            &first_dir,
            "intent.json",
            &AccountEffectIntent {
                version: 1,
                id: "quota-source".into(),
                binding: fixture.binding.clone(),
                request: first.clone(),
                environment_sha256: environment_digest(&first).unwrap(),
                plan_sha256: "b".repeat(64),
                auth_source: None,
            },
        )
        .unwrap();
        let mut follower_binding = fixture.binding.clone();
        follower_binding.handoff_id = uuid::Uuid::new_v4().to_string();
        let mut follower_candidate = fixture.candidate.clone();
        follower_candidate.binding = follower_binding.clone();
        let meta = fixture.source.metadata().unwrap();
        durable_new(
            &fixture.root,
            &format!("{}.route-source.json", follower_binding.handoff_id),
            &RouteSource {
                version: 1,
                binding: follower_binding.clone(),
                config_sha256: follower_candidate.config_sha256.clone(),
                directory_device: meta.dev(),
                directory_inode: meta.ino(),
            },
        )
        .unwrap();
        durable_new(
            &fixture.root,
            &candidate_name(&follower_binding.handoff_id, 0),
            &follower_candidate,
        )
        .unwrap();
        let retry = make_request(FreshAccountEffectKind::QuotaRetry);
        let retry_dir = effect_directory(&fixture.root, &follower_binding, &retry);
        std::fs::create_dir_all(&retry_dir).unwrap();
        durable_new(
            &retry_dir,
            "intent.json",
            &AccountEffectIntent {
                version: 1,
                id: "quota-follower".into(),
                binding: follower_binding,
                request: retry.clone(),
                environment_sha256: environment_digest(&retry).unwrap(),
                plan_sha256: "reused:quota-source".into(),
                auth_source: None,
            },
        )
        .unwrap();
        durable_new(
            &retry_dir,
            "reuse.json",
            &QuotaReuse {
                source_directory: first_dir
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned(),
                source_effect_id: "quota-source".into(),
            },
        )
        .unwrap();
        let auth = make_request(FreshAccountEffectKind::AuthRefresh);
        let auth_dir = effect_directory(&fixture.root, &fixture.binding, &auth);
        std::fs::create_dir_all(&auth_dir).unwrap();
        durable_new(
            &auth_dir,
            "intent.json",
            &AccountEffectIntent {
                version: 1,
                id: "auth-source".into(),
                binding: fixture.binding.clone(),
                request: auth.clone(),
                environment_sha256: environment_digest(&auth).unwrap(),
                plan_sha256: "c".repeat(64),
                auth_source: None,
            },
        )
        .unwrap();
        fixture.ready();
        let index = fixture.rebuild().unwrap();
        let cursor = index
            .cursor(&crate::linux_main::fresh_index::CursorKey {
                model: "work".into(),
                config_sha256: fixture.candidate.config_sha256.clone(),
            })
            .unwrap();
        assert_eq!(cursor.sequence, 0);
        let effects = index.account("physical-first").unwrap().effects;
        assert!(effects.contains_key("quota-source"));
        assert!(effects.contains_key("quota-follower"));
        assert!(effects.contains_key("auth-source"));
        assert!(effects["quota-follower"].reuse.is_some());
    }
}
