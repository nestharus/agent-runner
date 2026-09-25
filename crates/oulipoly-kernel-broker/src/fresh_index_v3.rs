//! Frozen v3 importer and a separate private, readback-only admission. The v2
//! Index deliberately refuses this manifest. No v3 route or K writer exists.
use super::*;
use keyed_store::{Change, KeyedAccountStore};

const V3: u32 = 3;

#[derive(Debug)]
pub(crate) struct KeyedGeneration {
    pub generation: String,
    root: PathBuf,
    storage: PathBuf,
}

impl KeyedGeneration {
    pub(crate) fn open(root: &Path) -> Result<Self> {
        let manifest: Manifest = read(&root.join("index-v1/manifest.json"))?
            .ok_or(IndexError::RebuildRequired("v3 manifest absent"))?;
        if manifest.version != V3 || !manifest.generation_dir {
            return Err(IndexError::RebuildRequired("v3 manifest not published"));
        }
        let id = uuid::Uuid::parse_str(&manifest.generation)
            .map_err(|_| corrupt("v3 generation UUID invalid"))?;
        if id.is_nil() || id.to_string() != manifest.generation {
            return Err(corrupt("v3 generation identity invalid"));
        }
        let storage = root.join("index-v1/generations").join(&manifest.generation);
        validate_storage(&storage, &manifest.generation)?;
        Ok(Self {
            generation: manifest.generation,
            root: root.to_owned(),
            storage,
        })
    }

    pub(crate) fn account(&self, physical_key: &str) -> Result<KeyedAccountStore> {
        let path = self
            .storage
            .join("keyed-accounts")
            .join(keyed(&physical_key)?);
        if !path.is_dir() {
            return Err(IndexError::RebuildRequired("v3 physical account absent"));
        }
        let store = KeyedAccountStore::open(&path, &self.generation, physical_key)?;
        store.summary()?;
        Ok(store)
    }

    /// The broker holds this generation's lifetime lease before socket bind.
    /// A fresh retained census is compared with every keyed live projection;
    /// a late physical K/Q or an incompatible writer requires another frozen
    /// rebuild. This mode does not authorize a new physical provider K.
    pub(crate) fn admit_provider_readback(
        root: &Path,
        lease: &AdmissionLease,
        source: &Path,
    ) -> Result<Self> {
        if lease.path != root.canonicalize()?.join("index-v1/admission.lock") {
            return Err(IndexError::Conflict("v3 admission lease differs"));
        }
        let generation = Self::open(root)?;
        generation.verify_retained(source)?;
        Ok(generation)
    }

    fn check_current(&self) -> Result<()> {
        let manifest: Manifest = read(&self.root.join("index-v1/manifest.json"))?
            .ok_or(IndexError::RebuildRequired("v3 manifest disappeared"))?;
        if manifest.version != V3
            || manifest.generation != self.generation
            || !manifest.generation_dir
        {
            return Err(IndexError::RebuildRequired("v3 generation changed"));
        }
        Ok(())
    }

    pub(crate) fn require_route(&self, handoff: &str) -> Result<Decision> {
        self.check_current()?;
        let index = Index {
            root: self.root.clone(),
            generation: self.generation.clone(),
            storage: self.storage.clone(),
            route_reader_probe: false,
        };
        let decision = index
            .read_decision(handoff)?
            .ok_or(IndexError::RebuildRequired("v3 provider decision absent"))?;
        if !index.known("decisions", &handoff.to_owned())? {
            return Err(corrupt("v3 provider decision known key absent"));
        }
        super::super::fresh_provider::validate_indexed_receipt(&self.root, &decision)
            .map_err(|error| corrupt(format!("v3 provider route receipt: {error}")))?;
        Ok(decision)
    }

    pub(crate) fn provider_grant(&self, account: &str, id: &str) -> Result<Option<ProviderGrant>> {
        self.check_current()?;
        self.account(account)?
            .get("grant", id)?
            .map(|value| serde_json::from_value(value).map_err(|error| corrupt(error.to_string())))
            .transpose()
    }

    pub(crate) fn provider_pending(&self, account: &str, id: &str) -> Result<Option<PendingHead>> {
        self.check_current()?;
        self.account(account)?
            .get("pending", &format!("grant:{id}"))?
            .map(|value| serde_json::from_value(value).map_err(|error| corrupt(error.to_string())))
            .transpose()
    }

