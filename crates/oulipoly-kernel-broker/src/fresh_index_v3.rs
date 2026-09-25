//! Frozen v3 importer and a separate private, readback-only admission. The v2
//! Index deliberately refuses this manifest. The keyed route facts below do
//! not authorize a route or K writer.
use super::*;
use chrono::Utc;
use keyed_store::{Change, KeyedAccountStore};
use oulipoly_kernel_broker::protocol::FreshAccountEffectKind;

const V3: u32 = 3;

/// One account-revision view for a future route writer. In particular, an
/// absent or invalid quota observation is never interpreted as available.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum RouteEligibility {
    Eligible {
        account_revision: u64,
        quota_basis_points: Option<u32>,
        observed_invocations: u64,
    },
    ProbeRequired,
    Excluded,
    Unknown,
}

fn typed<T: serde::de::DeserializeOwned>(value: Option<serde_json::Value>) -> Result<Option<T>> {
    value
        .map(|value| serde_json::from_value(value).map_err(|e| corrupt(e.to_string())))
        .transpose()
}

#[derive(Debug)]
pub(crate) struct KeyedGeneration {
    pub generation: String,
    root: PathBuf,
    storage: PathBuf,
    source: Option<PathBuf>,
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
            source: None,
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
        let mut generation = Self::open(root)?;
        generation.verify_retained(source)?;
        generation.source = Some(source.canonicalize()?);
        Ok(generation)
    }

    pub(crate) fn admitted_source(&self) -> Result<&Path> {
        self.source
            .as_deref()
            .ok_or(IndexError::Conflict("v3 live source was not admitted"))
    }

    fn ensure_quota_account(&self, account: &str, model: &str, config: &str) -> Result<()> {
        self.check_current()?;
        let ledger_path = self.storage.join("source-models.json");
        let mut ledger: BTreeMap<String, String> = read(&ledger_path)?
            .ok_or(IndexError::RebuildRequired("v3 source model ledger absent"))?;
        if ledger.get(model).is_some_and(|prior| prior != config) {
            return Err(IndexError::Conflict("v3 quota model config changed"));
        }
        if !ledger.contains_key(model) {
            ledger.insert(model.to_owned(), config.to_owned());
            write_atomic(&ledger_path, &ledger)?;
        }
        let digest = keyed(&account)?;
        let path = self.storage.join("keyed-accounts").join(&digest);
        let catalog = self
            .storage
            .join("account-catalog")
            .join(format!("{digest}.json"));
        if path.is_dir() {
            let marker: KnownKey<String> = read(&catalog)?.ok_or(IndexError::RebuildRequired(
                "v3 quota account catalog absent",
            ))?;
            if marker.generation != self.generation || marker.key != account {
                return Err(IndexError::RebuildRequired(
                    "v3 quota account catalog differs",
                ));
            }
            self.account(account)?;
            return Ok(());
        }
        if catalog.exists() {
            return Err(IndexError::RebuildRequired(
                "v3 quota catalog has no account",
            ));
        }
        let store = KeyedAccountStore::create(&path, &self.generation, account)?;
        let summary = serde_json::json!({
            "physical_key": account,
            "observed_invocations": 0,
            "marker_times": MarkerTimes::default(),
            "failure_count": 0,
            "unknown_marker_scope": false,
            "grant_count": 0,
            "effect_count": 0,
            "pending_count": 0,
            "source_count": 0,
        });
        store.commit(0, vec![change("account", "summary", &summary)?])?;
        write_new(
            &catalog,
            &KnownKey {
                generation: self.generation.clone(),
                key: account.to_owned(),
            },
        )?;
        self.account(account)?;
        Ok(())
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

    /// Find a settled auth source by the physical account and command/env
    /// key. This is one exact source read, independent of retained effects.
    pub(crate) fn auth_peer_intent(
        &self,
        account: &str,
        source: &SourceKey,
    ) -> Result<Option<Artifact>> {
        self.check_current()?;
        let store = self.account(account)?;
        let digest = keyed(source)?;
        let (revision, pending, mut values) =
            store.read_many(&[("source", &digest), ("auth-flight", "account")])?;
        let active: Option<Artifact> = typed(values.pop().unwrap())?;
        if let Some(active) = active {
            let value: serde_json::Value = active.read_json(&self.root)?;
            let id = value["id"]
                .as_str()
                .ok_or_else(|| corrupt("v3 auth flight id absent"))?;
            let effect: EffectIntent = typed(store.get("effect", id)?)?
                .ok_or(IndexError::RebuildRequired("v3 auth flight effect absent"))?;
            let debt: PendingHead = typed(store.get("pending", &format!("effect:{id}"))?)?
                .ok_or(IndexError::Conflict("v3 auth flight pending debt absent"))?;
            if pending == 0
                || debt.announcement != active
                || debt.source.as_ref() != Some(source)
                || effect.kind != EffectKind::Auth
                || effect.reuse.is_some()
                || effect.source != *source
                || effect.intent != active
                || effect.certified_q.is_some()
                || effect.result.is_some()
            {
                return Err(corrupt("v3 auth flight source differs"));
            }
            active.require_present(&self.root)?;
            if store.summary()? != (revision, pending) {
                return Err(IndexError::Conflict("v3 auth flight account changed"));
            }
            self.check_current()?;
            return Ok(Some(active));
        }
        if pending != 0 {
            return Ok(None);
        }
        let head: Option<SourceHead> = typed(values.pop().unwrap())?;
        let Some(head) = head else {
            return Ok(None);
        };
        if head.source != *source {
            return Err(corrupt("v3 auth peer source changed"));
        }
        let (Some(quota), Some(auth)) = (head.quota, head.auth) else {
            return Ok(None);
        };
        if auth.q.completed_unix_nanos <= quota.q.completed_unix_nanos {
            return Ok(None);
        }
        auth.q.verify(&self.root)?;
        let result = auth
            .result
            .ok_or(IndexError::Conflict("v3 auth peer result absent"))?;
        result.require_present(&self.root)?;
        let parent = Path::new(&result.path)
            .parent()
            .ok_or_else(|| corrupt("v3 auth peer result parent absent"))?;
        let intent = Artifact::from_existing(&self.root, &parent.join("intent.json"))?;
        if store.summary()? != (revision, pending) {
            return Err(IndexError::Conflict("v3 auth peer account changed"));
        }
        self.check_current()?;
        Ok(Some(intent))
    }

    pub(crate) fn require_auth_source(&self, account: &str, source: &SourceKey) -> Result<()> {
        self.check_current()?;
        let store = self.account(account)?;
        let digest = keyed(source)?;
        let (revision, pending, mut values) =
            store.read_many(&[("source", &digest), ("auth-marker", "account")])?;
        let marker: Option<MarkerHead> = typed(values.pop().unwrap())?;
        let head: Option<SourceHead> = typed(values.pop().unwrap())?;
        if pending != 0 {
            return Err(IndexError::Conflict("v3 auth account has pending debt"));
        }
        let head = head.ok_or(IndexError::Conflict("v3 auth has no quota Q"))?;
        if head.source != *source {
            return Err(corrupt("v3 auth source identity changed"));
        }
        self.check_auth_prerequisite(&head, marker.as_ref())?;
        if store.summary()? != (revision, pending) {
            return Err(IndexError::Conflict("v3 auth account changed"));
        }
        self.check_current()?;
        Ok(())
    }

    fn check_auth_prerequisite(
        &self,
        head: &SourceHead,
        marker: Option<&MarkerHead>,
    ) -> Result<()> {
        let quota = head
            .quota
            .as_ref()
            .ok_or(IndexError::Conflict("v3 auth has no quota Q"))?;
        quota.q.verify(&self.root)?;
        quota
            .result
            .as_ref()
            .ok_or(IndexError::Conflict("v3 auth quota result absent"))?
            .require_present(&self.root)?;
        let failed_quota = matches!(quota.outcome.as_str(), "failed" | "empty" | "invalid");
        let rejected_healthy = marker.is_some_and(|marker| {
            marker.outcome == "auth_rejected"
                && quota
                    .quota_basis_points_at(Utc::now().timestamp())
                    .is_some()
                && marker.q.completed_unix_nanos > quota.q.completed_unix_nanos
        });
        if !failed_quota && !rejected_healthy {
            return Err(IndexError::Conflict(
                "v3 auth has no failed quota or newer typed auth rejection",
            ));
        }
        if let Some(marker) = marker {
            marker.q.verify(&self.root)?;
        }
        if head.auth.as_ref().is_some_and(|auth| {
            auth.q.completed_unix_nanos >= quota.q.completed_unix_nanos
                && marker.is_none_or(|marker| {
                    auth.q.completed_unix_nanos >= marker.q.completed_unix_nanos
                })
        }) {
            return Err(IndexError::Conflict(
                "v3 auth already spent after rejection",
            ));
        }
        Ok(())
    }

    pub(crate) fn announce_auth_alias(
        &self,
        binding: &super::super::fresh_provider::Binding,
        request: &oulipoly_kernel_broker::protocol::FreshAccountEffectRequest,
        id: &str,
    ) -> Result<()> {
        self.check_current()?;
        let (readback, source, account, intent) =
            super::super::fresh_provider::v3_auth_alias_readback(
                &self.root,
                self.admitted_source()?,
                binding,
                request,
                id,
            )
            .map_err(|e| corrupt(format!("v3 auth peer: {e}")))?;
        let peer_intent = self
            .auth_peer_intent(&account, &source)?
            .ok_or(IndexError::Conflict("v3 auth peer no longer current"))?;
        let peer_dir = Path::new(&peer_intent.path)
            .parent()
            .ok_or_else(|| corrupt("v3 auth peer parent absent"))?;
        let peer_value: serde_json::Value = peer_intent.read_json(&self.root)?;
        if readback.peer_artifact.as_deref()
            != Some(self.root.join(peer_dir).to_string_lossy().as_ref())
            || readback.peer_effect_id.as_deref() != peer_value["id"].as_str()
        {
            return Err(IndexError::Conflict("v3 auth alias peer differs"));
        }
        self.ensure_quota_account(&account, &request.model, &request.config_sha256)?;
        let store = self.account(&account)?;
        let (revision, pending, mut values) =
            store.read_many(&[("account", "summary"), ("effect", id)])?;
        if values.pop().unwrap().is_some() {
            return Err(IndexError::Conflict(
                "v3 auth alias already announced or account pending",
            ));
        }
        let mut summary = values
            .pop()
            .unwrap()
            .ok_or(IndexError::RebuildRequired("v3 auth alias summary absent"))?;
        if summary["physical_key"].as_str() != Some(account.as_str())
            || summary["pending_count"].as_u64() != Some(pending)
        {
            return Err(corrupt("v3 auth alias account differs"));
        }
        let count = summary["effect_count"]
            .as_u64()
            .ok_or_else(|| corrupt("v3 auth alias count absent"))?;
        summary["effect_count"] = count
            .checked_add(1)
            .ok_or(IndexError::Conflict("v3 auth alias count overflow"))?
            .into();
        let effect = EffectIntent {
            kind: EffectKind::Auth,
            source,
            decision_handoff: binding.handoff_id.clone(),
            route_source: Some(Artifact::from_existing(
                &self.root,
                Path::new(&format!("{}.route-source.json", binding.handoff_id)),
            )?),
            candidate: Some(Artifact::from_existing(
                &self.root,
                Path::new(&format!(
                    "{}.route-{}.json",
                    binding.handoff_id, request.index
                )),
            )?),
            intent: intent.clone(),
            reuse: Some(intent),
            consumed_k: None,
            certified_q: None,
            result: None,
        };
        store.commit(
            revision,
            vec![
                change("effect", id, &effect)?,
                change("account", "summary", &summary)?,
            ],
        )?;
        self.check_current()?;
        Ok(())
    }

    pub(crate) fn observe_auth_alias(
        &self,
        binding: &super::super::fresh_provider::Binding,
        request: &oulipoly_kernel_broker::protocol::FreshAccountEffectRequest,
        id: &str,
    ) -> Result<oulipoly_kernel_broker::protocol::FreshAccountEffectReadback> {
        self.check_current()?;
        let (readback, source, account, intent) =
            super::super::fresh_provider::v3_auth_alias_readback(
                &self.root,
                self.admitted_source()?,
                binding,
                request,
                id,
            )
            .map_err(|e| corrupt(format!("v3 auth alias observation: {e}")))?;
        let store = self.account(&account)?;
        let digest = keyed(&source)?;
        let (revision, pending, mut values) =
            store.read_many(&[("effect", id), ("source", &digest)])?;
        let source_head: Option<SourceHead> = typed(values.pop().unwrap())?;
        let effect: EffectIntent = typed(values.pop().unwrap())?.ok_or(
            IndexError::RebuildRequired("v3 auth alias announcement absent"),
        )?;
        if effect.kind != EffectKind::Auth
            || effect.source != source
            || effect.intent != intent
            || effect.reuse.as_ref() != Some(&intent)
            || effect.consumed_k.is_some()
            || effect.certified_q.is_some()
            || effect.result.is_some()
        {
            return Err(corrupt("v3 auth alias keyed source differs"));
        }
        if store.summary()? != (revision, pending) {
            return Err(IndexError::Conflict("v3 auth alias account changed"));
        }
        self.check_current()?;
        if readback.state != "drained" {
            return Ok(readback);
        }
        let committed = source_head.as_ref().is_some_and(|head| {
            head.source == source
                && head.auth.as_ref().is_some_and(|auth| {
                    auth.outcome == readback.outcome.as_deref().unwrap_or("")
                        && auth.completed_unix_seconds == readback.completed_unix_seconds
                        && auth.result.as_ref().is_some_and(|result| {
                            Path::new(&result.path).parent().is_some_and(|parent| {
                                readback.peer_artifact.as_deref()
                                    == Some(self.root.join(parent).to_string_lossy().as_ref())
                            })
                        })
                })
        });
        if !committed {
            return Ok(
                oulipoly_kernel_broker::protocol::FreshAccountEffectReadback {
                    state: "unknown".into(),
                    outcome: None,
                    windows: Vec::new(),
                    completed_unix_seconds: None,
                    ..readback
                },
            );
        }
        let auth = source_head
            .as_ref()
            .and_then(|head| head.auth.as_ref())
            .ok_or_else(|| corrupt("v3 auth alias typed source absent"))?;
        auth.q.verify(&self.root)?;
        auth.result
            .as_ref()
            .ok_or_else(|| corrupt("v3 auth alias result absent"))?
            .require_present(&self.root)?;
        Ok(readback)
    }

    /// Publish the exact physical intent and account debt before the broker
    /// can consume K. The source was pinned by v3 admission, not supplied by
    /// this request. An existing fresh Q or any physical-account debt refuses
    /// an additional probe.
    pub(crate) fn announce_quota_effect(
        &self,
        binding: &super::super::fresh_provider::Binding,
        request: &oulipoly_kernel_broker::protocol::FreshAccountEffectRequest,
        id: &str,
    ) -> Result<u64> {
        use oulipoly_kernel_broker::protocol::FreshAccountEffectKind;
        if !matches!(
            request.kind,
            FreshAccountEffectKind::QuotaFirst
                | FreshAccountEffectKind::AuthRefresh
                | FreshAccountEffectKind::QuotaRetry
        ) {
            return Err(IndexError::Conflict("v3 account effect kind closed"));
        }
        self.check_current()?;
        let source = self.admitted_source()?;
        let (physical, source_key, account, intent, grant) =
            super::super::fresh_provider::v3_effect_physical_readback(
                &self.root, source, binding, request, id,
            )
            .map_err(|e| corrupt(format!("v3 quota intent: {e}")))?;
        if grant.is_some() || physical.state != "unknown" {
            return Err(IndexError::Conflict("v3 quota intent already spent"));
        }
        self.ensure_quota_account(&account, &request.model, &request.config_sha256)?;
        let store = self.account(&account)?;
        let digest = keyed(&source_key)?;
        let (revision, pending_count, mut values) = store.read_many(&[
            ("account", "summary"),
            ("source", &digest),
            ("effect", id),
            ("auth-marker", "account"),
        ])?;
        let auth_marker: Option<MarkerHead> = typed(values.pop().unwrap())?;
        if values.pop().unwrap().is_some() {
            return Err(IndexError::Conflict("v3 quota effect already announced"));
        }
        let source_head: Option<SourceHead> = typed(values.pop().unwrap())?;
        let mut summary = values.pop().unwrap().ok_or(IndexError::RebuildRequired(
            "v3 quota account summary absent",
        ))?;
        if summary["physical_key"].as_str() != Some(account.as_str())
            || summary["pending_count"].as_u64() != Some(pending_count)
            || pending_count != 0
            || summary["unknown_marker_scope"].as_bool() != Some(false)
        {
            return Err(IndexError::Conflict(
                "v3 quota account has debt or unknown scope",
            ));
        }
        if request.kind == FreshAccountEffectKind::AuthRefresh {
            let head = source_head
                .as_ref()
                .ok_or(IndexError::Conflict("v3 auth has no quota Q"))?;
            self.check_auth_prerequisite(head, auth_marker.as_ref())?;
        }
        if request.kind == FreshAccountEffectKind::QuotaRetry {
            let head = source_head
                .as_ref()
                .ok_or(IndexError::Conflict("v3 retry has no quota Q"))?;
            let quota = head
                .quota
                .as_ref()
                .ok_or(IndexError::Conflict("v3 retry has no quota Q"))?;
            let auth = head
                .auth
                .as_ref()
                .ok_or(IndexError::Conflict("v3 retry has no auth Q"))?;
            quota.q.verify(&self.root)?;
            auth.q.verify(&self.root)?;
            auth.result
                .as_ref()
                .ok_or(IndexError::Conflict("v3 retry auth result absent"))?
                .require_present(&self.root)?;
            if auth.outcome != "refreshed"
                || auth.q.completed_unix_nanos <= quota.q.completed_unix_nanos
                || auth_marker.as_ref().is_some_and(|marker| {
                    marker.q.completed_unix_nanos >= auth.q.completed_unix_nanos
                })
            {
                return Err(IndexError::Conflict(
                    "v3 retry has no newer successful auth Q",
                ));
            }
        }
        if request.kind == FreshAccountEffectKind::QuotaFirst {
            if let Some(head) = &source_head {
                if head.source != source_key {
                    return Err(corrupt("v3 quota source key changed"));
                }
                if let Some(quota) = &head.quota {
                    quota.q.verify(&self.root)?;
                    quota
                        .result
                        .as_ref()
                        .ok_or(IndexError::Conflict("v3 quota source result absent"))?
                        .require_present(&self.root)?;
                    let now = Utc::now().timestamp();
                    if quota.outcome == "valid_windows"
                        && quota.completed_unix_seconds.is_some_and(|completed| {
                            now >= completed && now - completed < 5 * 60 * 60
                        })
                        && !quota.windows.is_empty()
                        && quota
                            .windows
                            .iter()
                            .all(|window| window.reset_unix_seconds > now)
                    {
                        // A fresh full window is still a cached Q. It excludes
                        // routing; it must not trigger an automatic second probe.
                        return Err(IndexError::Conflict("v3 quota source has fresh Q"));
                    }
                }
            }
        }
        let candidate = Artifact::from_existing(
            &self.root,
            Path::new(&format!(
                "{}.route-{}.json",
                binding.handoff_id, request.index
            )),
        )?;
        let route_source = Artifact::from_existing(
            &self.root,
            Path::new(&format!("{}.route-source.json", binding.handoff_id)),
        )?;
        let effect = EffectIntent {
            kind: if request.kind == FreshAccountEffectKind::AuthRefresh {
                EffectKind::Auth
            } else {
                EffectKind::Quota
            },
            source: source_key.clone(),
            decision_handoff: binding.handoff_id.clone(),
            route_source: Some(route_source),
            candidate: Some(candidate.clone()),
            intent: intent.clone(),
            reuse: None,
            consumed_k: None,
            certified_q: None,
            result: None,
        };
        let pending = PendingHead {
            announcement: intent,
            candidate: Some(candidate),
            physical_k: None,
            source: Some(source_key),
            model: Some(request.model.clone()),
            config_sha256: Some(request.config_sha256.clone()),
            decision_handoff: binding.handoff_id.clone(),
            kind: if request.kind == FreshAccountEffectKind::AuthRefresh {
                "Auth"
            } else {
                "Quota"
            }
            .into(),
        };
        let effect_count = summary["effect_count"]
            .as_u64()
            .ok_or_else(|| corrupt("v3 quota effect count absent"))?;
        summary["effect_count"] = (effect_count
            .checked_add(1)
            .ok_or(IndexError::Conflict("v3 quota effect count overflow"))?)
        .into();
        summary["pending_count"] = 1.into();
        let next = store.commit(revision, {
            let mut changes = vec![
                change("effect", id, &effect)?,
                change("pending", &format!("effect:{id}"), &pending)?,
                change("account", "summary", &summary)?,
            ];
            if request.kind == FreshAccountEffectKind::AuthRefresh {
                changes.push(change("auth-flight", "account", &effect.intent)?);
            }
            changes
        })?;
        if store.summary()? != (next, 1) {
            return Err(corrupt("v3 quota intent commit readback differs"));
        }
        self.check_current()?;
        Ok(next)
    }

    /// This is called immediately after the fsynced physical K, before the
    /// child is released. A failed CAS leaves physical K as pending debt.
    pub(crate) fn record_quota_k(
        &self,
        binding: &super::super::fresh_provider::Binding,
        request: &oulipoly_kernel_broker::protocol::FreshAccountEffectRequest,
        id: &str,
        expected_revision: Option<u64>,
    ) -> Result<()> {
        self.check_current()?;
        let (_, source, account, intent, grant) =
            super::super::fresh_provider::v3_effect_physical_readback(
                &self.root,
                self.admitted_source()?,
                binding,
                request,
                id,
            )
            .map_err(|e| corrupt(format!("v3 quota K: {e}")))?;
        let grant = grant.ok_or(IndexError::Conflict("v3 quota physical K absent"))?;
        let kind = match request.kind {
            FreshAccountEffectKind::AuthRefresh => "auth-refresh",
            FreshAccountEffectKind::QuotaFirst => "quota-first",
            FreshAccountEffectKind::QuotaRetry => "quota-retry",
        };
        let k = Artifact::from_existing(
            &self.root,
            Path::new(&format!(
                "account-effects/{}-{}-{kind}/{grant}.consumed.json",
                binding.handoff_id, request.index
            )),
        )?;
        let store = self.account(&account)?;
        let (revision, pending_count, mut values) =
            store.read_many(&[("effect", id), ("pending", &format!("effect:{id}"))])?;
        let mut pending: PendingHead = typed(values.pop().unwrap())?
            .ok_or(IndexError::Conflict("v3 quota K pending debt absent"))?;
        let mut effect: EffectIntent = typed(values.pop().unwrap())?
            .ok_or(IndexError::Conflict("v3 quota K announcement absent"))?;
        if expected_revision.is_some_and(|expected| expected != revision)
            || pending_count == 0
            || effect.intent != intent
            || effect.source != source
            || pending.announcement != intent
            || effect.result.is_some()
        {
            return Err(IndexError::Conflict(
                "v3 quota K account revision or intent changed",
            ));
        }
        if effect.consumed_k.as_ref() == Some(&k) && pending.physical_k.as_ref() == Some(&k) {
            return Ok(());
        }
        if effect.consumed_k.is_some()
            || pending.physical_k.is_some()
            || effect.kind
                != if request.kind == FreshAccountEffectKind::AuthRefresh {
                    EffectKind::Auth
                } else {
                    EffectKind::Quota
                }
            || pending.kind
                != if request.kind == FreshAccountEffectKind::AuthRefresh {
                    "Auth"
                } else {
                    "Quota"
                }
        {
            return Err(corrupt("v3 quota K differs"));
        }
        #[cfg(feature = "age319-private-broker-fixture")]
        if (request.kind == FreshAccountEffectKind::AuthRefresh
            && std::env::var_os("OULIPOLY_KERNEL_BROKER_FIXTURE_FAIL_AUTH_POST_K_CAS_V3_V1")
                .is_some())
            || (request.kind != FreshAccountEffectKind::AuthRefresh
                && std::env::var_os("OULIPOLY_KERNEL_BROKER_FIXTURE_FAIL_QUOTA_POST_K_CAS_V3_V1")
                    .is_some())
        {
            return Err(IndexError::Conflict(
                "fixture v3 account post-K CAS failure",
            ));
        }
        effect.consumed_k = Some(k.clone());
        pending.physical_k = Some(k);
        store.commit(
            revision,
            vec![
                change("effect", id, &effect)?,
                change("pending", &format!("effect:{id}"), &pending)?,
            ],
        )?;
        self.check_current()?;
        Ok(())
    }

    pub(crate) fn require_quota_pre_k(
        &self,
        binding: &super::super::fresh_provider::Binding,
        request: &oulipoly_kernel_broker::protocol::FreshAccountEffectRequest,
        id: &str,
        revision: u64,
    ) -> Result<()> {
        if self
            .latest_effect_checkpoint(self.admitted_source()?, binding, request, id)?
            .is_some()
        {
            return Err(IndexError::Conflict("v3 quota already settled before K"));
        }
        let (_, _, account, _, grant) = super::super::fresh_provider::v3_effect_physical_readback(
            &self.root,
            self.admitted_source()?,
            binding,
            request,
            id,
        )
        .map_err(|e| corrupt(format!("v3 quota pre-K: {e}")))?;
        if grant.is_none() || self.account(&account)?.summary()?.0 != revision {
            return Err(IndexError::Conflict(
                "v3 quota pre-K account revision changed",
            ));
        }
        Ok(())
    }

    /// Reconcile exactly one announced K/Q/result. No scan, reuse or launch is
    /// possible here. A missing Q/result retains pending debt.
    pub(crate) fn settle_quota_effect(
        &self,
        binding: &super::super::fresh_provider::Binding,
        request: &oulipoly_kernel_broker::protocol::FreshAccountEffectRequest,
        id: &str,
    ) -> Result<Option<oulipoly_kernel_broker::protocol::FreshAccountEffectReadback>> {
        self.check_current()?;
        let source = self.admitted_source()?;
        let (physical, source_key, account, intent, grant) =
            super::super::fresh_provider::v3_effect_physical_readback(
                &self.root, source, binding, request, id,
            )
            .map_err(|e| corrupt(format!("v3 quota observation: {e}")))?;
        let Some(grant) = grant else {
            return Ok(None);
        };
        self.record_quota_k(binding, request, id, None)?;
        if physical.state != "drained" {
            return Ok(None);
        }
        super::super::fresh_provider::v3_materialize_quota_result(&self.root, binding, request, id)
            .map_err(|e| corrupt(format!("v3 quota result: {e}")))?;
        let dir = Path::new(&intent.path)
            .parent()
            .ok_or_else(|| corrupt("v3 quota intent parent absent"))?;
        let k = Artifact::from_existing(&self.root, &dir.join(format!("{grant}.consumed.json")))?;
        let q_path = dir.join(format!("{grant}.drain.json"));
        let q = PhysicalQ {
            physical_k: k.clone(),
            q: Artifact::from_existing(&self.root, &q_path)?,
            terminal: None,
            completed_unix_nanos: i64::try_from(
                super::super::fresh_provider::file_unix_nanos(&self.root.join(&q_path))
                    .map_err(|e| corrupt(e.to_string()))?,
            )
            .map_err(|_| corrupt("v3 quota Q time overflow"))?,
        };
        let result = Artifact::from_existing(&self.root, &dir.join("result.json"))?;
        let store = self.account(&account)?;
        let digest = keyed(&source_key)?;
        let (revision, pending_count, mut values) = store.read_many(&[
            ("account", "summary"),
            ("effect", id),
            ("pending", &format!("effect:{id}")),
            ("source", &digest),
        ])?;
        let prior_source: Option<SourceHead> = typed(values.pop().unwrap())?;
        let pending: Option<PendingHead> = typed(values.pop().unwrap())?;
        let mut effect: EffectIntent = typed(values.pop().unwrap())?.ok_or(
            IndexError::Conflict("v3 quota settlement announcement absent"),
        )?;
        let mut summary = values
            .pop()
            .unwrap()
            .ok_or(IndexError::RebuildRequired("v3 quota summary absent"))?;
        if effect.certified_q.as_ref() == Some(&q) && effect.result.as_ref() == Some(&result) {
            return self.latest_effect_checkpoint(source, binding, request, id);
        }
        if effect.intent != intent
            || effect.source != source_key
            || effect.consumed_k.as_ref() != Some(&k)
            || effect.certified_q.is_some()
            || effect.result.is_some()
            || pending
                .as_ref()
                .is_none_or(|p| p.announcement != intent || p.physical_k.as_ref() != Some(&k))
            || pending_count == 0
            || summary["pending_count"].as_u64() != Some(pending_count)
        {
            return Err(IndexError::Conflict("v3 quota Q/K/debt changed"));
        }
        let windows = physical
            .windows
            .iter()
            .map(|window| {
                let reset = DateTime::parse_from_rfc3339(&window.resets_at)
                    .map_err(|_| corrupt("v3 quota typed reset invalid"))?;
                Ok(WindowHead {
                    used_percent: window.used_percent,
                    resets_at: window.resets_at.clone(),
                    reset_unix_seconds: reset.timestamp(),
                    remaining: window.remaining,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let observation = ObservationHead {
            q: q.clone(),
            result: Some(result.clone()),
            outcome: physical.outcome.clone().unwrap_or_else(|| "unknown".into()),
            origin_model: Some(request.model.clone()),
            origin_config_sha256: Some(request.config_sha256.clone()),
            completed_unix_seconds: physical.completed_unix_seconds,
            windows,
        };
        let mut head = prior_source.clone().unwrap_or(SourceHead {
            source: source_key.clone(),
            quota: None,
            auth: None,
        });
        if head.source != source_key {
            return Err(corrupt("v3 quota source identity changed"));
        }
        let slot = if request.kind == FreshAccountEffectKind::AuthRefresh {
            &mut head.auth
        } else {
            &mut head.quota
        };
        if slot
            .as_ref()
            .is_none_or(|old| old.q.completed_unix_nanos <= q.completed_unix_nanos)
        {
            *slot = Some(observation);
        }
        effect.certified_q = Some(q);
        effect.result = Some(result);
        summary["pending_count"] = (pending_count - 1).into();
        if prior_source.is_none() {
            let count = summary["source_count"]
                .as_u64()
                .ok_or_else(|| corrupt("v3 quota source count absent"))?;
            summary["source_count"] = count
                .checked_add(1)
                .ok_or(IndexError::Conflict("v3 quota source count overflow"))?
                .into();
        }
        store.commit(revision, {
            let mut changes = vec![
                change("effect", id, &effect)?,
                Change {
                    class: "pending".into(),
                    key: format!("effect:{id}"),
                    value: None,
                },
                change("source", &digest, &head)?,
                change("account", "summary", &summary)?,
            ];
            if request.kind == FreshAccountEffectKind::AuthRefresh {
                changes.push(Change {
                    class: "auth-flight".into(),
                    key: "account".into(),
                    value: None,
                });
            }
            changes
        })?;
        self.latest_effect_checkpoint(source, binding, request, id)
    }

    /// Certify the latest typed quota/auth observation for one exact retained
    /// physical effect. A physical Q that arrived after keyed publication is
    /// still debt until a separate writer settles the same K/Q/result. This
    /// read never scans retained effect history or materializes result.json.
    pub(crate) fn latest_effect_checkpoint(
        &self,
        source: &Path,
        binding: &super::super::fresh_provider::Binding,
        request: &oulipoly_kernel_broker::protocol::FreshAccountEffectRequest,
        effect_id: &str,
    ) -> Result<Option<oulipoly_kernel_broker::protocol::FreshAccountEffectReadback>> {
        self.check_current()?;
        let (physical, source_key, physical_key, intent_artifact, grant_id) =
            super::super::fresh_provider::v3_effect_physical_readback(
                &self.root, source, binding, request, effect_id,
            )
            .map_err(|error| corrupt(format!("v3 effect physical readback: {error}")))?;
        let store = self.account(&physical_key)?;
        let source_digest = keyed(&source_key)?;
        let (revision, pending_count, mut values) = store.read_many(&[
            ("account", "summary"),
            ("effect", effect_id),
            ("pending", &format!("effect:{effect_id}")),
            ("source", &source_digest),
        ])?;
        let source_head: Option<SourceHead> = typed(values.pop().unwrap())?;
        let pending: Option<PendingHead> = typed(values.pop().unwrap())?;
        let effect: EffectIntent = typed(values.pop().unwrap())?
            .ok_or(IndexError::RebuildRequired("v3 effect announcement absent"))?;
        let summary = values.pop().unwrap().ok_or(IndexError::RebuildRequired(
            "v3 effect account summary absent",
        ))?;
        if summary["physical_key"].as_str() != Some(physical_key.as_str())
            || summary["pending_count"].as_u64() != Some(pending_count)
            || effect.source != source_key
            || effect.intent != intent_artifact
            || effect.decision_handoff != binding.handoff_id
        {
            return Err(corrupt("v3 effect source, account or announcement differs"));
        }
        let expected_kind = match request.kind {
            oulipoly_kernel_broker::protocol::FreshAccountEffectKind::AuthRefresh => {
                EffectKind::Auth
            }
            _ => EffectKind::Quota,
        };
        if effect.kind != expected_kind || effect.reuse.is_some() {
            return Err(corrupt("v3 effect kind or reuse differs"));
        }
        effect.intent.require_present(&self.root)?;
        for (indexed, path) in [
            (
                &effect.candidate,
                format!("{}.route-{}.json", binding.handoff_id, request.index),
            ),
            (
                &effect.route_source,
                format!("{}.route-source.json", binding.handoff_id),
            ),
        ] {
            let expected = Artifact::from_existing(&self.root, Path::new(&path))?;
            if indexed.as_ref() != Some(&expected) {
                return Err(corrupt("v3 effect candidate or route source differs"));
            }
        }
        let result = if let (Some(q), Some(result)) = (&effect.certified_q, &effect.result) {
            if pending.is_some() || effect.consumed_k.as_ref() != Some(&q.physical_k) {
                return Err(corrupt("v3 settled effect has pending or different K"));
            }
            let grant_id =
                grant_id.ok_or_else(|| corrupt("v3 settled effect physical grant absent"))?;
            let parent = Path::new(&effect.intent.path)
                .parent()
                .ok_or_else(|| corrupt("v3 effect intent parent absent"))?;
            let expected_k = Artifact::from_existing(
                &self.root,
                &parent.join(format!("{grant_id}.consumed.json")),
            )?;
            let expected_q = Artifact::from_existing(
                &self.root,
                &parent.join(format!("{grant_id}.drain.json")),
            )?;
            if q.physical_k != expected_k || q.q != expected_q || q.terminal.is_some() {
                return Err(corrupt("v3 effect physical K/Q differs"));
            }
            q.verify(&self.root)?;
            let expected_result_artifact =
                Artifact::from_existing(&self.root, &parent.join("result.json"))?;
            if result != &expected_result_artifact {
                return Err(corrupt("v3 effect result artifact differs"));
            }
            result.require_present(&self.root)?;
            let expected_result = result
                .read_json::<oulipoly_kernel_broker::protocol::FreshAccountEffectReadback>(
                &self.root,
            )?;
            if serde_json::to_value(&expected_result).map_err(|e| corrupt(e.to_string()))?
                != serde_json::to_value(&physical).map_err(|e| corrupt(e.to_string()))?
                || physical.state != "drained"
            {
                return Err(corrupt("v3 effect typed result differs from physical Q"));
            }
            let observation = source_head.as_ref().and_then(|head| {
                if head.source != source_key {
                    return None;
                }
                match effect.kind {
                    EffectKind::Quota => head.quota.as_ref(),
                    EffectKind::Auth => head.auth.as_ref(),
                    EffectKind::ManualQuota => None,
                }
            });
            match observation {
                Some(observation) if observation.q == *q => {
                    if observation.result.as_ref() != Some(result)
                        || observation.outcome != physical.outcome.as_deref().unwrap_or("")
                        || observation.completed_unix_seconds != physical.completed_unix_seconds
                        || observation.origin_model.as_deref() != Some(request.model.as_str())
                        || observation.origin_config_sha256.as_deref()
                            != Some(request.config_sha256.as_str())
                        || observation.windows.len() != physical.windows.len()
                        || observation.windows.iter().zip(&physical.windows).any(
                            |(indexed, observed)| {
                                indexed.used_percent != observed.used_percent
                                    || indexed.resets_at != observed.resets_at
                                    || DateTime::parse_from_rfc3339(&observed.resets_at)
                                        .map_or(true, |date| {
                                            date.timestamp() != indexed.reset_unix_seconds
                                        })
                                    || indexed.remaining != observed.remaining
                            },
                        )
                    {
                        return Err(corrupt("v3 effect latest typed source differs"));
                    }
                    Some(physical)
                }
                Some(_) => None, // A newer Q owns this source's current fact.
                None => return Err(corrupt("v3 settled effect typed source absent")),
            }
        } else {
            if pending
                .as_ref()
                .is_none_or(|item| item.announcement != effect.intent)
                || effect.result.is_some()
            {
                return Err(corrupt("v3 unresolved effect pending debt differs"));
            }
            if let Some(q) = &effect.certified_q {
                q.verify(&self.root)?;
            }
            None
        };
        if store.summary()? != (revision, pending_count) {
            return Err(IndexError::Conflict(
                "v3 effect account changed during readback",
            ));
        }
        self.check_current()?;
        Ok(result)
    }

    /// Read exact physical account facts under one keyed account lock. The
    /// source key must be derived from the candidate's command pair and the
    /// verified effect environment; this method does not select a candidate
    /// or publish a route receipt.
    pub(crate) fn route_facts(
        &self,
        physical_key: &str,
        source_key: Option<&SourceKey>,
        model: &str,
        config_sha256: &str,
        now: i64,
    ) -> Result<RouteEligibility> {
        self.check_current()?;
        if model.is_empty()
            || config_sha256.is_empty()
            || source_key.is_some_and(|key| !key.valid())
        {
            return Err(IndexError::Conflict("v3 route fact identity invalid"));
        }
        let source_digest = source_key.map(keyed).transpose()?.unwrap_or_default();
        let capacity_key = keyed(&(model, config_sha256))?;
        let store = self.account(physical_key)?;
        let (revision, pending, mut values) = store.read_many(&[
            ("account", "summary"),
            ("source", &source_digest),
            ("quota-marker", "account"),
            ("auth-marker", "account"),
            ("model-capacity", &capacity_key),
        ])?;
        let capacity: Option<MarkerHead> = typed(values.pop().unwrap())?;
        let auth_marker: Option<MarkerHead> = typed(values.pop().unwrap())?;
        let quota_marker: Option<MarkerHead> = typed(values.pop().unwrap())?;
        let source: Option<SourceHead> = typed(values.pop().unwrap())?;
        let summary = values
            .pop()
            .unwrap()
            .ok_or(IndexError::RebuildRequired("v3 account summary absent"))?;
        if summary["physical_key"].as_str() != Some(physical_key)
            || summary["pending_count"].as_u64() != Some(pending)
        {
            return Err(corrupt("v3 route account summary differs"));
        }
        let invocations = summary["observed_invocations"]
            .as_u64()
            .ok_or_else(|| corrupt("v3 route invocation count absent"))?;
        let unknown_scope = summary["unknown_marker_scope"]
            .as_bool()
            .ok_or_else(|| corrupt("v3 route marker scope absent"))?;
        if pending != 0 || unknown_scope {
            return Ok(RouteEligibility::Unknown);
        }
        for (marker, outcome) in [
            (&quota_marker, "quota_rejected"),
            (&auth_marker, "auth_rejected"),
        ] {
            if let Some(marker) = marker {
                if marker.outcome != outcome {
                    return Err(corrupt("v3 route account marker kind changed"));
                }
                marker.q.verify(&self.root)?;
            }
        }
        if let Some(marker) = capacity {
            if marker.outcome != "model_at_capacity"
                || marker.model != model
                || marker.config_sha256 != config_sha256
            {
                return Err(corrupt("v3 route capacity marker scope changed"));
            }
            marker.q.verify(&self.root)?;
            // A fresh quota Q says nothing about model capacity. A later
            // provider result must explicitly retire this marker.
            return Ok(RouteEligibility::Excluded);
        }
        let Some(key) = source_key else {
            if source.is_some() {
                return Err(corrupt("v3 unmetered route has keyed source"));
            }
            if quota_marker.is_some() || auth_marker.is_some() {
                return Ok(RouteEligibility::Excluded);
            }
            return Ok(RouteEligibility::Eligible {
                account_revision: revision,
                quota_basis_points: None,
                observed_invocations: invocations,
            });
        };
        let Some(source) = source else {
            return Ok(RouteEligibility::ProbeRequired);
        };
        if source.source != *key {
            return Err(corrupt("v3 route source identity changed"));
        }
        let Some(quota) = source.quota else {
            return Ok(RouteEligibility::ProbeRequired);
        };
        quota.q.verify(&self.root)?;
        let Some(result) = &quota.result else {
            return Ok(RouteEligibility::Unknown);
        };
        result.require_present(&self.root)?;
        if quota.origin_model.as_deref().is_none_or(str::is_empty)
            || quota
                .origin_config_sha256
                .as_deref()
                .is_none_or(str::is_empty)
        {
            return Ok(RouteEligibility::Unknown);
        }
        if quota.outcome == "unknown" {
            return Ok(RouteEligibility::Unknown);
        }
        if quota.outcome != "valid_windows" || quota.windows.is_empty() {
            return Ok(RouteEligibility::ProbeRequired);
        }
        let Some(completed) = quota.completed_unix_seconds else {
            return Ok(RouteEligibility::Unknown);
        };
        if quota
            .windows
            .iter()
            .any(|window| window.used_percent >= 100.0 || window.remaining == Some(0))
        {
            return Ok(RouteEligibility::Excluded);
        }
        if now < completed
            || now
                .checked_sub(completed)
                .is_none_or(|age| age >= 5 * 60 * 60)
        {
            return Ok(RouteEligibility::ProbeRequired);
        }
        if quota_marker
            .as_ref()
            .is_some_and(|marker| marker.q.completed_unix_nanos >= quota.q.completed_unix_nanos)
        {
            return Ok(RouteEligibility::Excluded);
        }
        if let Some(marker) = auth_marker {
            let Some(auth) = source.auth else {
                return Ok(RouteEligibility::Excluded);
            };
            auth.q.verify(&self.root)?;
            let Some(auth_result) = &auth.result else {
                return Ok(RouteEligibility::Unknown);
            };
            auth_result.require_present(&self.root)?;
            if auth.outcome != "refreshed"
                || auth.origin_model.as_deref().is_none_or(str::is_empty)
                || auth
                    .origin_config_sha256
                    .as_deref()
                    .is_none_or(str::is_empty)
                || auth.q.completed_unix_nanos <= marker.q.completed_unix_nanos
            {
                return Ok(RouteEligibility::Excluded);
            }
        }
        let mut basis = u32::MAX;
        for window in &quota.windows {
            if !window.used_percent.is_finite()
                || !(0.0..=100.0).contains(&window.used_percent)
                || DateTime::parse_from_rfc3339(&window.resets_at)
                    .map_or(true, |date| date.timestamp() != window.reset_unix_seconds)
            {
                return Err(corrupt("v3 route typed window invalid"));
            }
            if window.reset_unix_seconds <= now {
                return Ok(RouteEligibility::ProbeRequired);
            }
            basis = basis.min(((100.0 - window.used_percent) * 100.0).round() as u32);
        }
        Ok(RouteEligibility::Eligible {
            account_revision: revision,
            quota_basis_points: Some(basis),
            observed_invocations: invocations,
        })
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
            let mut lag_effects = BTreeMap::<String, EffectIntent>::new();
            let mut lag_sources = std::collections::HashSet::<String>::new();
            let mut lag_auth_sources = std::collections::HashSet::<String>::new();
            let (revision, pending_count) = store.summary()?;
            let summary = store
                .get("account", "summary")?
                .ok_or(IndexError::RebuildRequired("v3 account summary absent"))?;
            if revision == 0 || summary["pending_count"].as_u64() != Some(pending_count) {
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
                let actual: EffectIntent = typed(store.get(class, id)?)?
                    .ok_or_else(|| corrupt("v3 retained account effect unannounced"))?;
                if actual != *effect {
                    let mut common = effect.clone();
                    common.consumed_k = actual.consumed_k.clone();
                    common.certified_q = actual.certified_q.clone();
                    common.result = actual.result.clone();
                    if class != "effect"
                        || !matches!(effect.kind, EffectKind::Quota | EffectKind::Auth)
                        || effect.reuse.is_some()
                        || common != actual
                        || actual.consumed_k.is_some() && actual.consumed_k != effect.consumed_k
                        || actual.certified_q.is_some() && actual.certified_q != effect.certified_q
                        || actual.result.is_some() && actual.result != effect.result
                    {
                        return Err(corrupt("v3 retained account effect differs"));
                    }
                    lag_sources.insert(keyed(&effect.source)?);
                    if effect.kind == EffectKind::Auth {
                        lag_auth_sources.insert(keyed(&effect.source)?);
                    }
                    lag_effects.insert(id.clone(), actual);
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
                let actual_uncertain = store.get("uncertain-q", &format!("effect:{id}"))?;
                if actual_uncertain.is_some() {
                    expected_keys.insert(("uncertain-q".to_owned(), format!("effect:{id}")));
                }
                if actual_uncertain != expected
                    && !(lag_effects.contains_key(id) && actual_uncertain.is_none())
                {
                    return Err(corrupt("v3 retained account Q debt differs"));
                }
            }
            for (id, pending) in &head.pending {
                if id
                    .strip_prefix("effect:")
                    .is_some_and(|effect| lag_effects.contains_key(effect))
                {
                    continue;
                }
                expected_keys.insert(("pending".to_owned(), id.clone()));
                if store.get("pending", id)?
                    != Some(serde_json::to_value(pending).map_err(|e| corrupt(e.to_string()))?)
                {
                    return Err(corrupt("v3 retained pending source differs"));
                }
            }
            let mut auth_flights: Vec<_> = head
                .pending
                .values()
                .filter(|pending| pending.kind == "Auth")
                .map(|pending| pending.announcement.clone())
                .collect();
            auth_flights.extend(
                lag_effects
                    .values()
                    .filter(|effect| effect.kind == EffectKind::Auth)
                    .map(|effect| effect.intent.clone()),
            );
            auth_flights.sort_by(|a, b| a.path.cmp(&b.path));
            auth_flights.dedup();
            let expected_auth_flight = (auth_flights.len() == 1).then(|| auth_flights[0].clone());
            if store.get("auth-flight", "account")?
                != expected_auth_flight
                    .as_ref()
                    .map(|artifact| {
                        serde_json::to_value(artifact).map_err(|e| corrupt(e.to_string()))
                    })
                    .transpose()?
            {
                return Err(corrupt("v3 auth flight pointer differs"));
            }
            if expected_auth_flight.is_some() {
                expected_keys.insert(("auth-flight".to_owned(), "account".to_owned()));
            }
            for (id, effect) in &lag_effects {
                let pending_key = format!("effect:{id}");
                let pending: PendingHead = typed(store.get("pending", &pending_key)?)?
                    .ok_or_else(|| corrupt("v3 lagging quota lost pending debt"))?;
                if pending.announcement != effect.intent
                    || pending.candidate != effect.candidate
                    || pending.physical_k != effect.consumed_k
                    || pending.source.as_ref() != Some(&effect.source)
                    || pending.decision_handoff != effect.decision_handoff
                    || pending.kind
                        != if effect.kind == EffectKind::Auth {
                            "Auth"
                        } else {
                            "Quota"
                        }
                {
                    return Err(corrupt("v3 lagging quota pending debt differs"));
                }
                expected_keys.insert(("pending".to_owned(), pending_key));
            }
            for (id, typed) in &head.sources {
                let actual: Option<SourceHead> = self::typed(store.get("source", id)?)?;
                if actual.as_ref() != Some(typed) {
                    if !lag_sources.contains(id)
                        || actual.as_ref().is_some_and(|old| {
                            old.source != typed.source
                                || (old.auth != typed.auth
                                    && (!lag_auth_sources.contains(id)
                                        || old.auth.as_ref().zip(typed.auth.as_ref()).is_some_and(
                                            |(before, after)| {
                                                before.q.completed_unix_nanos
                                                    >= after.q.completed_unix_nanos
                                            },
                                        )))
                                || old.quota.as_ref().zip(typed.quota.as_ref()).is_some_and(
                                    |(before, after)| {
                                        before.q.completed_unix_nanos
                                            >= after.q.completed_unix_nanos
                                    },
                                )
                        })
                    {
                        return Err(corrupt("v3 retained typed source differs"));
                    }
                }
                if actual.is_some() {
                    expected_keys.insert(("source".to_owned(), id.clone()));
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
            let live_pending = expected_keys
                .iter()
                .filter(|(class, _)| class == "pending")
                .count();
            let live_sources = expected_keys
                .iter()
                .filter(|(class, _)| class == "source")
                .count();
            if pending_count != live_pending as u64 {
                return Err(corrupt("v3 retained pending count differs"));
            }
            let expected_summary = serde_json::json!({
                "physical_key": key,
                "observed_invocations": account.observed_invocations,
                "marker_times": account.markers,
                "failure_count": failure_count,
                "unknown_marker_scope": head.unknown_marker_scope,
                "grant_count": account.grants.len(),
                "effect_count": account.effects.len(),
                "pending_count": live_pending,
                "source_count": live_sources,
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
    let auth_flights: Vec<_> = head
        .pending
        .values()
        .filter(|pending| pending.kind == "Auth")
        .collect();
    if auth_flights.len() == 1 {
        insert_checked(
            &store,
            &mut revision,
            change("auth-flight", "account", &auth_flights[0].announcement)?,
        )?;
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

    fn route_summary(pending: u64) -> serde_json::Value {
        serde_json::json!({
            "physical_key": "physical", "pending_count": pending,
            "observed_invocations": 3, "unknown_marker_scope": false
        })
    }

    fn route_fixture(history: usize) -> (tempfile::TempDir, KeyedGeneration, SourceKey, u64) {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().to_path_buf();
        let generation = uuid::Uuid::new_v4().to_string();
        let storage = root.join("index-v1/generations").join(&generation);
        let accounts = storage.join("keyed-accounts");
        fs::create_dir_all(&accounts).unwrap();
        write_new(
            &root.join("index-v1/manifest.json"),
            &Manifest {
                version: V3,
                generation: generation.clone(),
                generation_dir: true,
            },
        )
        .unwrap();
        let store = KeyedAccountStore::create(
            &accounts.join(keyed(&"physical").unwrap()),
            &generation,
            "physical",
        )
        .unwrap();
        let k = artifact(&root, "route-k.json", b"K");
        let q = artifact(&root, "route-q.json", b"Q");
        let physical_q = PhysicalQ {
            physical_k: k,
            q,
            terminal: None,
            completed_unix_nanos: 1_800_000_000_000_000_000,
        };
        let result = artifact(&root, "route-result.json", b"typed-result");
        let observation = ObservationHead {
            q: physical_q,
            result: Some(result.clone()),
            outcome: "valid_windows".into(),
            origin_model: Some("model-a".into()),
            origin_config_sha256: Some("config".into()),
            completed_unix_seconds: Some(1_800_000_000),
            windows: vec![WindowHead {
                used_percent: 20.0,
                resets_at: "2099-01-01T00:00:00Z".into(),
                reset_unix_seconds: 4_070_908_800,
                remaining: Some(80),
            }],
        };
        let key = SourceKey {
            commands_sha256: "commands".into(),
            environment_sha256: "environment".into(),
        };
        let mut revision = store
            .commit(
                0,
                vec![change("account", "summary", &route_summary(0)).unwrap()],
            )
            .unwrap();
        for n in 0..history {
            let old = SourceKey {
                commands_sha256: format!("old-{n}"),
                environment_sha256: "environment".into(),
            };
            revision = store
                .commit(
                    revision,
                    vec![
                        change(
                            "source",
                            &keyed(&old).unwrap(),
                            &SourceHead {
                                source: old,
                                quota: Some(observation.clone()),
                                auth: None,
                            },
                        )
                        .unwrap(),
                    ],
                )
                .unwrap();
        }
        revision = store
            .commit(
                revision,
                vec![
                    change(
                        "source",
                        &keyed(&key).unwrap(),
                        &SourceHead {
                            source: key.clone(),
                            quota: Some(observation),
                            auth: None,
                        },
                    )
                    .unwrap(),
                ],
            )
            .unwrap();
        (
            temp,
            KeyedGeneration {
                generation,
                root,
                storage,
                source: None,
            },
            key,
            revision,
        )
    }

    #[test]
    fn v3_route_facts_are_keyed_and_fail_closed_on_debt_invalid_q_and_markers() {
        let (_small, small, source, _) = route_fixture(1);
        let (_large, large, large_source, revision) = route_fixture(205);
        let now = 1_800_000_100;
        let small_guard = ReaderIoGuard::start("v3-route-facts-small");
        let (small_result, small_io) = super::super::keyed_store::measured(|| {
            small
                .route_facts("physical", Some(&source), "model-a", "config", now)
                .unwrap()
        });
        drop(small_guard);
        let small_physical_io = last_reader_io().unwrap();
        let large_guard = ReaderIoGuard::start("v3-route-facts-many");
        let (large_result, large_io) = super::super::keyed_store::measured(|| {
            large
                .route_facts("physical", Some(&large_source), "model-a", "config", now)
                .unwrap()
        });
        drop(large_guard);
        let large_physical_io = last_reader_io().unwrap();
        assert!(matches!(
            small_result,
            RouteEligibility::Eligible {
                quota_basis_points: Some(8000),
                ..
            }
        ));
        assert_eq!(
            large_result,
            RouteEligibility::Eligible {
                account_revision: revision,
                quota_basis_points: Some(8000),
                observed_invocations: 3
            }
        );
        assert_eq!(small_io.open_attempts, large_io.open_attempts);
        assert_eq!(small_io.opened, large_io.opened);
        assert_eq!(small_io.directory_entries, 0);
        assert_eq!(large_io.directory_entries, 0);
        assert!(large_io.bytes_parsed <= small_io.bytes_parsed + 256);
        assert_eq!(
            small_physical_io.open_attempts,
            large_physical_io.open_attempts
        );
        assert_eq!(small_physical_io.opened, large_physical_io.opened);
        assert_eq!(small_physical_io.directory_entries, 0);
        assert_eq!(large_physical_io.directory_entries, 0);
        eprintln!("v3 route facts keyed 1/205 sources: {small_io:?} / {large_io:?}");
        eprintln!(
            "v3 route facts manifest/physical 1/205 sources: {small_physical_io:?} / {large_physical_io:?}"
        );
        assert_eq!(
            large
                .route_facts(
                    "physical",
                    Some(&large_source),
                    "model-a",
                    "config",
                    now + 5 * 60 * 60
                )
                .unwrap(),
            RouteEligibility::ProbeRequired
        );

        let store = large.account("physical").unwrap();
        let mut revision = store
            .commit(
                revision,
                vec![
                    change(
                        "pending",
                        "effect:new",
                        &serde_json::json!({"unresolved":"K"}),
                    )
                    .unwrap(),
                    change("account", "summary", &route_summary(1)).unwrap(),
                ],
            )
            .unwrap();
        assert_eq!(
            large
                .route_facts("physical", Some(&large_source), "model-a", "config", now)
                .unwrap(),
            RouteEligibility::Unknown
        );
        revision = store
            .commit(
                revision,
                vec![
                    Change {
                        class: "pending".into(),
                        key: "effect:new".into(),
                        value: None,
                    },
                    change("account", "summary", &route_summary(0)).unwrap(),
                ],
            )
            .unwrap();
        let mut source_head: SourceHead =
            typed(store.get("source", &keyed(&large_source).unwrap()).unwrap())
                .unwrap()
                .unwrap();
        source_head.quota.as_mut().unwrap().outcome = "invalid".into();
        revision = store
            .commit(
                revision,
                vec![change("source", &keyed(&large_source).unwrap(), &source_head).unwrap()],
            )
            .unwrap();
        assert_eq!(
            large
                .route_facts("physical", Some(&large_source), "model-a", "config", now)
                .unwrap(),
            RouteEligibility::ProbeRequired
        );
        source_head.quota.as_mut().unwrap().outcome = "valid_windows".into();
        source_head.quota.as_mut().unwrap().windows[0].used_percent = 100.0;
        revision = store
            .commit(
                revision,
                vec![change("source", &keyed(&large_source).unwrap(), &source_head).unwrap()],
            )
            .unwrap();
        assert_eq!(
            large
                .route_facts("physical", Some(&large_source), "model-a", "config", now)
                .unwrap(),
            RouteEligibility::Excluded
        );
        source_head.quota.as_mut().unwrap().windows[0].used_percent = 20.0;
        revision = store
            .commit(
                revision,
                vec![change("source", &keyed(&large_source).unwrap(), &source_head).unwrap()],
            )
            .unwrap();
        let capacity = MarkerHead {
            q: source_head.quota.as_ref().unwrap().q.clone(),
            model: "model-a".into(),
            config_sha256: "config".into(),
            outcome: "model_at_capacity".into(),
        };
        store
            .commit(
                revision,
                vec![
                    change(
                        "model-capacity",
                        &keyed(&("model-a", "config")).unwrap(),
                        &capacity,
                    )
                    .unwrap(),
                ],
            )
            .unwrap();
        assert_eq!(
            large
                .route_facts("physical", Some(&large_source), "model-a", "config", now)
                .unwrap(),
            RouteEligibility::Excluded
        );
        assert!(matches!(
            large
                .route_facts("physical", Some(&large_source), "model-b", "config", now)
                .unwrap(),
            RouteEligibility::Eligible { .. }
        ));
    }

    #[test]
    fn v3_route_facts_require_newer_source_q_to_clear_account_markers() {
        let (_temp, generation, key, mut revision) = route_fixture(0);
        let store = generation.account("physical").unwrap();
        let digest = keyed(&key).unwrap();
        let now = 1_800_000_100;
        let mut source: SourceHead = typed(store.get("source", &digest).unwrap())
            .unwrap()
            .unwrap();
        let q = source.quota.as_ref().unwrap().q.clone();
        let quota_marker = MarkerHead {
            q: q.clone(),
            model: "other-model".into(),
            config_sha256: "other-config".into(),
            outcome: "quota_rejected".into(),
        };
        revision = store
            .commit(
                revision,
                vec![change("quota-marker", "account", &quota_marker).unwrap()],
            )
            .unwrap();
        assert_eq!(
            generation
                .route_facts("physical", Some(&key), "model-a", "config", now)
                .unwrap(),
            RouteEligibility::Excluded
        );
        source.quota.as_mut().unwrap().q.completed_unix_nanos += 1;
        revision = store
            .commit(revision, vec![change("source", &digest, &source).unwrap()])
            .unwrap();
        assert!(matches!(
            generation
                .route_facts("physical", Some(&key), "model-a", "config", now)
                .unwrap(),
            RouteEligibility::Eligible { .. }
        ));

        let auth_marker = MarkerHead {
            outcome: "auth_rejected".into(),
            ..quota_marker
        };
        revision = store
            .commit(
                revision,
                vec![change("auth-marker", "account", &auth_marker).unwrap()],
            )
            .unwrap();
        assert_eq!(
            generation
                .route_facts("physical", Some(&key), "model-a", "config", now)
                .unwrap(),
            RouteEligibility::Excluded
        );
        source.auth = Some(ObservationHead {
            q: PhysicalQ {
                completed_unix_nanos: q.completed_unix_nanos + 2,
                ..q
            },
            result: source.quota.as_ref().unwrap().result.clone(),
            outcome: "refreshed".into(),
            origin_model: Some("other-model".into()),
            origin_config_sha256: Some("other-config".into()),
            completed_unix_seconds: Some(now),
            windows: Vec::new(),
        });
        revision = store
            .commit(revision, vec![change("source", &digest, &source).unwrap()])
            .unwrap();
        assert!(matches!(
            generation
                .route_facts("physical", Some(&key), "model-a", "config", now)
                .unwrap(),
            RouteEligibility::Eligible { .. }
        ));
        let auth = source.auth.as_mut().unwrap();
        auth.q.completed_unix_nanos += 1;
        auth.outcome = "failed".into();
        store
            .commit(revision, vec![change("source", &digest, &source).unwrap()])
            .unwrap();
        assert_eq!(
            generation
                .route_facts("physical", Some(&key), "model-a", "config", now)
                .unwrap(),
            RouteEligibility::Excluded
        );
    }

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
