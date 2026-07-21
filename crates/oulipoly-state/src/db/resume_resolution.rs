//! ## Declared roles
//!
//! - accessor
//! - filter
//! - mapper
//! - orchestration
//! - predicate
//! - validator
//! - parser
//!
//! Role set: { accessor, filter, mapper, orchestration, predicate, validator, parser }
//!
//! Resume request resolution and provider/model validation.
//!
//! ## Intrinsic-surface declarations
//!
//! ```yaml
//! intrinsic_surface_declarations:
//!   - component: crates/oulipoly-state/src/db/resume_resolution.rs
//!     role: intrinsic-surface
//!     Domain: provider-session-id-grammar
//!     Owns:
//!       - StateDb provider-neutral bounded opaque resume input acceptance
//!       - exact-chain-first classification before provider-native candidate handling
//!       - ResumeInputMatch and ResumeNativeCandidate lineage-cardinality resolution
//!       - provider-session-id-to-chain resolution query path
//!   - component: crates/oulipoly-state/src/db/resume_resolution.rs
//!     role: intrinsic-surface
//!     Domain: resume-model-provider-resolution
//!     Owns:
//!       - ModelStore and ModelConfig lookup for resume continuation
//!       - active-provider compatibility checks against model provider entries
//!       - provider/model mismatch suggestion construction and sorting
//!       - ResolvedResume assembly across chain, provider, session, and model fields
//!       - external contract symbols referenced by this concern via its `use`
//!         declarations, intrinsic and subordinate to this persistence domain: ModelConfig, Uuid
//! ```

use super::*;
use oulipoly_config::ModelConfig;

impl StateDb {
    pub fn resolve_resume(
        &self,
        models: &ModelStore,
        input: &str,
        model_override: Option<&str>,
    ) -> Result<ResolvedResume, ResumeError> {
        match self.classify_resume_input(input)? {
            ResumeInputMatch::ExactChain { chain_id } => {
                self.resolve_resume_chain(models, &chain_id, model_override)
            }
            ResumeInputMatch::NativeSession { candidates } => {
                self.resolve_single_native_lineage(models, input, &candidates, model_override)
            }
        }
    }

    fn resolve_single_native_lineage(
        &self,
        models: &ModelStore,
        input: &str,
        candidates: &[ResumeNativeCandidate],
        model_override: Option<&str>,
    ) -> Result<ResolvedResume, ResumeError> {
        let chain_ids = Self::distinct_native_candidate_chain_ids(candidates);
        let Some(chain_id) = Self::single_native_chain_id(&chain_ids) else {
            return Err(self.ambiguous_resume_error(input)?);
        };
        self.resolve_resume_chain(models, chain_id, model_override)
    }

    fn single_native_chain_id(chain_ids: &[String]) -> Option<&str> {
        match chain_ids {
            [chain_id] => Some(chain_id),
            _ => None,
        }
    }

    pub fn classify_resume_input(&self, input: &str) -> Result<ResumeInputMatch, ResumeError> {
        Self::validate_resume_input_id(input)?;
        self.reject_wrong_resume_id_kind(input)?;
        let exact_chain_id = self
            .exact_resume_chain_id(input)
            .map_err(|message| ResumeError::Db { message })?;
        if Self::has_exact_resume_chain_id(&exact_chain_id) {
            return Self::classify_resume_facts(input, exact_chain_id, Vec::new());
        }
        let native_candidates = self
            .native_resume_candidates(input)
            .map_err(|message| ResumeError::Db { message })?;
        Self::classify_resume_facts(input, None, native_candidates)
    }

    fn has_exact_resume_chain_id(exact_chain_id: &Option<String>) -> bool {
        exact_chain_id.is_some()
    }

    fn classify_resume_facts(
        input: &str,
        exact_chain_id: Option<String>,
        native_candidates: Vec<ResumeNativeCandidate>,
    ) -> Result<ResumeInputMatch, ResumeError> {
        if let Some(chain_id) = exact_chain_id {
            return Ok(ResumeInputMatch::ExactChain { chain_id });
        }
        if native_candidates.is_empty() {
            return Err(Self::no_resume_chain_found_error(input));
        }
        Ok(ResumeInputMatch::NativeSession {
            candidates: native_candidates,
        })
    }