    fn verify_retained(&self, source: &Path) -> Result<()> {
        let source_before = source.metadata()?;
        if !source_before.is_dir() {
            return Err(corrupt("v3 admission config source not directory"));
        }
        let mut snapshot = super::super::fresh_provider::offline_snapshot_v3(&self.root, source)
            .map_err(|error| corrupt(format!("v3 admission retained evidence: {error}")))?;
        super::super::fresh_provider::complete_v3_effects(&self.root, &mut snapshot)
            .map_err(|error| corrupt(format!("v3 admission effect evidence: {error}")))?;
        let models: BTreeMap<String, String> = read(&self.storage.join("source-models.json"))?
            .ok_or(IndexError::RebuildRequired("v3 source model ledger absent"))?;
        if models != snapshot.source_models {
            return Err(IndexError::RebuildRequired(
                "v3 source model ledger differs",
            ));
        }
        for (model, digest) in &models {
            let pool = oulipoly_runtime::executor::cli::fresh_remote::load_fresh_headless_pool(
                source, model,
            )
            .map_err(|error| corrupt(format!("v3 admission config readback: {error}")))?;
            if &pool.config_sha256 != digest {
                return Err(corrupt("v3 admission config source changed"));
            }
        }
        let catalog: std::collections::HashSet<String> =
            fs::read_dir(self.storage.join("account-catalog"))?
                .map(|entry| entry.map(|entry| entry.file_name().to_string_lossy().into_owned()))
                .collect::<io::Result<_>>()?;
        let expected_catalog: std::collections::HashSet<String> = snapshot
            .accounts
            .keys()
            .map(|key| keyed(key).map(|digest| format!("{digest}.json")))
            .collect::<Result<_>>()?;
        if catalog != expected_catalog {
            return Err(IndexError::RebuildRequired(
                "v3 retained account catalog differs",
            ));
        }
        for decision in &snapshot.decisions {
            let indexed = self.require_route(&decision.handoff)?;
            if indexed.key != decision.key
                || indexed.candidate_identity != decision.candidate_identity
                || indexed.candidate_index != decision.candidate_index
                || indexed.pin != decision.pin
                || indexed.sequence != decision.sequence
                || indexed.receipt.as_ref() != Some(&decision.receipt)
            {
                return Err(corrupt("v3 retained route decision differs"));
            }
        }
        let decisions = fs::read_dir(self.storage.join("decisions"))?
            .collect::<io::Result<Vec<_>>>()?
            .into_iter()
            .filter(|entry| {
                entry.file_name().to_string_lossy().ends_with(".json")
                    && !entry.file_name().to_string_lossy().ends_with(".known.json")
            })
            .count();
        if decisions != snapshot.decisions.len() {
            return Err(IndexError::RebuildRequired(
                "v3 route decision count differs",
            ));
        }
        let index = Index {
            root: self.root.clone(),
            generation: self.generation.clone(),
            storage: self.storage.clone(),
            route_reader_probe: false,
        };
        let mut expected_cursors = BTreeMap::<String, Cursor>::new();
        let mut ordered = snapshot.decisions.clone();
        ordered.sort_by_key(|decision| decision.sequence);
        for decision in ordered {
            let digest = keyed(&decision.key)?;
            let cursor = expected_cursors.entry(digest).or_insert_with(|| Cursor {
                generation: self.generation.clone(),
                key: decision.key.clone(),
                sequence: 0,
                index: None,
                last_handoff: None,
            });
            let published = index
                .read_decision(&decision.handoff)?
                .ok_or(IndexError::RebuildRequired("v3 route decision absent"))?;
            *cursor = index.advanced(cursor, &published)?;
        }
        let cursor_files = fs::read_dir(self.storage.join("cursors"))?
            .collect::<io::Result<Vec<_>>>()?
            .into_iter()
            .filter(|entry| {
                entry.file_name().to_string_lossy().ends_with(".json")
                    && !entry.file_name().to_string_lossy().ends_with(".known.json")
            })
            .count();
        if cursor_files
            != expected_cursors
                .values()
                .filter(|cursor| cursor.sequence != 0)
                .count()
        {
            return Err(IndexError::RebuildRequired("v3 route cursor count differs"));
        }
        for cursor in expected_cursors
            .values()
            .filter(|cursor| cursor.sequence != 0)
        {
            if !index.known("cursors", &cursor.key)?
                || index.cursor_unlocked(&cursor.key)? != *cursor
            {
                return Err(corrupt("v3 route cursor differs from retained decisions"));
            }
        }
        for (key, mut account) in snapshot.accounts {
            account.generation = self.generation.clone();
            account.revision = account.revision.max(1);
            let head = AccountHead::from_account_unbounded(&self.root, &account)?;
            let store = self.account(&key)?;
            let mut expected_keys =
                std::collections::HashSet::from([("account".to_owned(), "summary".to_owned())]);
            let (revision, pending_count) = store.summary()?;
            let summary = store
                .get("account", "summary")?
                .ok_or(IndexError::RebuildRequired("v3 account summary absent"))?;
            if revision == 0 || pending_count != head.pending.len() as u64 {
                return Err(IndexError::RebuildRequired("v3 account summary differs"));
            }
            let mut failure_count = 0_u64;
            for (id, grant) in &account.grants {
                expected_keys.insert(("grant".to_owned(), id.clone()));
                if store.get("grant", id)?
                    != Some(serde_json::to_value(grant).map_err(|e| corrupt(e.to_string()))?)
                {
                    return Err(corrupt("v3 retained provider grant differs"));
                }
                let uncertain = grant
                    .consumed_k
                    .as_ref()
                    .filter(|_| grant.certified_q.is_none())
                    .map(|k| uncertain_q(&self.root, k, false))
                    .transpose()?
                    .flatten();
                let expected = uncertain
                    .as_ref()
                    .map(|q| serde_json::to_value(q).map_err(|error| corrupt(error.to_string())))
                    .transpose()?;
                if expected.is_some() {
                    expected_keys.insert(("uncertain-q".to_owned(), format!("grant:{id}")));
                }
                if store.get("uncertain-q", &format!("grant:{id}"))? != expected {
                    return Err(corrupt("v3 retained provider Q debt differs"));
                }
                if let Some(q) = &grant.certified_q {
                    let terminal = q
                        .terminal
                        .as_ref()
                        .ok_or_else(|| corrupt("v3 provider terminal absent"))?;
                    let value: serde_json::Value = terminal.read_json(&self.root)?;
                    let outcome = value["outcome"]
                        .as_str()
                        .ok_or_else(|| corrupt("v3 provider terminal outcome absent"))?;
                    if outcome != "clean" {
                        failure_count = failure_count
                            .checked_add(1)
                            .ok_or(IndexError::Conflict("v3 failure count overflow"))?;
                        expected_keys.insert(("failure".to_owned(), id.clone()));
                        let expected = serde_json::json!({
                            "q": q,
                            "outcome": outcome,
                            "completed_unix_nanos": q.completed_unix_nanos,
                        });
                        if store.get("failure", id)? != Some(expected) {
                            return Err(corrupt("v3 retained provider failure differs"));
                        }
                    }
                }
            }
            for (id, effect) in &account.effects {
                let class = if matches!(effect.kind, EffectKind::ManualQuota) {
                    "manual"
                } else {
                    "effect"
                };
                expected_keys.insert((class.to_owned(), id.clone()));
                if store.get(class, id)?
                    != Some(serde_json::to_value(effect).map_err(|e| corrupt(e.to_string()))?)
                {
                    return Err(corrupt("v3 retained account effect differs"));
                }
                let uncertain = effect
                    .consumed_k
                    .as_ref()
                    .filter(|_| effect.certified_q.is_none())
                    .map(|k| {
                        uncertain_q(
                            &self.root,
                            k,
                            matches!(effect.kind, EffectKind::ManualQuota),
                        )
                    })
                    .transpose()?
                    .flatten();
                let expected = uncertain
                    .as_ref()
                    .map(|q| serde_json::to_value(q).map_err(|error| corrupt(error.to_string())))
                    .transpose()?;
                if expected.is_some() {
                    expected_keys.insert(("uncertain-q".to_owned(), format!("effect:{id}")));
                }
                if store.get("uncertain-q", &format!("effect:{id}"))? != expected {
                    return Err(corrupt("v3 retained account Q debt differs"));
                }
            }
            for (id, pending) in &head.pending {
                expected_keys.insert(("pending".to_owned(), id.clone()));
                if store.get("pending", id)?
                    != Some(serde_json::to_value(pending).map_err(|e| corrupt(e.to_string()))?)
                {
                    return Err(corrupt("v3 retained pending source differs"));
                }
            }
            for (id, typed) in &head.sources {
                expected_keys.insert(("source".to_owned(), id.clone()));
                if store.get("source", id)?
                    != Some(serde_json::to_value(typed).map_err(|e| corrupt(e.to_string()))?)
                {
                    return Err(corrupt("v3 retained typed source differs"));
                }
            }
            for (class, marker) in [
                ("quota-marker", &head.quota_rejection),
                ("auth-marker", &head.auth_rejection),
            ] {
                let expected = marker
                    .as_ref()
                    .map(|marker| {
                        serde_json::to_value(marker).map_err(|error| corrupt(error.to_string()))
                    })
                    .transpose()?;
                if expected.is_some() {
                    expected_keys.insert((class.to_owned(), "account".to_owned()));
                }
                if store.get(class, "account")? != expected {
                    return Err(corrupt("v3 retained account marker differs"));
                }
            }
            for (model, marker) in &head.model_capacity {
                expected_keys.insert(("model-capacity".to_owned(), model.clone()));
                if store.get("model-capacity", model)?
                    != Some(serde_json::to_value(marker).map_err(|e| corrupt(e.to_string()))?)
                {
                    return Err(corrupt("v3 retained model marker differs"));
                }
            }
            if store.admission_keys()? != expected_keys {
                return Err(corrupt("v3 retained keyed inventory differs"));
            }
            let expected_summary = serde_json::json!({
                "physical_key": key,
                "observed_invocations": account.observed_invocations,
                "marker_times": account.markers,
                "failure_count": failure_count,
                "unknown_marker_scope": head.unknown_marker_scope,
                "grant_count": account.grants.len(),
                "effect_count": account.effects.len(),
                "pending_count": head.pending.len(),
                "source_count": head.sources.len(),
            });
            if summary != expected_summary {
                return Err(corrupt("v3 retained account summary differs"));
            }
        }
        let source_after = source.metadata()?;
        if (source_before.dev(), source_before.ino()) != (source_after.dev(), source_after.ino()) {
            return Err(corrupt("v3 admission config directory changed"));
        }
        self.check_current()?;
        Ok(())
    }
}

