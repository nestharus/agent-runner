//! ## Declared roles
//!
//! Roles: mapper.
//!
//! TEST: model/provider config mappers and TOML/path string encoders for the
//! proactive wake integration fixture.

use crate::fixtures::Fixture;
use crate::{MODEL, PROVIDER};
use std::fs;
use std::path::Path;

impl Fixture {
    pub(crate) fn write_provider(&self, body: &str) {
        let script = self.write_script("provider.sh", body);
        fs::write(
            self.models_dir.join(format!("{MODEL}.toml")),
            format!(
                r#"[[providers]]
name = "{PROVIDER}"
args = []
"#,
            ),
        )
        .unwrap();
        fs::write(
            self.app_config_dir.join("providers.toml"),
            format!(
                r#"[{PROVIDER}]
command = {}
args = []
interactive_args = ["interactive"]
prompt_mode = "arg"

[{PROVIDER}.session_capture]
kind = "forced_flag_verified"
flag = "--session-id"

[{PROVIDER}.resume]
kind = "flag"
flag = "--resume"
"#,
                toml_string(&path_string(&script))
            ),
        )
        .unwrap();
    }

    pub(crate) fn write_opencode_provider(&self, body: &str) {
        let script = self.write_script("opencode.sh", body);
        fs::write(
            self.models_dir.join(format!("{MODEL}.toml")),
            r#"[[providers]]
name = "opencode"
args = []
"#,
        )
        .unwrap();
        fs::write(
            self.app_config_dir.join("providers.toml"),
            format!(
                r#"[opencode]
command = {}
args = []
interactive_args = ["interactive"]
prompt_mode = "arg"

[opencode.resume]
kind = "flag"
flag = "--session"
"#,
                toml_string(&path_string(&script))
            ),
        )
        .unwrap();
    }

    pub(crate) fn write_opencode_capture_provider(&self, body: &str) {
        let script = self.write_script("opencode-capture.sh", body);
        fs::write(
            self.models_dir.join(format!("{MODEL}.toml")),
            r#"[[providers]]
name = "opencode"
args = []
"#,
        )
        .unwrap();
        fs::write(
            self.app_config_dir.join("providers.toml"),
            format!(
                r#"[opencode]
command = {}
args = []
interactive_args = ["interactive"]
prompt_mode = "arg"

[opencode.session_capture]
kind = "stdout_json_event"
json_args = ["--json"]
event_type = "step_start"
event_id_path = "sessionID"

[opencode.resume]
kind = "flag"
flag = "--session"
"#,
                toml_string(&path_string(&script))
            ),
        )
        .unwrap();
    }
}

pub(crate) fn path_string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

pub(crate) fn toml_string(value: &str) -> String {
    serde_json::to_string(value).unwrap()
}
