//! ## Declared roles
//!
//! `mapper`, `accessor`, `validator`, `formatter`.

use super::super::*;

pub(in crate::balancer::tests) fn migratable_model(provider_names: &[(&str, &str)]) -> ModelConfig {
    let providers = provider_names
        .iter()
        .map(|(name, storage_kind)| migratable_provider(name, storage_kind))
        .collect();
    ModelConfig {
        name: "migration-fixture".to_string(),
        prompt_mode: PromptMode::Arg,
        providers,
        inputs: Vec::new(),
        provider: None,
    }
}

pub(in crate::balancer::tests) fn migratable_provider(
    name: &str,
    storage_kind: &str,
) -> oulipoly_config::ProviderConfig {
    oulipoly_config::ProviderConfig {
        name: name.to_string(),
        command: name.to_string(),
        args: Vec::new(),
        interactive_args: Some(vec!["launch".to_string()]),
        resume: Some(resume_strategy_for_test()),
        session_capture: None,
        resume_acceptance: None,
        session_storage: session_storage_for_test(name, storage_kind),
        system_prompt_override: None,
        tool_restrictions: None,
        invocation_mode: Default::default(),
    }
}

pub(in crate::balancer::tests) fn resume_strategy_for_test() -> oulipoly_config::ResumeStrategy {
    oulipoly_config::ResumeStrategy {
        kind: oulipoly_config::ResumeKind::Flag,
        flag: Some("--resume".to_string()),
        subcommand: None,
    }
}

pub(in crate::balancer::tests) fn session_storage_for_test(
    name: &str,
    storage_kind: &str,
) -> Option<oulipoly_config::SessionStorage> {
    match storage_kind {
        "project_storage" => Some(project_storage_storage_for_test(name)),
        "script_storage" => Some(script_storage_storage_for_test(name)),
        "custom_script" => Some(custom_script_storage_for_test()),
        "omega" => Some(omega_storage_for_test(name)),
        "none" => None,
        other => panic!("unknown storage kind fixture {other}"),
    }
}

pub(in crate::balancer::tests) fn project_storage_storage_for_test(
    name: &str,
) -> oulipoly_config::SessionStorage {
    oulipoly_config::SessionStorage::ClaudeCode {
        projects_dir: PathBuf::from(format!("/tmp/{name}/projects")),
    }
}

pub(in crate::balancer::tests) fn script_storage_storage_for_test(
    name: &str,
) -> oulipoly_config::SessionStorage {
    oulipoly_config::SessionStorage::Script {
        cwd_script: format!("{} /tmp/{name}/projects", concat!("clau", "de-code-cwd")),
        transcript_script: Some(format!("project-locate-transcript /tmp/{name}/projects")),
        storage_type: Some(oulipoly_config::ScriptSessionStorageType::ClaudeCode),
    }
}

pub(in crate::balancer::tests) fn custom_script_storage_for_test() -> oulipoly_config::SessionStorage
{
    oulipoly_config::SessionStorage::Script {
        cwd_script: "custom-cwd /tmp/custom/projects".to_string(),
        transcript_script: Some("custom-locate-transcript /tmp/custom/projects".to_string()),
        storage_type: Some(oulipoly_config::ScriptSessionStorageType::ClaudeCode),
    }
}

pub(in crate::balancer::tests) fn omega_storage_for_test(
    name: &str,
) -> oulipoly_config::SessionStorage {
    oulipoly_config::SessionStorage::Codex {
        sessions_dir: PathBuf::from(format!("/tmp/{name}/sessions")),
    }
}

pub(in crate::balancer::tests) fn resolved_for(
    model: &ModelConfig,
    provider_index: usize,
) -> oulipoly_state::ResolvedResume {
    let provider = &model.providers[provider_index];
    oulipoly_state::ResolvedResume {
        chain_id: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa".to_string(),
        model_name: Some(model.name.clone()),
        model: Some(model.clone()),
        active_provider: provider.name.clone(),
        active_session_id: "5169694d-de0f-40d1-890c-6e28e55bab27".to_string(),
    }
}

pub(in crate::balancer::tests) fn drop_quota_table(db: &StateDb) {
    db.drop_provider_quotas_for_test();
}