fn validate_storage(storage: &Path, generation: &str) -> Result<()> {
    for name in ["cursors", "decisions", "keyed-accounts", "account-catalog"] {
        if !storage.join(name).is_dir() {
            return Err(IndexError::RebuildRequired("v3 generation storage absent"));
        }
    }
    let mut catalog = std::collections::HashSet::new();
    for entry in fs::read_dir(storage.join("account-catalog"))? {
        let path = entry?.path();
        let marker: KnownKey<String> =
            read(&path)?.ok_or_else(|| corrupt("v3 account catalog entry absent"))?;
        let digest = keyed(&marker.key)?;
        if marker.generation != generation
            || path.file_name().and_then(|name| name.to_str()) != Some(&format!("{digest}.json"))
            || !catalog.insert(digest.clone())
        {
            return Err(corrupt("v3 account catalog identity differs"));
        }
        let store = KeyedAccountStore::open(
            &storage.join("keyed-accounts").join(&digest),
            generation,
            &marker.key,
        )?;
        store.summary()?;
    }
    let actual = fs::read_dir(storage.join("keyed-accounts"))?
        .map(|entry| entry.map(|item| item.file_name().to_string_lossy().into_owned()))
        .collect::<io::Result<std::collections::HashSet<_>>>()?;
    if actual != catalog {
        return Err(IndexError::RebuildRequired(
            "v3 account catalog/storage differs",
        ));
    }
    Ok(())
}