    pub fn resolve_resume_chain(
        &self,
        models: &ModelStore,
        chain_id: &str,
        model_override: Option<&str>,
    ) -> Result<ResolvedResume, ResumeError> {
        let (active_provider, active_session_id) = self.require_active_segment(chain_id)?;
        let model_name = self.resolve_resume_model_name(chain_id, model_override)?;
        let model =
            Self::resolve_resume_model_config(models, model_name.as_ref(), &active_provider)?;
        Ok(Self::assemble_resolved_resume(
            chain_id.to_string(),
            model_name,
            model,
            active_provider,
            active_session_id,
        ))
    }

    fn distinct_native_candidate_chain_ids(candidates: &[ResumeNativeCandidate]) -> Vec<String> {
        Self::sort_and_deduplicate_chain_ids(Self::native_candidate_chain_ids(candidates))
    }

    fn native_candidate_chain_ids(candidates: &[ResumeNativeCandidate]) -> Vec<String> {
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

    pub fn validate_resume_input_id(input: &str) -> Result<(), ResumeError> {
        match Self::resume_input_validation_error(input) {
            Some(reason) => Err(Self::invalid_resume_input_error(input, reason)),
            None => Ok(()),
        }
    }

    fn resume_input_validation_error(input: &str) -> Option<String> {
        if input.trim().is_empty() {
            Some("session id is required".to_string())
        } else if input.len() > RESUME_INPUT_MAX_LEN {
            Some(format!(
                "session id exceeds maximum length of {RESUME_INPUT_MAX_LEN} bytes"
            ))
        } else if input.chars().any(char::is_control) {
            Some("session id contains control characters".to_string())
        } else {
            None
        }
    }

    fn invalid_resume_input_error(input: &str, reason: String) -> ResumeError {
        ResumeError::InvalidResumeInput {
            input: input.to_string(),
            reason,
        }
    }

    pub(super) fn reject_wrong_resume_id_kind(&self, input: &str) -> Result<(), ResumeError> {
        match self
            .wrong_id_kind_invocation_match(input)
            .map_err(|message| ResumeError::Db { message })?
        {
            Some(wrong_id) => Err(Self::wrong_id_kind_resume_error(input, wrong_id)),
            None => Ok(()),
        }
    }

    pub(super) fn wrong_id_kind_resume_error(
        input: &str,
        wrong_id: WrongIdKindInvocationMatch,
    ) -> ResumeError {
        ResumeError::WrongIdKind {
            input: input.to_string(),
            input_kind: WrongIdKindInput::AgentRunnerInvocationId,
            provider_session_id: wrong_id.provider_session_id,
            agent_runner_invocation_id: wrong_id.invocation_uuid,
            chain_id: wrong_id.chain_id,
            provider_name: wrong_id.provider_name,
        }
    }

    fn no_resume_chain_found_error(input: &str) -> ResumeError {
        ResumeError::NoChainFound {
            input: input.to_string(),
        }
    }

    pub(super) fn ambiguous_resume_error(&self, input: &str) -> Result<ResumeError, ResumeError> {
        let previews = self
            .chain_previews(input)
            .map_err(|message| ResumeError::Db { message })?;
        Ok(Self::map_ambiguous_resume_error(input, previews))
    }

    fn map_ambiguous_resume_error(input: &str, previews: Vec<ChainPreview>) -> ResumeError {
        ResumeError::Ambiguous {
            input: input.to_string(),
            previews,
        }
    }

    pub(super) fn require_active_segment(
        &self,
        chain_id: &str,
    ) -> Result<(String, String), ResumeError> {
        self.active_segment_for_chain(chain_id)
            .map_err(|message| ResumeError::Db { message })?
            .ok_or_else(|| Self::active_segment_missing_error(chain_id))
    }

    fn active_segment_missing_error(chain_id: &str) -> ResumeError {
        ResumeError::ActiveSegmentMissing {
            chain_id: chain_id.to_string(),
        }
    }

    pub(super) fn resolve_resume_model_name(
        &self,
        chain_id: &str,
        model_override: Option<&str>,
    ) -> Result<Option<String>, ResumeError> {
        match model_override {
            Some(model_name) => Ok(Some(model_name.to_string())),
            None => self.infer_resume_model_name(chain_id),
        }
    }

    pub(super) fn infer_resume_model_name(
        &self,
        chain_id: &str,
    ) -> Result<Option<String>, ResumeError> {
        let latest_invocation = self
            .latest_invocation_model_for_chain(chain_id)
            .map_err(|message| ResumeError::Db { message })?;
        let chain_model = self
            .chain_model_name(chain_id)
            .map_err(|message| ResumeError::Db { message })?;
        Ok(Self::first_known_resume_model_name(
            latest_invocation,
            chain_model,
        ))
    }

    pub(super) fn first_known_resume_model_name(
        latest_invocation: Option<String>,
        chain_model: Option<String>,
    ) -> Option<String> {
        latest_invocation
            .filter(|name| Self::resume_model_name_is_known(name))
            .or(chain_model.filter(|name| Self::resume_model_name_is_known(name)))
    }

    pub(super) fn resume_model_name_is_known(model_name: &str) -> bool {
        model_name != "<unknown>"
    }

    pub(super) fn resolve_resume_model_config(
        models: &ModelStore,
        model_name: Option<&String>,
        active_provider: &str,
    ) -> Result<Option<ModelConfig>, ResumeError> {
        match model_name {
            Some(model_name) => {
                let model = Self::require_resume_model(models, model_name)?;
                Self::validate_resume_provider_for_model(
                    models,
                    model_name,
                    &model,
                    active_provider,
                )?;
                Ok(Some(model))
            }
            None => Ok(None),
        }
    }

    pub(super) fn require_resume_model(
        models: &ModelStore,
        model_name: &str,
    ) -> Result<ModelConfig, ResumeError> {
        models
            .get(model_name)
            .cloned()
            .ok_or_else(|| Self::unknown_resume_model_error(model_name))
    }

    fn unknown_resume_model_error(model_name: &str) -> ResumeError {
        ResumeError::UnknownModel {
            model_name: model_name.to_string(),
        }
    }

    pub(super) fn validate_resume_provider_for_model(
        models: &ModelStore,
        model_name: &str,
        model: &ModelConfig,
        active_provider: &str,
    ) -> Result<(), ResumeError> {
        if Self::model_has_provider(model, active_provider) {
            Ok(())
        } else {
            Err(Self::provider_model_mismatch_error(
                models,
                model_name,
                active_provider,
            ))
        }
    }

    fn provider_model_mismatch_error(
        models: &ModelStore,
        model_name: &str,
        active_provider: &str,
    ) -> ResumeError {
        ResumeError::ProviderModelMismatch {
            model_name: model_name.to_string(),
            active_provider: active_provider.to_string(),
            suggestions: Self::model_names_for_provider(models, active_provider),
        }
    }

    pub(super) fn model_has_provider(model: &ModelConfig, active_provider: &str) -> bool {
        model
            .providers
            .iter()
            .any(|provider| provider.name == active_provider)
    }

    pub(super) fn model_names_for_provider(
        models: &ModelStore,
        active_provider: &str,
    ) -> Vec<String> {
        let compatible_models = Self::models_for_provider(models, active_provider);
        let mut suggestions = Self::model_names_from_entries(compatible_models);
        suggestions.sort();
        suggestions
    }

    fn models_for_provider<'a>(
        models: &'a ModelStore,
        active_provider: &str,
    ) -> Vec<(&'a String, &'a ModelConfig)> {
        models
            .iter()
            .filter(|(_, model)| Self::model_has_provider(model, active_provider))
            .collect()
    }

    fn model_names_from_entries(entries: Vec<(&String, &ModelConfig)>) -> Vec<String> {
        entries.into_iter().map(|(name, _)| name.clone()).collect()
    }

    pub(super) fn assemble_resolved_resume(
        chain_id: String,
        model_name: Option<String>,
        model: Option<ModelConfig>,
        active_provider: String,
        active_session_id: String,
    ) -> ResolvedResume {
        ResolvedResume {
            chain_id,
            model_name,
            model,
            active_provider,
            active_session_id,
        }
    }
}
