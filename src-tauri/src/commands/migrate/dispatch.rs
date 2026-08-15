//! Declared role: orchestration

use super::formatter::{render_migrate_rebuild_report, render_session_chain_backfill_report};
use super::rebuild::{complete_migrate_rebuild, execute_migrate_rebuild, migrate_rebuild_plan};
use super::validator::validate_migrate_rebuild_flag;
use crate::commands::compaction_backfill::run_compaction_backfill;
use std::path::Path;

pub(crate) struct MigrateSessionOwnershipArgs<'a> {
    pub(crate) dry_run: bool,
    pub(crate) apply: bool,
    pub(crate) rollback: bool,
    pub(crate) corrective: bool,
    pub(crate) scratch_dir: Option<&'a Path>,
    pub(crate) backup_dir: Option<&'a Path>,
    pub(crate) confirm_mutate_live_db: bool,
    pub(crate) skip_provider_proof: bool,
    pub(crate) confirm_skip_provider_proof: bool,
    pub(crate) state_db: Option<&'a Path>,
    pub(crate) models_dir: Option<&'a Path>,
}

pub(crate) fn run_migrate_db() -> Result<i32, String> {
    let state = super::accessor::open_default_state_db()?;
    let report = state.backfill_session_chains()?;
    render_session_chain_backfill_report(&report);
    run_compaction_backfill(&state)?;
    Ok(0)
}

pub(crate) fn run_migrate(rebuild: bool) -> Result<i32, String> {
    validate_migrate_rebuild_flag(rebuild)?;
    run_migrate_rebuild()
}

pub(crate) fn run_migrate_session_ownership(
    args: MigrateSessionOwnershipArgs<'_>,
) -> Result<i32, String> {
    let mode = validate_session_ownership_mode(&args)?;
    let live_state_db_path = match args.state_db {
        Some(path) => path.to_path_buf(),
        None => super::accessor::default_state_db_path()?,
    };
    match mode {
        SessionOwnershipMode::DryRun { scratch_dir } => {
            let opts = super::session_ownership::DryRunOptions {
                live_state_db_path,
                mailbox_db_path: None,
                models_dir: args.models_dir.map(Path::to_path_buf),
                scratch_dir,
                skip_provider_proof: args.skip_provider_proof,
            };
            let outcome = if args.corrective {
                super::session_ownership::run_session_ownership_corrective_dry_run(opts)
            } else {
                super::session_ownership::run_session_ownership_dry_run(opts)
            }
            .map_err(|err| err.to_string())?;
            println!("report={}", outcome.report_path.display());
            println!("live_db_mutated=no");
        }
        SessionOwnershipMode::Apply { backup_dir } => {
            let opts = super::session_ownership::ApplyOptions {
                live_state_db_path,
                models_dir: args.models_dir.map(Path::to_path_buf),
                backup_dir,
                skip_provider_proof: args.skip_provider_proof,
            };
            let outcome = if args.corrective {
                super::session_ownership::run_session_ownership_corrective_apply(opts)
            } else {
                super::session_ownership::run_session_ownership_apply(opts)
            }
            .map_err(|err| err.to_string())?;
            println!("report={}", outcome.report_path.display());
            println!("live_db_mutated=yes");
            println!("backup={}", outcome.backup_path.display());
        }
        SessionOwnershipMode::Rollback => {
            let opts = super::session_ownership::RollbackOptions { live_state_db_path };
            let outcome = if args.corrective {
                super::session_ownership::run_session_ownership_corrective_rollback(opts)
            } else {
                super::session_ownership::run_session_ownership_rollback(opts)
            }
            .map_err(|err| err.to_string())?;
            println!("report={}", outcome.report_path.display());
            println!("live_db_mutated=yes");
        }
    }
    Ok(0)
}

enum SessionOwnershipMode {
    DryRun { scratch_dir: std::path::PathBuf },
    Apply { backup_dir: std::path::PathBuf },
    Rollback,
}

fn validate_session_ownership_mode(
    args: &MigrateSessionOwnershipArgs<'_>,
) -> Result<SessionOwnershipMode, String> {
    validate_single_mode(args.dry_run, args.apply, args.rollback)?;
    validate_skip_provider_proof_ack(args.skip_provider_proof, args.confirm_skip_provider_proof)?;
    if args.dry_run {
        return Ok(SessionOwnershipMode::DryRun {
            scratch_dir: args
                .scratch_dir
                .ok_or_else(|| {
                    "migrate-session-ownership --dry-run requires --scratch-dir <dir>".to_string()
                })?
                .to_path_buf(),
        });
    }
    if args.apply {
        if !args.confirm_mutate_live_db {
            return Err(
                "migrate-session-ownership --apply requires --confirm-mutate-live-db".to_string(),
            );
        }
        return Ok(SessionOwnershipMode::Apply {
            backup_dir: args
                .backup_dir
                .ok_or_else(|| {
                    "migrate-session-ownership --apply requires --backup-dir <dir>".to_string()
                })?
                .to_path_buf(),
        });
    }
    if !args.confirm_mutate_live_db {
        return Err(
            "migrate-session-ownership --rollback requires --confirm-mutate-live-db".to_string(),
        );
    }
    Ok(SessionOwnershipMode::Rollback)
}

fn validate_single_mode(dry_run: bool, apply: bool, rollback: bool) -> Result<(), String> {
    match [dry_run, apply, rollback]
        .into_iter()
        .filter(|selected| *selected)
        .count()
    {
        1 => Ok(()),
        0 => Err(
            "migrate-session-ownership requires exactly one of --dry-run, --apply, or --rollback"
                .to_string(),
        ),
        _ => Err(
            "migrate-session-ownership accepts only one of --dry-run, --apply, or --rollback"
                .to_string(),
        ),
    }
}

fn validate_skip_provider_proof_ack(skip: bool, confirm: bool) -> Result<(), String> {
    if skip && !confirm {
        return Err(
            "migrate-session-ownership --skip-provider-proof requires --confirm-skip-provider-proof"
                .to_string(),
        );
    }
    Ok(())
}

fn run_migrate_rebuild() -> Result<i32, String> {
    let Some(mut plan) = migrate_rebuild_plan()? else {
        return Ok(0);
    };
    let authority = super::accessor::acquire_rebuild_authority(&plan.db_path)?;
    plan.bind_to_authority_path(authority.path())?;
    let mut sidecar_authority = super::accessor::acquire_sidecar_rebuild_authority(&authority)?;
    execute_migrate_rebuild(&mut plan, &mut sidecar_authority)?;
    super::accessor::initialize_after_rebuild(&plan.db_path, &authority)?;
    super::accessor::initialize_sidecar_after_rebuild(&mut sidecar_authority)?;
    complete_migrate_rebuild(&plan)?;
    render_migrate_rebuild_report(&plan);
    Ok(0)
}
