pub mod support {
    pub mod provider_client;
}

use oulipoly_provider::resolver::{
    ProviderArtifactRef, ProviderResolveOptions, ProviderResolver, RuntimeDisabledArtifact,
};
use std::fs;
use std::path::Path;
use support::provider_client::{executable_script, non_executable_script, temp_fixture_dir};

#[test]
fn resolves_absolute_path_and_builds_exact_subcommand_argv() {
    let artifact = ProviderArtifactRef::Path {
        path: executable_script(),
    };
    let resolved = ProviderResolver::new(ProviderResolveOptions::default())
        .resolve(&artifact, None)
        .expect("absolute executable path should resolve");

    assert_eq!(
        resolved.argv_for_subcommand("describe"),
        vec![executable_script().into_os_string(), "describe".into()]
    );
}

#[test]
fn resolves_config_relative_path_against_config_directory() {
    let dir = temp_fixture_dir("resolver-relative");
    fs::create_dir_all(&dir).expect("temp dir should be created");
    let target = dir.join("provider-a");
    fs::copy(executable_script(), &target).expect("fixture copy should succeed");
    make_executable(&target);

    let artifact = ProviderArtifactRef::Path {
        path: "provider-a".into(),
    };
    let resolved = ProviderResolver::new(ProviderResolveOptions::default())
        .resolve(&artifact, Some(&dir))
        .expect("config-relative executable path should resolve");

    assert_eq!(resolved.executable(), target);
    assert_eq!(
        resolved.argv_for_subcommand("quota.probe"),
        vec![target.into_os_string(), "quota.probe".into()]
    );
}

#[test]
fn resolves_binary_only_from_explicit_path_entries() {
    let dir = temp_fixture_dir("resolver-path");
    fs::create_dir_all(&dir).expect("temp dir should be created");
    let target = dir.join("example");
    fs::copy(executable_script(), &target).expect("fixture copy should succeed");
    make_executable(&target);

    let options = ProviderResolveOptions::default().with_path_entries([dir.clone()]);
    let artifact = ProviderArtifactRef::Binary {
        name: "example".to_owned(),
    };
    let resolved = ProviderResolver::new(options)
        .resolve(&artifact, None)
        .expect("binary should resolve from explicit path entries");

    assert_eq!(resolved.executable(), target);
    assert_eq!(
        resolved.argv_for_subcommand("describe"),
        vec![target.into_os_string(), "describe".into()]
    );
}

#[test]
fn resolves_packaged_binary_only_when_allowlisted_in_search_root() {
    let root = temp_fixture_dir("resolver-packaged");
    fs::create_dir_all(&root).expect("temp dir should be created");
    let target = root.join("fake-provider");
    fs::copy(executable_script(), &target).expect("fixture copy should succeed");
    make_executable(&target);

    let options = ProviderResolveOptions::default()
        .with_packaged_search_roots([root.clone()])
        .with_packaged_allowlist(["fake-provider"]);
    let artifact = ProviderArtifactRef::Binary {
        name: "fake-provider".to_owned(),
    };
    let resolved = ProviderResolver::new(options)
        .resolve(&artifact, None)
        .expect("allowlisted packaged binary should resolve");

    assert_eq!(resolved.executable(), target);
}

#[test]
fn resolves_direct_executable_script_without_shell_wrapper() {
    let artifact = ProviderArtifactRef::Script {
        path: executable_script(),
    };
    let resolved = ProviderResolver::new(ProviderResolveOptions::default())
        .resolve(&artifact, None)
        .expect("direct executable script should resolve");

    assert_eq!(
        resolved.argv_for_subcommand("describe"),
        vec![executable_script().into_os_string(), "describe".into()]
    );
    assert!(!resolved.uses_shell_wrapper());
}

#[test]
fn rejects_missing_directory_and_non_executable_paths() {
    let dir = temp_fixture_dir("resolver-rejects-path");
    fs::create_dir_all(&dir).expect("temp dir should be created");
    let non_executable = dir.join("provider-a");
    fs::write(&non_executable, "not executable").expect("fixture file should be written");

    let resolver = ProviderResolver::new(ProviderResolveOptions::default());
    assert_eq!(
        resolver
            .resolve(&ProviderArtifactRef::Path { path: dir.clone() }, None)
            .expect_err("directories are not executable artifacts")
            .kind(),
        "not_executable"
    );
    assert_eq!(
        resolver
            .resolve(
                &ProviderArtifactRef::Path {
                    path: non_executable
                },
                None
            )
            .expect_err("plain file should be rejected")
            .kind(),
        "not_executable"
    );
    assert_eq!(
        resolver
            .resolve(
                &ProviderArtifactRef::Path {
                    path: dir.join("missing")
                },
                None
            )
            .expect_err("missing path should be rejected")
            .kind(),
        "missing_artifact"
    );
}