fn change<T: Serialize>(class: &str, key: &str, value: &T) -> Result<Change> {
    Ok(Change {
        class: class.into(),
        key: key.into(),
        value: Some(serde_json::to_value(value).map_err(|e| corrupt(e.to_string()))?),
    })
}

fn insert_checked(store: &KeyedAccountStore, revision: &mut u64, item: Change) -> Result<()> {
    let expected = item.value.clone();
    let class = item.class.clone();
    let key = item.key.clone();
    *revision = store.commit(*revision, vec![item])?;
    if store.get(&class, &key)? != expected {
        return Err(corrupt("v3 keyed stage readback differs"));
    }
    Ok(())
}

fn uncertain_q(root: &Path, k: &Artifact, manual: bool) -> Result<Option<Artifact>> {
    let relative = Path::new(&k.path);
    let parent = relative
        .parent()
        .ok_or_else(|| corrupt("v3 K path has no parent"))?;
    let file = relative
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| corrupt("v3 K path invalid"))?;
    let q_name = if manual {
        if file != "k.json" {
            return Err(corrupt("v3 manual K path invalid"));
        }
        "q.json".to_owned()
    } else {
        let id = file
            .strip_suffix(".consumed.json")
            .ok_or_else(|| corrupt("v3 K path invalid"))?;
        format!("{id}.drain.json")
    };
    let q = parent.join(q_name);
    if root.join(&q).exists() {
        Ok(Some(Artifact::from_existing(root, &q)?))
    } else {
        Ok(None)
    }
}

