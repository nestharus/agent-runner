use oulipoly_config::{
    ProviderImplementationFlavor, ProviderImplementationRef, ProviderImplementationRefError,
};

fn empty_ref() -> ProviderImplementationRef {
    ProviderImplementationRef {
        path: None,
        crate_name: None,
        version: None,
        binary: None,
        script: None,
    }
}

fn path_ref(value: &str) -> ProviderImplementationRef {
    ProviderImplementationRef {
        path: Some(value.to_string()),
        ..empty_ref()
    }
}

fn crate_ref(value: &str) -> ProviderImplementationRef {
    ProviderImplementationRef {
        crate_name: Some(value.to_string()),
        ..empty_ref()
    }
}

fn binary_ref(value: &str) -> ProviderImplementationRef {
    ProviderImplementationRef {
        binary: Some(value.to_string()),
        ..empty_ref()
    }
}

fn script_ref(value: &str) -> ProviderImplementationRef {
    ProviderImplementationRef {
        script: Some(value.to_string()),
        ..empty_ref()
    }
}

#[test]
// Risk: Test-intent item 5, unit level, A5/A178-2.
fn validate_rejects_empty() {
    let err = empty_ref().validate().unwrap_err();

    assert_eq!(err, ProviderImplementationRefError::NoFlavor);
}

#[test]
// Risk: Test-intent item 5, unit level, A5/A178-1/A178-2.
fn validate_rejects_multiple_flavors() {
    let cases = [
        ProviderImplementationRef {
            binary: Some("/usr/local/bin/example-provider".to_string()),
            ..path_ref("./agent-runner-example")
        },
        ProviderImplementationRef {
            script: Some("scripts/example-provider-locate".to_string()),
            ..crate_ref("agent-runner-example")
        },
        ProviderImplementationRef {
            script: Some("scripts/example-provider-locate".to_string()),
            ..binary_ref("/usr/local/bin/example-provider")
        },
    ];

    for implementation in cases {
        assert!(matches!(
            implementation.validate(),
            Err(ProviderImplementationRefError::MultipleFlavors(_))
        ));
    }
}

#[test]
// Risk: Test-intent item 5, unit level, A5/A178-2.
fn validate_rejects_version_without_crate() {
    let implementation = ProviderImplementationRef {
        version: Some("0.1".to_string()),
        ..binary_ref("/usr/local/bin/example-provider")
    };

    let err = implementation.validate().unwrap_err();

    assert_eq!(err, ProviderImplementationRefError::VersionWithoutCrate);
}

#[test]
// Risk: Test-intent items 1-3, unit level, A5/A178-2.
fn validate_accepts_each_flavor() {
    let cases = [
        (
            path_ref("./agent-runner-example"),
            ProviderImplementationFlavor::Path,
        ),
        (
            ProviderImplementationRef {
                version: Some("0.1".to_string()),
                ..crate_ref("agent-runner-example")
            },
            ProviderImplementationFlavor::Crate,
        ),
        (
            binary_ref("/usr/local/bin/example-provider"),
            ProviderImplementationFlavor::Binary,
        ),
        (
            script_ref("scripts/example-provider-locate"),
            ProviderImplementationFlavor::Script,
        ),
    ];

    for (implementation, flavor) in cases {
        assert_eq!(implementation.validate(), Ok(()));
        assert_eq!(implementation.flavor(), Ok(flavor));
    }
}
