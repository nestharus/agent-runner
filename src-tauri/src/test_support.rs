use crate::balancer;
use crate::config::{
    FilesystemModelConfigRepository, FilesystemProviderConfigSource, ModelConfigRepository,
    ProviderConfigSource,
};
use crate::discovery;
use crate::executor;
use crate::process::{
    CommandSpec, InteractiveCommandSpec, OutputSpec, ProcessOutput, ProcessRunner,
};
use crate::quota;
use crate::runtime::RuntimePaths;
use crate::state::{
    DefaultStateDbOpener, DiscoveredModel, DiscoveryRepository, QuotaRepository, QuotaWindowInput,
    StateDbOpener,
};
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::fs;
use std::sync::{Arc, Mutex};

#[derive(Default)]
pub struct HarnessProcessRunner {
    calls: Mutex<Vec<CommandSpec>>,
    interactive_calls: Mutex<Vec<InteractiveCommandSpec>>,
    responses: Mutex<VecDeque<Result<ProcessOutput, String>>>,
    interactive_responses: Mutex<VecDeque<Result<i32, String>>>,
}

impl HarnessProcessRunner {
    pub fn push_response(&self, response: Result<ProcessOutput, String>) {
        self.responses.lock().unwrap().push_back(response);
    }

    pub fn push_stdout(&self, stdout: impl AsRef<[u8]>) {
        self.push_response(Ok(ProcessOutput {
            stdout: stdout.as_ref().to_vec(),
            stderr: Vec::new(),
            exit_code: 0,
            timed_out: false,
        }));
    }

    pub fn push_stderr_exit(&self, stderr: &str, exit_code: i32) {
        self.push_response(Ok(ProcessOutput {
            stdout: Vec::new(),
            stderr: stderr.as_bytes().to_vec(),
            exit_code,
            timed_out: false,
        }));
    }

    pub fn only_call(&self) -> CommandSpec {
        let calls = self.calls.lock().unwrap().clone();
        assert_eq!(calls.len(), 1, "expected exactly one call: {calls:?}");
        calls[0].clone()
    }
}

impl ProcessRunner for HarnessProcessRunner {
    fn run(&self, spec: CommandSpec) -> Result<ProcessOutput, String> {
        self.calls.lock().unwrap().push(spec.clone());
        let response = self
            .responses
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| {
                Ok(ProcessOutput {
                    stdout: Vec::new(),
                    stderr: Vec::new(),
                    exit_code: 0,
                    timed_out: false,
                })
            });
        response.map(|mut output| {
            if spec.stdout == OutputSpec::Null {
                output.stdout.clear();
            }
            if spec.stderr == OutputSpec::Null {
                output.stderr.clear();
            }
            output
        })
    }

    fn run_interactive(&self, spec: InteractiveCommandSpec) -> Result<i32, String> {
        self.interactive_calls.lock().unwrap().push(spec);
        self.interactive_responses
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(Ok(0))
    }
}

pub struct AppStateCommandHarness {
    paths: Arc<dyn RuntimePaths>,
    runner: Arc<HarnessProcessRunner>,
}

impl AppStateCommandHarness {
    pub fn new<P>(paths: P) -> Self
    where
        P: RuntimePaths + 'static,
    {
        Self {
            paths: Arc::new(paths),
            runner: Arc::new(HarnessProcessRunner::default()),
        }
    }

    pub fn runner(&self) -> Arc<HarnessProcessRunner> {
        self.runner.clone()
    }

