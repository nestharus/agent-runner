use oulipoly_runtime::repl_default_provider::RuntimeServices;

#[test]
fn agent_binary_and_runner_new_share_runtime_services_production() {
    let agent_services = RuntimeServices::production(None).unwrap();
    let runner_new_services = RuntimeServices::production(None).unwrap();

    assert_eq!(agent_services.config_root, runner_new_services.config_root);
    assert_eq!(
        agent_services.state_db_path,
        runner_new_services.state_db_path
    );
    assert_eq!(agent_services.working_dir, runner_new_services.working_dir);

    let project = std::env::current_dir()
        .unwrap()
        .join("target")
        .join("wu-prereq-05-parity-project");
    let runner_project_services = RuntimeServices::production(Some(project.clone())).unwrap();

    assert_eq!(
        runner_project_services.config_root,
        agent_services.config_root
    );
    assert_eq!(runner_project_services.state_db_path, None);
    assert_eq!(runner_project_services.working_dir.as_ref(), Some(&project));
}