fn stage_account(root: &Path, staged: &Index, key: &str, mut account: Account) -> Result<()> {
    if key.is_empty() || account.physical_key != key {
        return Err(corrupt("v3 physical account identity differs"));
    }
    account.generation = staged.generation.clone();
    account.revision = account.revision.max(1);
    let head = AccountHead::from_account_unbounded(root, &account)?;
    head.validate_unbounded(root, &staged.generation, key)?;
    let store = KeyedAccountStore::create(
        &staged.base().join("keyed-accounts").join(keyed(&key)?),
        &staged.generation,
        key,
    )?;
    let mut revision = 0;
    let mut failure_count = 0_u64;
    for (id, grant) in &account.grants {
        grant.grant.require_present(root)?;
        if let Some(k) = &grant.consumed_k {
            k.require_present(root)?;
        }
        if let Some(q) = &grant.certified_q {
            q.verify(root)?;
            let terminal = q
                .terminal
                .as_ref()
                .ok_or_else(|| corrupt("v3 settled provider lacks terminal"))?;
            let value: serde_json::Value = terminal.read_json(root)?;
            let outcome = value["outcome"]
                .as_str()
                .ok_or_else(|| corrupt("v3 typed terminal outcome absent"))?;
            if outcome != "clean" {
                insert_checked(
                    &store,
                    &mut revision,
                    change(
                        "failure",
                        id,
                        &serde_json::json!({
                            "q": q,
                            "outcome": outcome,
                            "completed_unix_nanos": q.completed_unix_nanos,
                        }),
                    )?,
                )?;
                failure_count = failure_count
                    .checked_add(1)
                    .ok_or(IndexError::Conflict("v3 failure count overflow"))?;
            }
        } else if let Some(k) = &grant.consumed_k {
            if let Some(q) = uncertain_q(root, k, false)? {
                insert_checked(
                    &store,
                    &mut revision,
                    change("uncertain-q", &format!("grant:{id}"), &q)?,
                )?;
            }
        }
        insert_checked(&store, &mut revision, change("grant", id, grant)?)?;
    }
    for (id, effect) in &account.effects {
        effect.intent.require_present(root)?;
        if let Some(k) = &effect.consumed_k {
            k.require_present(root)?;
        }
        if let Some(q) = &effect.certified_q {
            q.verify(root)?;
        } else if let Some(k) = &effect.consumed_k {
            if let Some(q) = uncertain_q(root, k, matches!(effect.kind, EffectKind::ManualQuota))? {
                insert_checked(
                    &store,
                    &mut revision,
                    change("uncertain-q", &format!("effect:{id}"), &q)?,
                )?;
            }
        }
        if let Some(result) = &effect.result {
            result.require_present(root)?;
        }
        let class = if matches!(effect.kind, EffectKind::ManualQuota) {
            "manual"
        } else {
            "effect"
        };
        insert_checked(&store, &mut revision, change(class, id, effect)?)?;
    }
    for (id, pending) in &head.pending {
        insert_checked(&store, &mut revision, change("pending", id, pending)?)?;
    }
    for (source, typed) in &head.sources {
        insert_checked(&store, &mut revision, change("source", source, typed)?)?;
    }
    for (class, marker) in [
        ("quota-marker", &head.quota_rejection),
        ("auth-marker", &head.auth_rejection),
    ] {
        if let Some(marker) = marker {
            insert_checked(&store, &mut revision, change(class, "account", marker)?)?;
        }
    }
    for (model, marker) in &head.model_capacity {
        insert_checked(
            &store,
            &mut revision,
            change("model-capacity", model, marker)?,
        )?;
    }
    let metadata = serde_json::json!({
        "physical_key": key,
        "observed_invocations": account.observed_invocations,
        "marker_times": account.markers,
        "failure_count": failure_count,
        "unknown_marker_scope": head.unknown_marker_scope,
        "grant_count": account.grants.len(),
        "effect_count": account.effects.len(),
        "pending_count": head.pending.len(),
        "source_count": head.sources.len(),
    });
    insert_checked(
        &store,
        &mut revision,
        change("account", "summary", &metadata)?,
    )?;
    let (actual_revision, pending_count) = store.summary()?;
    if actual_revision != revision || pending_count != head.pending.len() as u64 {
        return Err(corrupt("v3 keyed account stage count differs"));
    }
    Ok(())
}

fn stage_routes(staged: &Index, mut decisions: Vec<OfflineDecision>) -> Result<()> {
    let mut cursors: BTreeMap<String, Cursor> = BTreeMap::new();
    let mut seen = std::collections::HashSet::new();
    decisions.sort_by_key(|decision| decision.sequence);
    for seed in decisions {
        if seed.handoff.is_empty()
            || seed.key.model.is_empty()
            || seed.key.config_sha256.is_empty()
            || seed.candidate_identity.is_empty()
            || !seen.insert(seed.handoff.clone())
        {
            return Err(corrupt("v3 route decision identity invalid"));
        }
        let digest = keyed(&seed.key)?;
        let cursor = cursors.entry(digest).or_insert_with(|| Cursor {
            generation: staged.generation.clone(),
            key: seed.key.clone(),
            sequence: 0,
            index: None,
            last_handoff: None,
        });
        if cursor.key != seed.key {
            return Err(corrupt("v3 route cursor collision"));
        }
        let decision = Decision {
            generation: staged.generation.clone(),
            handoff: seed.handoff,
            key: seed.key,
            candidate_identity: seed.candidate_identity,
            candidate_index: seed.candidate_index,
            pin: seed.pin,
            sequence: seed.sequence,
            receipt: Some(seed.receipt),
        };
        let next = staged.advanced(cursor, &decision)?;
        write_new(&staged.decision_path(&decision.handoff)?, &decision)?;
        staged.mark_known("decisions", &decision.handoff)?;
        if staged.read_decision(&decision.handoff)? != Some(decision.clone()) {
            return Err(corrupt("v3 route decision readback differs"));
        }
        *cursor = next;
    }
    for cursor in cursors.into_values() {
        if cursor.sequence != 0 {
            staged.mark_known("cursors", &cursor.key)?;
            write_new(&staged.key_path("cursors", &cursor.key)?, &cursor)?;
            if staged.cursor_unlocked(&cursor.key)? != cursor {
                return Err(corrupt("v3 route cursor readback differs"));
            }
        }
    }
    Ok(())
}

