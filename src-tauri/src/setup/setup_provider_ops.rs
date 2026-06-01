use oulipoly_config::app::SetupBrainConfig;
use oulipoly_provider::client::{ProviderClient, ProviderClientOptions};
use oulipoly_provider::error::ProviderClientError;
use oulipoly_provider::generated::{
    CONTRACT_VERSION, DiscoveryAccountsResult, DiscoveryObject, ErrorCategory, HostContext,
    RequestEnvelope, SetupDetectResult, SetupInstallPlanResult, SetupObject, SetupSyncPlanResult,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;

use super::setup_brain_host::provider_artifact_from_ref;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SetupProviderDiagnostic {
    pub kind: &'static str,
    pub operation: &'static str,
    pub fallback_used: bool,
}

pub fn unsupported_setup_operation(operation: &'static str) -> SetupProviderDiagnostic {
    SetupProviderDiagnostic {
        kind: "unsupported_setup_operation",
        operation,
        fallback_used: false,
    }
}

pub fn setup_provider_error(operation: &'static str) -> SetupProviderDiagnostic {
    SetupProviderDiagnostic {
        kind: "setup_provider_error",
        operation,
        fallback_used: false,
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SetupProviderContext {
    pub context: Value,
    pub diagnostics: Vec<SetupProviderDiagnostic>,
    pub operation_calls: Vec<&'static str>,
}

pub fn build_setup_provider_context(config: &SetupBrainConfig) -> SetupProviderContext {
    let mut context = json!({});
    let mut diagnostics = Vec::new();
    let mut operation_calls = Vec::new();

    let client = match provider_artifact_from_ref(&config.artifact) {
        Ok(artifact) => ProviderClient::new(artifact, ProviderClientOptions::default()),
        Err(_) => {
            for operation in SETUP_PROVIDER_OPERATIONS {
                operation_calls.push(*operation);
                diagnostics.push(setup_provider_error(operation));
            }
            return SetupProviderContext {
                context,
                diagnostics,
                operation_calls,
            };
        }
    };

    let settings_id = config.settings_id.as_deref();
    for operation in SETUP_PROVIDER_OPERATIONS {
        operation_calls.push(*operation);
        match invoke_setup_provider_operation(&client, operation, settings_id) {
            Ok(addition) => merge_context(&mut context, addition),
            Err(diagnostic) => diagnostics.push(diagnostic),
        }
    }

    SetupProviderContext {
        context,
        diagnostics,
        operation_calls,
    }
}

pub fn map_setup_detect_report(report: Value) -> Value {
    json!({ "setup": { "detect": report } })
}

pub fn map_setup_install_plan(steps: Vec<Value>) -> Value {
    json!({ "setup": { "install_plan": { "steps": steps } } })
}

pub fn map_setup_sync_plan(operations: Vec<Value>) -> Value {
    json!({ "setup": { "sync_plan": { "operations": operations } } })
}

pub fn map_discovery_accounts(accounts: Vec<Value>) -> Value {
    json!({ "discovery": { "accounts": accounts } })
}

const SETUP_PROVIDER_OPERATIONS: &[&str] = &[
    "setup.detect",
    "setup.install_plan",
    "setup.sync_plan",
    "discovery.accounts",
];

fn invoke_setup_provider_operation(
    client: &ProviderClient,
    operation: &'static str,
    settings_id: Option<&str>,
) -> Result<Value, SetupProviderDiagnostic> {
    match operation {
        "setup.detect" => client
            .invoke_typed::<SetupDetectResult, _>(
                operation,
                setup_request(operation, settings_id),
                [],
            )
            .map(|result| {
                map_setup_detect_report(serde_json::to_value(result).unwrap_or(json!({})))
            })
            .map_err(|error| map_setup_provider_error_to_diagnostic(operation, error)),
        "setup.install_plan" => client
            .invoke_typed::<SetupInstallPlanResult, _>(
                operation,
                setup_request(operation, settings_id),
                [],
            )
            .map(|result| map_setup_install_plan(result.steps))
            .map_err(|error| map_setup_provider_error_to_diagnostic(operation, error)),
        "setup.sync_plan" => client
            .invoke_typed::<SetupSyncPlanResult, _>(
                operation,
                setup_request(operation, settings_id),
                [],
            )
            .map(|result| map_setup_sync_plan(result.operations))
            .map_err(|error| map_setup_provider_error_to_diagnostic(operation, error)),
        "discovery.accounts" => client
            .invoke_typed::<DiscoveryAccountsResult, _>(
                operation,
                discovery_request(operation, settings_id),
                [],
            )
            .map(|result| map_discovery_accounts(result.accounts))
            .map_err(|error| map_setup_provider_error_to_diagnostic(operation, error)),
        _ => Err(unsupported_setup_operation(operation)),
    }
}

fn map_setup_provider_error_to_diagnostic(
    operation: &'static str,
    error: ProviderClientError,
) -> SetupProviderDiagnostic {
    if error.provider_category() == Some(ErrorCategory::Unsupported) {
        unsupported_setup_operation(operation)
    } else {
        setup_provider_error(operation)
    }
}

fn setup_request(operation: &str, settings_id: Option<&str>) -> Value {
    let mut fields = BTreeMap::new();
    if let Some(settings_id) = settings_id {
        fields.insert(
            "settings_id".to_string(),
            Value::String(settings_id.to_string()),
        );
    }
    serde_json::to_value(RequestEnvelope {
        contract: CONTRACT_VERSION.to_string(),
        request_id: request_id(operation),
        provider_instance_id: None,
        host: default_host_context(),
        params: SetupObject { fields },
    })
    .unwrap_or_else(|_| json!({}))
}

fn discovery_request(operation: &str, settings_id: Option<&str>) -> Value {
    let mut fields = BTreeMap::new();
    if let Some(settings_id) = settings_id {
        fields.insert(
            "settings_id".to_string(),
            Value::String(settings_id.to_string()),
        );
    }
    serde_json::to_value(RequestEnvelope {
        contract: CONTRACT_VERSION.to_string(),
        request_id: request_id(operation),
        provider_instance_id: None,
        host: default_host_context(),
        params: DiscoveryObject { fields },
    })
    .unwrap_or_else(|_| json!({}))
}

fn request_id(operation: &str) -> String {
    format!("setup-provider-{operation}")
}

fn default_host_context() -> HostContext {
    HostContext {
        app: "oulipoly-agent-runner".to_string(),
        app_version: None,
        platform: None,
        working_directory: None,
        config_root: None,
        data_root: None,
        env: BTreeMap::new(),
        deadline_unix_ms: None,
    }
}

fn merge_context(target: &mut Value, addition: Value) {
    merge_object(target, addition);
}

fn merge_object(target: &mut Value, addition: Value) {
    let (Some(target), Some(addition)) = (target.as_object_mut(), addition.as_object()) else {
        return;
    };
    for (key, value) in addition {
        if target.contains_key(key) {
            merge_object(target.get_mut(key).expect("existing key"), value.clone());
        } else {
            target.insert(key.clone(), value.clone());
        }
    }
}

#[cfg(debug_assertions)]
#[doc(hidden)]
pub mod test_support {
    use super::*;
    use crate::setup::setup_brain_host::test_support::TurnRequest;

    #[derive(Clone, Debug)]
    pub enum SetupOperationFailure {
        Unsupported,
        ProviderError { message: String },
    }

    #[derive(Clone, Debug)]
    pub struct SetupProviderFixture {
        id: String,
        detect_report: Value,
        install_plan_steps: Vec<Value>,
        sync_plan_operations: Vec<Value>,
        discovered_accounts: Vec<Value>,
        failure: Option<(&'static str, SetupOperationFailure)>,
    }

    impl SetupProviderFixture {
        pub fn new(id: &str) -> Self {
            Self {
                id: id.to_string(),
                detect_report: json!({}),
                install_plan_steps: Vec::new(),
                sync_plan_operations: Vec::new(),
                discovered_accounts: Vec::new(),
                failure: None,
            }
        }

        pub fn with_detect_report(mut self, report: Value) -> Self {
            self.detect_report = report;
            self
        }

        pub fn with_install_plan_steps(mut self, steps: Vec<Value>) -> Self {
            self.install_plan_steps = steps;
            self
        }

        pub fn with_sync_plan_operations(mut self, operations: Vec<Value>) -> Self {
            self.sync_plan_operations = operations;
            self
        }

        pub fn with_discovered_accounts(mut self, accounts: Vec<&str>) -> Self {
            self.discovered_accounts = accounts.into_iter().map(|account| json!(account)).collect();
            self
        }

        pub fn with_failure(
            mut self,
            operation: &'static str,
            failure: SetupOperationFailure,
        ) -> Self {
            self.failure = Some((operation, failure));
            self
        }
    }

    #[derive(Clone, Debug)]
    pub struct SetupProviderFlowHarness {
        artifact_id: String,
        provider: SetupProviderFixture,
    }

    impl SetupProviderFlowHarness {
        pub fn configured(artifact_id: &str) -> Self {
            Self {
                artifact_id: artifact_id.to_string(),
                provider: SetupProviderFixture::new("fixture-setup-provider"),
            }
        }

        pub fn with_setup_provider(mut self, provider: SetupProviderFixture) -> Self {
            self.provider = provider;
            self
        }

        pub fn run_until_first_brain_turn(self) -> SetupProviderOutcome {
            let _ = (&self.artifact_id, &self.provider.id);
            let mut calls = Vec::new();
            let mut diagnostics = Vec::new();
            let mut context = json!({});

            for operation in [
                "setup.detect",
                "setup.install_plan",
                "setup.sync_plan",
                "discovery.accounts",
            ] {
                calls.push(operation);
                if let Some((failed_operation, failure)) = &self.provider.failure
                    && *failed_operation == operation
                {
                    diagnostics.push(match failure {
                        SetupOperationFailure::Unsupported => {
                            unsupported_setup_operation(operation)
                        }
                        SetupOperationFailure::ProviderError { message } => {
                            let _ = message;
                            setup_provider_error(operation)
                        }
                    });
                    continue;
                }

                merge_context(
                    &mut context,
                    match operation {
                        "setup.detect" => {
                            map_setup_detect_report(self.provider.detect_report.clone())
                        }
                        "setup.install_plan" => {
                            map_setup_install_plan(self.provider.install_plan_steps.clone())
                        }
                        "setup.sync_plan" => {
                            map_setup_sync_plan(self.provider.sync_plan_operations.clone())
                        }
                        "discovery.accounts" => {
                            map_discovery_accounts(self.provider.discovered_accounts.clone())
                        }
                        _ => json!({}),
                    },
                );
            }

            SetupProviderOutcome {
                setup_provider_calls: calls,
                diagnostics,
                runner_recipe_fallbacks: Vec::new(),
                turn_request: TurnRequest::new_for_provider_ops(context),
            }
        }
    }

    #[derive(Clone, Debug)]
    pub struct SetupProviderOutcome {
        setup_provider_calls: Vec<&'static str>,
        diagnostics: Vec<SetupProviderDiagnostic>,
        runner_recipe_fallbacks: Vec<&'static str>,
        turn_request: TurnRequest,
    }

    impl SetupProviderOutcome {
        pub fn setup_provider_calls(&self) -> Vec<&'static str> {
            self.setup_provider_calls.clone()
        }

        pub fn diagnostics(&self) -> &[SetupProviderDiagnostic] {
            &self.diagnostics
        }

        pub fn runner_recipe_fallbacks(&self) -> Vec<&'static str> {
            self.runner_recipe_fallbacks.clone()
        }

        pub fn only_turn_request(&self) -> &TurnRequest {
            &self.turn_request
        }
    }
}
