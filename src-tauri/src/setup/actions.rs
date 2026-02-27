use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Events pushed to frontend via Tauri Channel during the setup flow.
#[derive(Clone, Serialize)]
#[serde(tag = "event", content = "data", rename_all = "snake_case")]
pub enum SetupEvent {
    Status {
        message: String,
    },
    Progress {
        message: String,
        percent: Option<f64>,
        detail: Option<String>,
    },
    NeedInput {
        action: Action,
    },
    ShowResult {
        content: ResultContent,
    },
    Complete {
        summary: String,
        items_configured: Vec<String>,
    },
    Error {
        message: String,
        recoverable: bool,
    },
}

#[derive(Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ResultContent {
    CommandOutput {
        command: String,
        stdout: String,
        stderr: String,
        exit_code: i32,
    },
    DetectionSummary {
        clis: Vec<CliSummaryItem>,
    },
    ConfigWritten {
        path: String,
        description: String,
    },
    TestResult {
        model: String,
        success: bool,
        output: String,
    },
}

#[derive(Clone, Serialize)]
pub struct CliSummaryItem {
    pub name: String,
    pub installed: bool,
    pub version: Option<String>,
    pub authenticated: bool,
    pub wrapper_count: usize,
    /// Profiles / accounts discovered for this CLI.
    pub profiles: Vec<super::detection::CliProfile>,
    /// Whether the version changed since last detection (None if no tracker).
    pub version_changed: Option<bool>,
    /// The previously stored version, if any.
    pub previous_version: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Action {
    Form(FormAction),
    Wizard(WizardAction),
    Confirm {
        title: String,
        message: String,
        confirm_id: String,
        confirm_label: Option<String>,
        cancel_label: Option<String>,
    },
    OauthFlow {
        provider: String,
        login_command: String,
        instructions: String,
    },
    ApiKeyEntry {
        provider: String,
        env_var: String,
        help_url: Option<String>,
    },
    CliSelection {
        available: Vec<CliOption>,
        message: String,
    },
}

#[derive(Clone, Serialize, Deserialize)]
pub struct FormAction {
    pub title: String,
    pub description: Option<String>,
    pub fields: Vec<FormField>,
    pub form_id: String,
    pub submit_label: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct FormField {
    pub name: String,
    pub label: String,
    pub field_type: String,
    pub required: bool,
    pub default_value: Option<String>,
    pub options: Option<Vec<SelectOption>>,
    pub placeholder: Option<String>,
    pub help_text: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct SelectOption {
    pub value: String,
    pub label: String,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct WizardAction {
    pub title: String,
    pub steps: Vec<WizardStep>,
    pub current_step: usize,
    pub wizard_id: String,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct WizardStep {
    pub label: String,
    pub description: Option<String>,
    pub form: FormAction,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct CliOption {
    pub name: String,
    pub installed: bool,
    pub description: String,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum UserResponse {
    FormSubmit {
        form_id: String,
        values: HashMap<String, String>,
    },
    WizardStep {
        wizard_id: String,
        step: usize,
        values: HashMap<String, String>,
    },
    Confirm {
        confirm_id: String,
        confirmed: bool,
    },
    OauthComplete {
        provider: String,
        success: bool,
    },
    ApiKey {
        provider: String,
        key: String,
    },
    CliSelection {
        selected: Vec<String>,
    },
    Skip {
        reason: Option<String>,
    },
    Cancel,
}

/// What the agent returns per turn
#[derive(Deserialize)]
pub struct AgentTurnResult {
    pub actions: Vec<AgentAction>,
    pub done: bool,
}

/// An instruction from the agent to the flow orchestrator
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentAction {
    Status {
        message: String,
    },
    RunCommand {
        command: String,
        args: Vec<String>,
        description: String,
    },
    WriteConfig {
        path: String,
        content: String,
        description: String,
    },
    TestIntegration {
        model_name: String,
        command: String,
        args: Vec<String>,
    },
    AskUser {
        action: Action,
    },
    SyncSkill {
        source_cli: String,
        target_cli: String,
        skill_name: String,
    },
    SyncMcp {
        source_cli: String,
        target_cli: String,
        mcp_name: String,
        config: String,
    },
    UpdateMemory {
        node_type: String,
        label: String,
        data: String,
        edges: Vec<MemoryEdgeSpec>,
    },
    Complete {
        summary: String,
        items: Vec<String>,
    },
}

#[derive(Deserialize)]
pub struct MemoryEdgeSpec {
    pub target_label: String,
    pub edge_type: String,
}