fn check_v2_evidence(root: &Path, snapshot: &OfflineSnapshot) -> Result<()> {
    let Some(manifest): Option<Manifest> = read(&root.join("index-v1/manifest.json"))? else {
        return Ok(());
    };
    if manifest.version != VERSION {
        return if manifest.version == V3 {
            Ok(())
        } else {
            Err(IndexError::RebuildRequired(
                "unsupported source manifest version",
            ))
        };
    }
    let old = Index::open(root)?;
    old.validate_live_routes()?;
    for key in old.live_account_keys()? {
        let indexed = old.account(&key)?;
        let scanned = snapshot
            .accounts
            .get(&key)
            .ok_or_else(|| corrupt("v2 indexed account absent from retained evidence"))?;
        if indexed.observed_invocations > scanned.observed_invocations {
            return Err(corrupt("v2 observed K count exceeds retained evidence"));
        }
        for (id, grant) in indexed.grants {
            let retained = scanned
                .grants
                .get(&id)
                .ok_or_else(|| corrupt("v2 indexed grant absent from retained evidence"))?;
            if retained.grant != grant.grant
                || retained.decision_handoff != grant.decision_handoff
                || (grant.consumed_k.is_some() && retained.consumed_k != grant.consumed_k)
            {
                return Err(corrupt("v2 grant differs from retained evidence"));
            }
        }
        for (id, effect) in indexed.effects {
            let retained = scanned
                .effects
                .get(&id)
                .ok_or_else(|| corrupt("v2 indexed effect absent from retained evidence"))?;
            if retained.intent != effect.intent
                || retained.source != effect.source
                || retained.kind != effect.kind
                || (effect.consumed_k.is_some() && retained.consumed_k != effect.consumed_k)
            {
                return Err(corrupt("v2 effect differs from retained evidence"));
            }
        }
    }
    Ok(())
}

pub(super) fn rebuild(root: &Path, socket: &Path, source: &Path) -> Result<KeyedGeneration> {
    rebuild_inner(root, socket, source, || {}, || Ok(()))
}

#[cfg(test)]
pub(super) fn rebuild_test_hook(
    root: &Path,
    socket: &Path,
    source: &Path,
    before_manifest: impl FnOnce() -> Result<()>,
) -> Result<KeyedGeneration> {
    rebuild_inner(root, socket, source, || {}, before_manifest)
}

fn rebuild_inner(
    root: &Path,
    socket: &Path,
    source: &Path,
    after_scan: impl FnOnce(),
    before_manifest: impl FnOnce() -> Result<()>,
) -> Result<KeyedGeneration> {
    let _freeze = frozen_admission(root, socket)?;
    let source_before = source.metadata()?;
    if !source_before.is_dir() {
        return Err(corrupt("v3 config source not a directory"));
    }
    let mut snapshot = super::super::fresh_provider::offline_snapshot_v3(root, source)
        .map_err(|e| corrupt(format!("v3 retained evidence: {e}")))?;
    super::super::fresh_provider::complete_v3_effects(root, &mut snapshot)
        .map_err(|e| corrupt(format!("v3 effect evidence: {e}")))?;
    check_v2_evidence(root, &snapshot)?;
    after_scan();
    let source_after = source.metadata()?;
    if (source_before.dev(), source_before.ino()) != (source_after.dev(), source_after.ino()) {
        return Err(corrupt("v3 config source directory changed"));
    }
    for (model, digest) in &snapshot.source_models {
        let pool =
            oulipoly_runtime::executor::cli::fresh_remote::load_fresh_headless_pool(source, model)
                .map_err(|e| corrupt(format!("v3 config source readback: {e}")))?;
        if &pool.config_sha256 != digest {
            return Err(corrupt("v3 config source changed before publication"));
        }
    }
    let base = root.join("index-v1");
    let generations = base.join("generations");
    fs::create_dir_all(&generations)?;
    let generation = uuid::Uuid::new_v4().to_string();
    let storage = generations.join(&generation);
    fs::create_dir(&storage)?;
    for name in ["cursors", "decisions", "keyed-accounts", "account-catalog"] {
        fs::create_dir(storage.join(name))?;
    }
    sync_dir(&generations)?;
    let staged = Index {
        root: root.to_owned(),
        generation: generation.clone(),
        storage,
        route_reader_probe: false,
    };
    stage_routes(&staged, snapshot.decisions)?;
    for (key, account) in snapshot.accounts {
        stage_account(root, &staged, &key, account)?;
        write_new(
            &staged
                .base()
                .join("account-catalog")
                .join(format!("{}.json", keyed(&key)?)),
            &KnownKey {
                generation: generation.clone(),
                key,
            },
        )?;
    }
    // Source identity is separately retained even when a model has no account
    // evidence; no route or effect can silently inherit a different config.
    write_new(
        &staged.base().join("source-models.json"),
        &snapshot.source_models,
    )?;
    let models: BTreeMap<String, String> = read(&staged.base().join("source-models.json"))?
        .ok_or_else(|| corrupt("v3 source models absent"))?;
    if models != snapshot.source_models {
        return Err(corrupt("v3 source model readback differs"));
    }
    for name in ["cursors", "decisions", "keyed-accounts", "account-catalog"] {
        sync_dir(&staged.base().join(name))?;
    }
    sync_dir(&staged.base())?;
    validate_storage(&staged.base(), &generation)?;
    for (model, digest) in &models {
        let pool =
            oulipoly_runtime::executor::cli::fresh_remote::load_fresh_headless_pool(source, model)
                .map_err(|e| corrupt(format!("v3 final config readback: {e}")))?;
        if &pool.config_sha256 != digest {
            return Err(corrupt("v3 config source changed during staging"));
        }
    }
    before_manifest()?;
    write_atomic(
        &base.join("manifest.json"),
        &Manifest {
            version: V3,
            generation,
            generation_dir: true,
        },
    )?;
    KeyedGeneration::open(root)
}

