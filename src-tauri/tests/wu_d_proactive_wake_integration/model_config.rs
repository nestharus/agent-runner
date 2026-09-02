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
        let provider = self.write_executable("provider.py", body);
        fs::write(
            self.models_dir.join(format!("{MODEL}.toml")),
            format!(
                r#"prompt_mode = "arg"

[[providers]]
name = "{PROVIDER}"
args = []
"#,
            ),
        )
        .unwrap();
        fs::write(
            self.app_config_dir.join("providers.toml"),
            crate::provider_authority_fixture::with_explicit_provider_authority_at(
                &format!(
                    r#"[{PROVIDER}]
command = "wu-d-native-fixture"
args = []
prompt_mode = "arg"
settings_id = "{PROVIDER}"
"#
                ),
                "wu-d-provider",
                &provider,
            ),
        )
        .unwrap();
    }
}

pub(crate) fn path_string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}
