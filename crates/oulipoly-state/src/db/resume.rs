use super::{DbError, RusqliteOptionalExtension, StateDb, sqlite};
use chrono::{DateTime, Utc};
use oulipoly_config::ModelConfig;
use uuid::Uuid;

const OPENCODE_SESSION_PREFIX: &str = "ses_";
const OPENCODE_SESSION_MIN_SUFFIX_LEN: usize = 3;

pub type ModelStore = std::collections::HashMap<String, ModelConfig>;

#[derive(Debug, Clone)]
pub struct ResolvedResume {
    pub chain_id: String,
    pub model_name: Option<String>,
    pub model: Option<ModelConfig>,
    pub active_provider: String,
    pub active_session_id: String,
}

#[derive(Debug, Clone)]
pub enum ResumeError {
    InvalidUuid {
        input: String,
    },
    NoChainFound {
        input: String,
    },
    WrongIdKind {
        input: String,
        input_kind: WrongIdKindInput,
        provider_session_id: Option<String>,
        agent_runner_invocation_id: String,
        chain_id: Option<String>,
        provider_name: Option<String>,
    },
    Ambiguous {
        input: String,
        previews: Vec<ChainPreview>,
    },
    ProviderModelMismatch {
        model_name: String,
        active_provider: String,
        suggestions: Vec<String>,
    },
    ProviderNotConfigured {
        provider: String,
    },
    UnknownModel {
        model_name: String,
    },
    ActiveSegmentMissing {
        chain_id: String,
    },
    ProviderMissingResume {
        provider_name: String,
    },
    Db {
        message: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WrongIdKindInput {
    AgentRunnerInvocationId,
}

#[derive(Debug, Clone)]
pub struct ChainPreview {
    pub chain_id: String,
    pub last_used_at: DateTime<Utc>,
    pub active_provider: String,
    pub active_session_id: String,
    pub turn_count: usize,
    pub recent_turns: Vec<TurnPreview>,
}

#[derive(Debug, Clone)]
pub struct TurnPreview {
    pub role: String,
    pub timestamp: DateTime<Utc>,
    pub snippet: Option<String>,
}

struct WrongIdKindInvocationMatch {
    invocation_uuid: String,
    provider_name: Option<String>,
    provider_session_id: Option<String>,
    chain_id: Option<String>,
}

struct WrongIdKindInvocationRow {
    invocation_uuid: String,
    provider_name: Option<String>,
    provider_session_id: Option<String>,
}

struct RecentTurnRow {
    role: String,
    timestamp_raw: String,
}

struct ParsedTurnPreviewTimestamp {
    role: String,
    timestamp: DateTime<Utc>,
}

struct ResumeChainCandidate {
    chain_id: String,
    last_used_at: DateTime<Utc>,
    latest_segment_started_at: DateTime<Utc>,
}

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

    fn validate_resume_input_id(input: &str) -> Result<(), ResumeError> {
        if Uuid::parse_str(input).is_ok() || Self::is_opencode_provider_session_id(input) {
            return Ok(());
        }

        Err(ResumeError::InvalidUuid {
            input: input.to_string(),
        })
    }

    fn is_opencode_provider_session_id(input: &str) -> bool {
        let Some(suffix) = input.strip_prefix(OPENCODE_SESSION_PREFIX) else {
            return false;
        };

        suffix.len() >= OPENCODE_SESSION_MIN_SUFFIX_LEN
            && suffix.bytes().all(|byte| byte.is_ascii_alphanumeric())
    }

    fn reject_wrong_resume_id_kind(&self, input: &str) -> Result<(), ResumeError> {
        match self
            .wrong_id_kind_invocation_match(input)
            .map_err(|message| ResumeError::Db { message })?
        {
            Some(wrong_id) => Err(Self::wrong_id_kind_resume_error(input, wrong_id)),
            None => Ok(()),
        }
    }

    fn wrong_id_kind_resume_error(
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

    fn resolve_resume_chain_id(&self, input: &str) -> Result<String, ResumeError> {
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

    fn validate_resume_chain_candidates(
        input: &str,
        chain_ids: &[String],
    ) -> Result<(), ResumeError> {
        if chain_ids.is_empty() {
            Err(ResumeError::NoChainFound {
                input: input.to_string(),
            })
        } else {
            Ok(())
        }
    }

    fn ambiguous_resume_error(&self, input: &str) -> Result<ResumeError, ResumeError> {
        let previews = self
            .chain_previews(input)
            .map_err(|message| ResumeError::Db { message })?;
        Ok(ResumeError::Ambiguous {
            input: input.to_string(),
            previews,
        })
    }

    fn require_active_segment(&self, chain_id: &str) -> Result<(String, String), ResumeError> {
        self.active_segment_for_chain(chain_id)
            .map_err(|message| ResumeError::Db { message })?
            .ok_or_else(|| ResumeError::ActiveSegmentMissing {
                chain_id: chain_id.to_string(),
            })
    }

    fn resolve_resume_model_name(
        &self,
        chain_id: &str,
        model_override: Option<&str>,
    ) -> Result<Option<String>, ResumeError> {
        match model_override {
            Some(model_name) => Ok(Some(model_name.to_string())),
            None => self.infer_resume_model_name(chain_id),
        }
    }

    fn infer_resume_model_name(&self, chain_id: &str) -> Result<Option<String>, ResumeError> {
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

    fn first_known_resume_model_name(
        latest_invocation: Option<String>,
        chain_model: Option<String>,
    ) -> Option<String> {
        latest_invocation
            .filter(|name| Self::resume_model_name_is_known(name))
            .or(chain_model.filter(|name| Self::resume_model_name_is_known(name)))
    }

    fn resume_model_name_is_known(model_name: &str) -> bool {
        model_name != "<unknown>"
    }

    fn resolve_resume_model_config(
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

    fn require_resume_model(
        models: &ModelStore,
        model_name: &str,
    ) -> Result<ModelConfig, ResumeError> {
        models
            .get(model_name)
            .cloned()
            .ok_or_else(|| ResumeError::UnknownModel {
                model_name: model_name.to_string(),
            })
    }

    fn validate_resume_provider_for_model(
        models: &ModelStore,
        model_name: &str,
        model: &ModelConfig,
        active_provider: &str,
    ) -> Result<(), ResumeError> {
        if Self::model_has_provider(model, active_provider) {
            Ok(())
        } else {
            Err(ResumeError::ProviderModelMismatch {
                model_name: model_name.to_string(),
                active_provider: active_provider.to_string(),
                suggestions: Self::model_names_for_provider(models, active_provider),
            })
        }
    }

    fn model_has_provider(model: &ModelConfig, active_provider: &str) -> bool {
        model
            .providers
            .iter()
            .any(|provider| provider.name == active_provider)
    }

    fn model_names_for_provider(models: &ModelStore, active_provider: &str) -> Vec<String> {
        let mut suggestions = models
            .iter()
            .filter(|(_, model)| Self::model_has_provider(model, active_provider))
            .map(|(name, _)| name.clone())
            .collect::<Vec<_>>();
        suggestions.sort();
        suggestions
    }

    fn assemble_resolved_resume(
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

    pub fn resume_previews(&self, input: &str) -> Result<Vec<ChainPreview>, DbError> {
        Uuid::try_parse(input).map_err(|e| format!("Invalid UUID {input}: {e}"))?;
        self.chain_previews(input)
    }

    pub fn chain_id_for_segment(
        &self,
        provider_name: &str,
        session_id: &str,
    ) -> Result<Option<String>, DbError> {
        self.conn
            .query_row(
                "SELECT chain_id
                 FROM session_chain_segments
                 WHERE provider_name = ?1 AND session_id = ?2
                 ORDER BY ended_at IS NULL DESC, started_at DESC, id DESC
                 LIMIT 1",
                sqlite::params![provider_name, session_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| format!("Failed to look up session chain id: {e}"))
    }

    fn candidate_chain_ids(&self, input: &str) -> Result<Vec<String>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT DISTINCT chain_id
                 FROM session_chain_segments
                 WHERE session_id = ?1 OR chain_id = ?1
                 ORDER BY chain_id",
            )
            .map_err(|e| format!("Failed to prepare resume chain lookup: {e}"))?;
        let rows = stmt
            .query_map(sqlite::params![input], |row| row.get::<_, String>(0))
            .map_err(|e| format!("Failed to query resume chain lookup: {e}"))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Failed to read resume chain lookup: {e}"))
    }

    fn wrong_id_kind_invocation_match(
        &self,
        input: &str,
    ) -> Result<Option<WrongIdKindInvocationMatch>, String> {
        let sql = Self::wrong_id_invocation_match_sql(&self.conn)?;
        let row = Self::load_wrong_id_invocation_match_row(&self.conn, &sql, input)?;

        let Some(row) = row else {
            return Ok(None);
        };
        let chain_id = self.chain_id_for_wrong_id_match(
            row.provider_name.as_deref(),
            row.provider_session_id.as_deref(),
        )?;
        Ok(Some(WrongIdKindInvocationMatch {
            invocation_uuid: row.invocation_uuid,
            provider_name: row.provider_name,
            provider_session_id: row.provider_session_id,
            chain_id,
        }))
    }

    fn wrong_id_invocation_match_sql(conn: &sqlite::Connection) -> Result<String, String> {
        let provider_session_select = Self::wrong_id_provider_session_select(conn)?;
        Ok(format!(
            "SELECT invocation_uuid, provider_name, {provider_session_select}
             FROM invocations
             WHERE invocation_uuid = ?1"
        ))
    }

    fn wrong_id_provider_session_select(conn: &sqlite::Connection) -> Result<&'static str, String> {
        if Self::invocations_have_dual_id_columns(conn)? {
            Ok("provider_session_id")
        } else {
            Ok("NULL AS provider_session_id")
        }
    }

    fn load_wrong_id_invocation_match_row(
        conn: &sqlite::Connection,
        sql: &str,
        input: &str,
    ) -> Result<Option<WrongIdKindInvocationRow>, String> {
        conn.query_row(sql, sqlite::params![input], |row| {
            Ok(WrongIdKindInvocationRow {
                invocation_uuid: row.get(0)?,
                provider_name: row.get(1)?,
                provider_session_id: row.get(2)?,
            })
        })
        .optional()
        .map_err(|e| format!("Failed to query invocation id-kind match: {e}"))
    }

    fn chain_id_for_wrong_id_match(
        &self,
        provider_name: Option<&str>,
        provider_session_id: Option<&str>,
    ) -> Result<Option<String>, String> {
        match (provider_name, provider_session_id) {
            (Some(provider_name), Some(provider_session_id)) => self
                .chain_id_for_segment(provider_name, provider_session_id)
                .map_err(|e| format!("Failed to resolve chain for wrong-id-kind match: {e}")),
            _ => Ok(None),
        }
    }

    fn choose_resume_chain(
        &self,
        _input: &str,
        mut chain_ids: Vec<String>,
    ) -> Result<Option<String>, String> {
        if chain_ids.len() == 1 {
            return Ok(chain_ids.pop());
        }
        let mut rows = Vec::new();
        for chain_id in chain_ids {
            rows.push(self.load_resume_chain_candidate(chain_id)?);
        }
        Self::sort_resume_chain_candidates(&mut rows);
        Ok(rows.into_iter().next().map(|row| row.chain_id))
    }

    fn load_resume_chain_candidate(
        &self,
        chain_id: String,
    ) -> Result<ResumeChainCandidate, String> {
        let last_used_at = self.read_chain_last_used_at(&chain_id)?;
        let latest_segment_started_at = self.read_latest_segment_started_at(&chain_id)?;
        Ok(ResumeChainCandidate {
            chain_id,
            last_used_at,
            latest_segment_started_at,
        })
    }

    fn read_chain_last_used_at(&self, chain_id: &str) -> Result<DateTime<Utc>, String> {
        let raw: String = self
            .conn
            .query_row(
                "SELECT last_used_at FROM session_chains WHERE chain_id = ?1",
                sqlite::params![chain_id],
                |row| row.get(0),
            )
            .map_err(|e| format!("Failed to read chain last_used_at: {e}"))?;
        Self::strict_rfc3339_message(&raw, "chain last_used_at")
    }

    fn read_latest_segment_started_at(&self, chain_id: &str) -> Result<DateTime<Utc>, String> {
        let raw_started: String = self
            .conn
            .query_row(
                "SELECT started_at
                 FROM session_chain_segments
                 WHERE chain_id = ?1
                 ORDER BY started_at DESC, id DESC
                 LIMIT 1",
                sqlite::params![chain_id],
                |row| row.get(0),
            )
            .map_err(|e| format!("Failed to read chain latest segment started_at: {e}"))?;
        Self::strict_rfc3339_message(&raw_started, "chain segment started_at")
    }

    fn sort_resume_chain_candidates(rows: &mut [ResumeChainCandidate]) {
        rows.sort_by(|a, b| {
            b.last_used_at
                .cmp(&a.last_used_at)
                .then_with(|| {
                    b.latest_segment_started_at
                        .cmp(&a.latest_segment_started_at)
                })
                .then_with(|| a.chain_id.cmp(&b.chain_id))
        });
    }

    pub fn active_segment_id_for_chain_provider_session(
        &self,
        chain_id: &str,
        provider_name: &str,
        session_id: &str,
    ) -> Result<Option<i64>, String> {
        self.conn
            .query_row(
                "SELECT id
                 FROM session_chain_segments
                 WHERE chain_id = ?1
                   AND provider_name = ?2
                   AND session_id = ?3
                   AND ended_at IS NULL
                 ORDER BY started_at DESC, id DESC
                 LIMIT 1",
                sqlite::params![chain_id, provider_name, session_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| format!("Failed to read active chain segment id: {e}"))
    }

    fn active_segment_for_chain(&self, chain_id: &str) -> Result<Option<(String, String)>, String> {
        self.conn
            .query_row(
                "SELECT provider_name, session_id
                 FROM session_chain_segments
                 WHERE chain_id = ?1 AND ended_at IS NULL
                 ORDER BY started_at DESC, id DESC
                 LIMIT 1",
                sqlite::params![chain_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(|e| format!("Failed to read active chain segment: {e}"))
    }

    fn chain_model_name(&self, chain_id: &str) -> Result<Option<String>, String> {
        self.conn
            .query_row(
                "SELECT model_name FROM session_chains WHERE chain_id = ?1",
                sqlite::params![chain_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| format!("Failed to read session chain model: {e}"))
    }

    fn latest_invocation_model_for_chain(&self, chain_id: &str) -> Result<Option<String>, String> {
        let provider_session_expr = Self::provider_session_expr(&self.conn, Some("i."))?;
        let sql = Self::latest_invocation_model_sql(&provider_session_expr);
        self.conn
            .query_row(&sql, sqlite::params![chain_id], |row| row.get(0))
            .optional()
            .map_err(|e| format!("Failed to infer session chain model from invocations: {e}"))
    }

    fn latest_invocation_model_sql(provider_session_expr: &str) -> String {
        format!(
            "SELECT i.model_name
             FROM invocations i
             WHERE {provider_session_expr} IN (
                SELECT session_id FROM session_chain_segments WHERE chain_id = ?1
             )
             ORDER BY COALESCE(i.finished_at, i.created_at) DESC, i.id DESC
             LIMIT 1"
        )
    }

    fn chain_previews(&self, input: &str) -> Result<Vec<ChainPreview>, String> {
        let chain_ids = self.candidate_chain_ids(input)?;
        let mut out = Vec::new();
        for chain_id in chain_ids {
            out.push(self.build_chain_preview(chain_id)?);
        }
        Self::sort_chain_previews(&mut out);
        Ok(out)
    }

    fn build_chain_preview(&self, chain_id: String) -> Result<ChainPreview, String> {
        let last_used_at = self.read_chain_preview_last_used_at(&chain_id)?;
        let (active_provider, active_session_id) = self
            .active_segment_for_chain(&chain_id)?
            .unwrap_or_else(|| ("<none>".to_string(), "<none>".to_string()));
        let turn_count = self.preview_turn_count(&active_provider, &active_session_id);
        let recent_turns = self.recent_turn_previews(&active_provider, &active_session_id)?;
        Ok(ChainPreview {
            chain_id,
            last_used_at,
            active_provider,
            active_session_id,
            turn_count,
            recent_turns,
        })
    }

    fn read_chain_preview_last_used_at(&self, chain_id: &str) -> Result<DateTime<Utc>, String> {
        let raw_last: String = self
            .conn
            .query_row(
                "SELECT last_used_at FROM session_chains WHERE chain_id = ?1",
                sqlite::params![chain_id],
                |row| row.get(0),
            )
            .map_err(|e| format!("Failed to read chain preview: {e}"))?;
        Self::strict_rfc3339_message(&raw_last, "chain preview timestamp")
    }

    fn preview_turn_count(&self, active_provider: &str, active_session_id: &str) -> usize {
        let turn_count: i64 = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM session_turns WHERE provider_name = ?1 AND session_id = ?2",
                sqlite::params![active_provider, active_session_id],
                |row| row.get(0),
            )
            .unwrap_or(0);
        turn_count.max(0) as usize
    }

    fn recent_turn_previews(
        &self,
        active_provider: &str,
        active_session_id: &str,
    ) -> Result<Vec<TurnPreview>, String> {
        let rows = self.query_recent_turn_rows(active_provider, active_session_id)?;
        let parsed = Self::parse_turn_preview_timestamps(rows)?;
        let mut recent_turns = Self::map_recent_turn_previews(parsed);
        recent_turns.reverse();
        Ok(recent_turns)
    }

    fn query_recent_turn_rows(
        &self,
        active_provider: &str,
        active_session_id: &str,
    ) -> Result<Vec<RecentTurnRow>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT role, timestamp
                 FROM session_turns
                 WHERE provider_name = ?1 AND session_id = ?2
                 ORDER BY timestamp DESC, id DESC
                 LIMIT 3",
            )
            .map_err(|e| format!("Failed to prepare recent turns preview: {e}"))?;
        let rows = stmt
            .query_map(
                sqlite::params![active_provider, active_session_id],
                Self::recent_turn_row_mapper,
            )
            .map_err(|e| format!("Failed to query recent turns preview: {e}"))?;

        let mut recent_turns = Vec::new();
        for row in rows {
            recent_turns.push(row.map_err(|e| format!("Failed to read recent turn: {e}"))?);
        }
        Ok(recent_turns)
    }

    fn recent_turn_row_mapper(row: &sqlite::Row<'_>) -> sqlite::Result<RecentTurnRow> {
        Ok(RecentTurnRow {
            role: row.get(0)?,
            timestamp_raw: row.get(1)?,
        })
    }

    fn parse_turn_preview_timestamps(
        rows: Vec<RecentTurnRow>,
    ) -> Result<Vec<ParsedTurnPreviewTimestamp>, String> {
        rows.into_iter()
            .map(|row| {
                Ok(ParsedTurnPreviewTimestamp {
                    role: row.role,
                    timestamp: Self::strict_rfc3339_message(
                        &row.timestamp_raw,
                        "recent turn timestamp",
                    )?,
                })
            })
            .collect()
    }

    fn map_recent_turn_previews(rows: Vec<ParsedTurnPreviewTimestamp>) -> Vec<TurnPreview> {
        rows.into_iter()
            .map(|row| TurnPreview {
                role: row.role,
                timestamp: row.timestamp,
                snippet: None,
            })
            .collect()
    }

    fn sort_chain_previews(out: &mut [ChainPreview]) {
        out.sort_by_key(|preview| std::cmp::Reverse(preview.last_used_at));
    }
}