    pub fn write_model_toml(&self, name: &str, body: &str) {
        let dir = self.paths.models_dir();
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(format!("{name}.toml")), body).unwrap();
    }

    pub fn write_providers_toml(&self, body: &str) {
        let path = self.paths.providers_path();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, body).unwrap();
    }

    pub fn invoke_json(&self, command: &str, payload: Value) -> Result<Value, String> {
        match command {
            "list_models" => self.list_models(),
            "refresh_quotas" => self.refresh_quotas(payload),
            "test_model" => self.test_model(payload),
            "discover_models_cmd" => self.discover_models_cmd(payload),
            other => Err(format!("unsupported harness command {other}")),
        }
    }

    pub fn quota_window_count(&self, provider: &str) -> usize {
        let db = self.open_db().unwrap();
        let repo: &dyn QuotaRepository = &db;
        repo.get_windows(provider).unwrap().len()
    }

    pub fn provider_is_exhausted(&self, provider: &str) -> bool {
        let db = self.open_db().unwrap();
        let repo: &dyn QuotaRepository = &db;
        repo.get_quota(provider)
            .unwrap()
            .and_then(|quota| quota.exhausted_at)
            .is_some()
    }

    pub fn seed_discovered_model(&self, name: &str, provider: &str, version: &str) {
        let db = self.open_db().unwrap();
        let repo: &dyn DiscoveryRepository = &db;
        repo.upsert_discovered_model(&DiscoveredModel {
            canonical_name: name.to_string(),
            provider: provider.to_string(),
            discovered_at: "2026-05-02T00:00:00Z".to_string(),
            cli_version: version.to_string(),
        })
        .unwrap();
    }

    pub fn discovered_model_count(&self, provider: &str) -> usize {
        let db = self.open_db().unwrap();
        let repo: &dyn DiscoveryRepository = &db;
        repo.list_discovered_models(Some(provider)).unwrap().len()
    }

    pub fn has_discovered_model(&self, name: &str, provider: &str) -> bool {
        let db = self.open_db().unwrap();
        let repo: &dyn DiscoveryRepository = &db;
        repo.list_discovered_models(Some(provider))
            .unwrap()
            .iter()
            .any(|model| model.canonical_name == name && model.provider == provider)
    }

    fn list_models(&self) -> Result<Value, String> {
        let repo = FilesystemModelConfigRepository::new(self.paths.models_dir());
        let mut models = repo.load_models()?.into_values().collect::<Vec<_>>();
        models.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(json!(
            models
                .into_iter()
                .map(|model| json!({
                    "name": model.name,
                    "prompt_mode": model.prompt_mode,
                    "provider_count": model.providers.len(),
                }))
                .collect::<Vec<_>>()
        ))
    }

    fn refresh_quotas(&self, payload: Value) -> Result<Value, String> {
        let provider_names = payload
            .get("providers")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(ToOwned::to_owned)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let provider_source = FilesystemProviderConfigSource::new(self.paths.providers_path());
        let providers_cfg = provider_source.load_providers()?;
        let db = self.open_db()?;
        let in_flight = quota::InFlight::new();
        let mut out = Vec::new();
        for provider in provider_names {
            let outcome = quota::refresh_provider(
                &provider,
                &providers_cfg,
                &in_flight,
                &db,
                self.runner.as_ref(),
            );
            out.push(match outcome {
                quota::RefreshOutcome::Updated { windows } => json!({
                    "provider": provider,
                    "status": "updated",
                    "windows": windows_json(windows),
                }),
                quota::RefreshOutcome::NoScript => {
                    json!({"provider": provider, "status": "no_script", "windows": []})
                }
                quota::RefreshOutcome::AlreadyInFlight => {
                    json!({"provider": provider, "status": "in_flight", "windows": []})
                }
                quota::RefreshOutcome::Failed(message) => {
                    json!({"provider": provider, "status": "failed", "message": message, "windows": []})
                }
            });
        }
        Ok(Value::Array(out))
    }

    fn test_model(&self, payload: Value) -> Result<Value, String> {
        let name = payload
            .get("model")
            .and_then(Value::as_str)
            .ok_or_else(|| "missing model".to_string())?;
        let model_repo = FilesystemModelConfigRepository::new(self.paths.models_dir());
        let models = model_repo.load_models()?;
        let model = models
            .get(name)
            .cloned()
            .ok_or_else(|| format!("Model '{name}' not found"))?;
        let db = self.open_db()?;
        let provider_index = balancer::select_provider(&model, &db, None);
        let provider_source = FilesystemProviderConfigSource::new(self.paths.providers_path());
        let providers_cfg = provider_source.load_providers()?;
        let (provider, prompt_mode) =
            providers_cfg.effective_provider(&model.providers[provider_index])?;
        let extra_inputs = Default::default();
        let result = executor::cli::execute_effective_with_runner(
            self.runner.as_ref(),
            executor::cli::EffectiveExecuteRequest {
                model: &model,
                provider: &provider,
                provider_index,
                prompt_mode,
                prompt: "Say hello in one sentence.",
                working_dir: None,
                extra_inputs: &extra_inputs,
                parent_invocation_env: None,
            },
        )?;
        if result.exit_code != 0 && crate::diagnostics::classify_exhaustion(&result.stderr) {
            let repo: &dyn QuotaRepository = &db;
            repo.mark_exhausted(&model.providers[provider_index].name)?;
        }
        Ok(json!({
            "success": result.exit_code == 0,
            "stdout": String::from_utf8_lossy(&result.stdout),
            "stderr": result.stderr,
            "exit_code": result.exit_code,
        }))
    }

    fn discover_models_cmd(&self, payload: Value) -> Result<Value, String> {
        let provider = payload
            .get("provider")
            .and_then(Value::as_str)
            .ok_or_else(|| "missing provider".to_string())?;
        let result = discovery::discover_models_with_runner(provider, self.runner.as_ref())?;
        let db = self.open_db()?;
        let repo: &dyn DiscoveryRepository = &db;
        if !result.models.is_empty() {
            repo.delete_stale_models(provider, &result.cli_version)?;
        }
        for model in &result.models {
            repo.upsert_discovered_model(model)?;
        }
        for (model_name, param) in &result.parameters {
            repo.upsert_model_parameter(model_name, provider, param)?;
        }
        Ok(json!({
            "provider": provider,
            "models": result.models,
        }))
    }

    fn open_db(&self) -> Result<crate::state::StateDb, String> {
        let opener = DefaultStateDbOpener;
        opener.open(&self.paths.state_db_path()?)
    }
}

fn windows_json(windows: Vec<QuotaWindowInput>) -> Vec<Value> {
    windows
        .into_iter()
        .map(|window| {
            json!({
                "used_percent": window.used_percent,
                "resets_at": window.resets_at.to_rfc3339(),
            })
        })
        .collect()
}