#[test]
fn rejects_binary_outside_explicit_roots_or_allowlist() {
    let resolver = ProviderResolver::new(ProviderResolveOptions::default());
    let artifact = ProviderArtifactRef::Binary {
        name: "example".to_owned(),
    };

    assert_eq!(
        resolver
            .resolve(&artifact, None)
            .expect_err("binary search must be explicit")
            .kind(),
        "missing_artifact"
    );
}

#[test]
fn rejects_invalid_binary_names_before_searching_roots() {
    let root = temp_fixture_dir("resolver-invalid-binary-name");
    fs::create_dir_all(&root).expect("temp dir should be created");
    let resolver =
        ProviderResolver::new(ProviderResolveOptions::default().with_path_entries([root]));

    for name in [
        "",
        "/usr/bin/sh",
        "../escape",
        "sub/dir/x",
        "sub\\dir\\x",
        ".",
    ] {
        let artifact = ProviderArtifactRef::Binary {
            name: name.to_owned(),
        };
        assert_eq!(
            resolver
                .resolve(&artifact, None)
                .expect_err("invalid binary names should reject before root search")
                .kind(),
            "invalid_binary_name",
            "name {name:?}"
        );
    }
}

#[test]
fn rejects_packaged_binary_present_but_not_allowlisted() {
    let root = temp_fixture_dir("resolver-packaged-deny");
    fs::create_dir_all(&root).expect("temp dir should be created");
    let target = root.join("not-allowlisted");
    fs::copy(executable_script(), &target).expect("fixture copy should succeed");
    make_executable(&target);

    let options = ProviderResolveOptions::default()
        .with_packaged_search_roots([root])
        .with_packaged_allowlist(["different-binary"]);
    let artifact = ProviderArtifactRef::Binary {
        name: "not-allowlisted".to_owned(),
    };

    assert_eq!(
        ProviderResolver::new(options)
            .resolve(&artifact, None)
            .expect_err("packaged binary must be allowlisted even when present")
            .kind(),
        "missing_artifact"
    );
}

#[cfg(unix)]
#[test]
fn rejects_binary_symlink_that_escapes_explicit_root() {
    use std::os::unix::fs::symlink;

    let root = temp_fixture_dir("resolver-symlink-root");
    let outside = temp_fixture_dir("resolver-symlink-outside");
    fs::create_dir_all(&root).expect("root should be created");
    fs::create_dir_all(&outside).expect("outside dir should be created");
    let outside_target = outside.join("escaped");
    fs::copy(executable_script(), &outside_target).expect("fixture copy should succeed");
    make_executable(&outside_target);
    symlink(&outside_target, root.join("escaped")).expect("symlink should be created");

    let options = ProviderResolveOptions::default().with_path_entries([root]);
    let artifact = ProviderArtifactRef::Binary {
        name: "escaped".to_owned(),
    };

    assert_eq!(
        ProviderResolver::new(options)
            .resolve(&artifact, None)
            .expect_err("binary symlink target must remain within explicit root")
            .kind(),
        "unsafe_binary"
    );
}

#[test]
fn rejects_non_executable_script_without_shell_opt_in() {
    let resolver = ProviderResolver::new(ProviderResolveOptions::default());
    let artifact = ProviderArtifactRef::Script {
        path: non_executable_script(),
    };

    assert_eq!(
        resolver
            .resolve(&artifact, None)
            .expect_err("non-executable script should be rejected")
            .kind(),
        "not_executable"
    );
}

#[test]
fn rejects_runtime_disabled_crate_flavor() {
    let artifact = RuntimeDisabledArtifact::Crate {
        crate_name: "example-provider".to_owned(),
        version: Some("0.1.0".to_owned()),
    };

    let error = ProviderResolver::new(ProviderResolveOptions::default())
        .resolve_runtime_disabled(&artifact)
        .expect_err("crate artifact refs remain parse-only in this release");

    assert_eq!(error.kind(), "runtime_disabled");
}

fn make_executable(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(path)
            .expect("fixture metadata should be readable")
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(path, permissions).expect("fixture permissions should update");
    }
}
