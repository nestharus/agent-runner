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
//!       - StateDb resume input acceptance for UUIDs and OpenCode provider session IDs
//!       - OpenCode `ses_` provider-session prefix strip and suffix validation
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
use uuid::Uuid;

impl StateDb {
    pub fn resolve_resume(
        &self,
        models: &ModelStore,
        input: &str,
        model_override: Option<&str>,
    ) -> Result<ResolvedResume, ResumeError> {
        Self::validate_resume_input_id(input)?;
        self.reject_wrong_resume_id_kind(input)?;
        let chain_id = self.resolve_resume_chain_id(input)?;
        let (active_provider, active_session_id) = self.require_active_segment(&chain_id)?;
        let model_name = self.resolve_resume_model_name(&chain_id, model_override)?;
        let model =
            Self::resolve_resume_model_config(models, model_name.as_ref(), &active_provider)?;
        Ok(Self::assemble_resolved_resume(
            chain_id,
            model_name,
            model,
            active_provider,
            active_session_id,
        ))
    }

    pub(super) fn validate_resume_input_id(input: &str) -> Result<(), ResumeError> {
        if Self::resume_input_id_is_valid(input) {
            return Ok(());
        }

        Err(Self::invalid_resume_uuid_error(input))
    }

    fn resume_input_id_is_valid(input: &str) -> bool {
        Self::resume_input_id_is_uuid(input) || Self::is_opencode_provider_session_id(input)
    }

    fn resume_input_id_is_uuid(input: &str) -> bool {
        Uuid::parse_str(input).is_ok()
    }

    fn invalid_resume_uuid_error(input: &str) -> ResumeError {
        ResumeError::InvalidUuid {
            input: input.to_string(),
        }
    }

    pub(super) fn is_opencode_provider_session_id(input: &str) -> bool {
        let Some(suffix) = input.strip_prefix(OPENCODE_SESSION_PREFIX) else {
            return false;
        };

        suffix.len() >= OPENCODE_SESSION_MIN_SUFFIX_LEN
            && suffix.bytes().all(|byte| byte.is_ascii_alphanumeric())
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

    pub(super) fn resolve_resume_chain_id(&self, input: &str) -> Result<String, ResumeError> {
        let chain_ids = self
            .candidate_chain_ids(input)
            .map_err(|message| ResumeError::Db { message })?;
        Self::validate_resume_chain_candidates(input, &chain_ids)?;
        match self
            .choose_resume_chain(input, chain_ids)
            .map_err(|message| ResumeError::Db { message })?
        {
            Some(chain_id) => Ok(chain_id),
            None => Err(self.ambiguous_resume_error(input)?),
        }
    }

    pub(super) fn validate_resume_chain_candidates(
        input: &str,
        chain_ids: &[String],
    ) -> Result<(), ResumeError> {
        if Self::resume_chain_candidates_exist(chain_ids) {
            Ok(())
        } else {
            Err(Self::no_resume_chain_found_error(input))
        }
    }

    fn resume_chain_candidates_exist(chain_ids: &[String]) -> bool {
        !chain_ids.is_empty()
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