#[cfg(test)]
mod tests {
    use super::*;
    use oulipoly_kernel_broker::protocol::{FreshAccountEffectReadback, FreshQuotaWindow};

    fn artifact(root: &Path, name: &str, bytes: &[u8]) -> Artifact {
        fs::write(root.join(name), bytes).unwrap();
        Artifact::from_existing(root, Path::new(name)).unwrap()
    }

    #[test]
    fn v3_clean_empty_genesis_is_empty_and_v2_refuses_it() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("broker");
        let source = temp.path().join("source");
        fs::create_dir(&root).unwrap();
        fs::create_dir(&source).unwrap();
        drop(broker_admission_lease(&root).unwrap());
        let generation = rebuild(&root, &temp.path().join("absent.sock"), &source).unwrap();
        assert!(
            generation
                .storage
                .join("keyed-accounts")
                .read_dir()
                .unwrap()
                .next()
                .is_none()
        );
        assert!(Index::open(&root).is_err());
    }

    #[test]
    fn v3_separates_account_quota_auth_from_model_capacity() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let storage = root.join("staged");
        fs::create_dir(&storage).unwrap();
        fs::create_dir(storage.join("keyed-accounts")).unwrap();
        let generation = uuid::Uuid::new_v4().to_string();
        let staged = Index {
            root: root.into(),
            generation: generation.clone(),
            storage,
            route_reader_probe: false,
        };
        let mut account = Account {
            generation: String::new(),
            physical_key: "physical".into(),
            revision: 0,
            grants: BTreeMap::new(),
            effects: BTreeMap::new(),
            observed_invocations: 3,
            markers: MarkerTimes {
                quota_rejection_nanos: Some(20),
                auth_rejection_nanos: Some(30),
                model_capacity_nanos: Some(10),
            },
            source_q: BTreeMap::new(),
            recent_failure_nanos: Vec::new(),
        };
        for (id, model, outcome, nanos) in [
            ("capacity", "model-a", "model_at_capacity", 10),
            ("quota", "model-b", "quota_rejected", 20),
            ("auth", "model-c", "auth_rejected", 30),
        ] {
            let grant = artifact(root, &format!("{id}.grant"), b"grant");
            let k = artifact(root, &format!("{id}.k"), b"K");
            let q = artifact(root, &format!("{id}.q"), b"Q");
            let terminal = artifact(root, &format!("{id}.terminal"), &serde_json::to_vec(&serde_json::json!({
                "grant_id": id,
                "selection": {"account_identity":"physical", "model":model, "config_sha256":"config"},
                "physical_q_sha256": q.sha256,
                "physical_q_unix_nanos": nanos,
                "outcome": outcome,
            })).unwrap());
            account.grants.insert(
                id.into(),
                ProviderGrant {
                    decision_handoff: id.into(),
                    grant,
                    candidate: None,
                    consumed_k: Some(k.clone()),
                    certified_q: Some(PhysicalQ {
                        physical_k: k,
                        q,
                        terminal: Some(terminal),
                        completed_unix_nanos: nanos,
                    }),
                },
            );
        }
        stage_account(root, &staged, "physical", account).unwrap();
        let store = KeyedAccountStore::open(
            &staged
                .base()
                .join("keyed-accounts")
                .join(keyed(&"physical").unwrap()),
            &generation,
            "physical",
        )
        .unwrap();
        assert_eq!(store.summary().unwrap().1, 0);
        assert_eq!(
            store.get("quota-marker", "account").unwrap().unwrap()["model"],
            "model-b"
        );
        assert_eq!(
            store.get("auth-marker", "account").unwrap().unwrap()["model"],
            "model-c"
        );
        let capacity = store
            .get("model-capacity", &keyed(&("model-a", "config")).unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(capacity["model"], "model-a");
        assert!(store.get("failure", "capacity").unwrap().is_some());
        assert_eq!(
            store.get("account", "summary").unwrap().unwrap()["failure_count"],
            3
        );
    }

    #[test]
    fn v3_stages_hundreds_of_settled_sources_and_all_pending_classes() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let storage = root.join("staged");
        fs::create_dir(&storage).unwrap();
        fs::create_dir(storage.join("keyed-accounts")).unwrap();
        let generation = uuid::Uuid::new_v4().to_string();
        let staged = Index {
            root: root.into(),
            generation: generation.clone(),
            storage,
            route_reader_probe: false,
        };
        let mut account = Account {
            generation: String::new(),
            physical_key: "shared-physical".into(),
            revision: 0,
            grants: BTreeMap::new(),
            effects: BTreeMap::new(),
            observed_invocations: 205,
            markers: MarkerTimes::default(),
            source_q: BTreeMap::new(),
            recent_failure_nanos: Vec::new(),
        };
        for n in 0..205 {
            let id = format!("settled-{n}");
            let source_name = match n {
                201 | 202 => "conflict".to_owned(),
                203 | 204 => "ordered".to_owned(),
                _ => format!("commands-{n}"),
            };
            let intent = artifact(root, &format!("{id}-intent.json"), b"{}");
            let k = artifact(root, &format!("{id}-k.json"), b"{}");
            let q_artifact = artifact(root, &format!("{id}-q.json"), b"{}");
            let result = FreshAccountEffectReadback {
                effect_id: id.clone(),
                state: "drained".into(),
                outcome: Some(if n == 204 { "invalid" } else { "valid_windows" }.into()),
                windows: if n == 204 {
                    Vec::new()
                } else {
                    vec![FreshQuotaWindow {
                        used_percent: 20.0,
                        resets_at: "2099-01-01T00:00:00Z".into(),
                        remaining: Some(80),
                    }]
                },
                completed_unix_seconds: Some(1_800_000_000),
                artifact: id.clone(),
                peer_effect_id: None,
                peer_artifact: None,
            };
            let result = artifact(
                root,
                &format!("{id}-result.json"),
                &serde_json::to_vec(&result).unwrap(),
            );
            account.effects.insert(
                id,
                EffectIntent {
                    kind: EffectKind::Quota,
                    source: SourceKey {
                        commands_sha256: source_name,
                        environment_sha256: "environment".into(),
                    },
                    decision_handoff: String::new(),
                    route_source: None,
                    candidate: None,
                    intent,
                    reuse: None,
                    consumed_k: Some(k.clone()),
                    certified_q: Some(PhysicalQ {
                        physical_k: k,
                        q: q_artifact,
                        terminal: None,
                        completed_unix_nanos: if n == 201 || n == 202 { 202 } else { n + 1 },
                    }),
                    result: Some(result),
                },
            );
        }
        account.grants.insert(
            "pending-provider".into(),
            ProviderGrant {
                decision_handoff: "handoff".into(),
                grant: artifact(root, "pending-provider.json", b"{}"),
                candidate: None,
                consumed_k: None,
                certified_q: None,
            },
        );
        for (id, kind) in [
            ("pending-effect", EffectKind::Quota),
            ("pending-manual", EffectKind::ManualQuota),
        ] {
            account.effects.insert(
                id.into(),
                EffectIntent {
                    kind,
                    source: SourceKey {
                        commands_sha256: "pending-commands".into(),
                        environment_sha256: "pending-environment".into(),
                    },
                    decision_handoff: String::new(),
                    route_source: None,
                    candidate: None,
                    intent: artifact(root, &format!("{id}.json"), b"{}"),
                    reuse: None,
                    consumed_k: None,
                    certified_q: None,
                    result: None,
                },
            );
        }
        let small_storage = root.join("staged-small");
        fs::create_dir(&small_storage).unwrap();
        fs::create_dir(small_storage.join("keyed-accounts")).unwrap();
        let small_staged = Index {
            root: root.into(),
            generation: generation.clone(),
            storage: small_storage,
            route_reader_probe: false,
        };
        let mut small = account.clone();
        small.effects.clear();
        small.observed_invocations = 0;
        stage_account(root, &small_staged, "shared-physical", small).unwrap();
        let small_store = KeyedAccountStore::open(
            &small_staged
                .base()
                .join("keyed-accounts")
                .join(keyed(&"shared-physical").unwrap()),
            &generation,
            "shared-physical",
        )
        .unwrap();
        let (_, read_one) = keyed_store::measured(|| {
            assert!(
                small_store
                    .get("grant", "pending-provider")
                    .unwrap()
                    .is_some()
            );
            assert!(
                small_store
                    .get("pending", "grant:pending-provider")
                    .unwrap()
                    .is_some()
            );
        });
        stage_account(root, &staged, "shared-physical", account).unwrap();
        let store = KeyedAccountStore::open(
            &staged
                .base()
                .join("keyed-accounts")
                .join(keyed(&"shared-physical").unwrap()),
            &generation,
            "shared-physical",
        )
        .unwrap();
        let (_, read_many) = keyed_store::measured(|| {
            assert!(store.get("grant", "pending-provider").unwrap().is_some());
            assert!(
                store
                    .get("pending", "grant:pending-provider")
                    .unwrap()
                    .is_some()
            );
        });
        eprintln!("v3 provider grant/pending keyed one/205 settled: {read_one:?} / {read_many:?}");
        assert_eq!(read_one.open_attempts, read_many.open_attempts);
        assert_eq!(read_one.directory_entries, 0);
        assert_eq!(read_many.directory_entries, 0);
        assert!(read_many.bytes_parsed <= read_one.bytes_parsed + 32);
        assert_eq!(read_one.bytes_written + read_many.bytes_written, 0);
        assert_eq!(store.summary().unwrap().1, 3);
        assert!(store.get("manual", "pending-manual").unwrap().is_some());
        assert!(store.get("effect", "settled-204").unwrap().is_some());
        let source = |name: &str| SourceKey {
            commands_sha256: name.into(),
            environment_sha256: "environment".into(),
        };
        let conflict = store
            .get("source", &keyed(&source("conflict")).unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(conflict["quota"]["outcome"], "unknown");
        let ordered = store
            .get("source", &keyed(&source("ordered")).unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(ordered["quota"]["outcome"], "invalid");
    }
}
