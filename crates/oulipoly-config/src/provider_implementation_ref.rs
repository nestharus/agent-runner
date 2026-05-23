/// Singular implementation reference for a model's runtime provider.
/// Parse-only in AGE-178: no dynamic loading, no protocol execution.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderImplementationRef {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "crate")]
    pub crate_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub script: Option<String>,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ProviderImplementationRefError {
    #[error(
        "provider implementation reference is empty: at least one of `path`, `crate`, `binary`, or `script` must be set"
    )]
    NoFlavor,
    #[error(
        "provider implementation reference has multiple flavors: at most one of `path`, `crate`, `binary`, or `script` may be set; found {0:?}"
    )]
    MultipleFlavors(Vec<&'static str>),
    #[error("provider implementation reference: `version` is only valid with `crate`")]
    VersionWithoutCrate,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProviderImplementationFlavor {
    Path,
    Crate,
    Binary,
    Script,
}

impl ProviderImplementationRef {
    pub fn validate(&self) -> Result<(), ProviderImplementationRefError> {
        if self.version.is_some() && self.crate_name.is_none() {
            return Err(ProviderImplementationRefError::VersionWithoutCrate);
        }

        let flavors = self.flavors();
        match flavors.len() {
            0 => Err(ProviderImplementationRefError::NoFlavor),
            1 => Ok(()),
            _ => Err(ProviderImplementationRefError::MultipleFlavors(flavors)),
        }
    }

    pub fn flavor(&self) -> Result<ProviderImplementationFlavor, ProviderImplementationRefError> {
        self.validate()?;
        if self.path.is_some() {
            Ok(ProviderImplementationFlavor::Path)
        } else if self.crate_name.is_some() {
            Ok(ProviderImplementationFlavor::Crate)
        } else if self.binary.is_some() {
            Ok(ProviderImplementationFlavor::Binary)
        } else if self.script.is_some() {
            Ok(ProviderImplementationFlavor::Script)
        } else {
            Err(ProviderImplementationRefError::NoFlavor)
        }
    }

    fn flavors(&self) -> Vec<&'static str> {
        let mut flavors = Vec::new();
        if self.path.is_some() {
            flavors.push("path");
        }
        if self.crate_name.is_some() {
            flavors.push("crate");
        }
        if self.binary.is_some() {
            flavors.push("binary");
        }
        if self.script.is_some() {
            flavors.push("script");
        }
        flavors
    }
}
