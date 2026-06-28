//! Declared roles: formatter, mapper

use oulipoly_runtime::services::{
    SessionImportProviderReport, SessionImportProviderStatus, SessionImportReport,
    SessionImportTotals,
};
use serde_json::{Value, json};

pub(super) fn render_session_import_report(
    report: &SessionImportReport,
    provider_filter: Option<&str>,
    json_mode: bool,
) -> Result<(), String> {
    if json_mode {
        render_session_import_json(report)
    } else {
        render_session_import_human(report, provider_filter);
        Ok(())
    }
}

fn render_session_import_json(report: &SessionImportReport) -> Result<(), String> {
    serde_json::to_writer(std::io::stdout(), &session_import_report_json(report))
        .map_err(format_session_import_json_error)?;
    println!();
    Ok(())
}

fn render_session_import_human(report: &SessionImportReport, provider_filter: Option<&str>) {
    if report.providers.is_empty() {
        render_empty_report(provider_filter);
        return;
    }

    println!("Session import report");
    for provider in &report.providers {
        println!("{}", format_provider_report_line(provider));
        for warning in &provider.warnings {
            println!("  warning: {}", sanitize_line(warning));
        }
        for error in &provider.errors {
            println!("  error: {}", sanitize_line(error));
        }
    }
    println!("{}", format_totals_line(&report.totals));
}

fn render_empty_report(provider_filter: Option<&str>) {
    match provider_filter {
        Some(filter) => println!(
            "No session import provider targets matched provider filter `{}`",
            sanitize_line(filter)
        ),
        None => println!("No session import provider targets found"),
    }
}

pub(super) fn format_provider_report_line(provider: &SessionImportProviderReport) -> String {
    format!(
        "provider={} model={} settings_id={} status={} discovered={} imported={} skipped={} errors={} warnings={} turns_backfilled={}",
        sanitize_cell(&provider.provider_name),
        sanitize_cell(&provider.model_name),
        sanitize_cell(&provider.settings_id),
        format_status(&provider.status),
        provider.discovered,
        provider.imported,
        provider.skipped,
        provider.errors.len(),
        provider.warnings.len(),
        provider.turns_backfilled,
    )
}

pub(super) fn format_totals_line(totals: &SessionImportTotals) -> String {
    format!(
        "totals providers={} succeeded={} skipped_providers={} failed={} discovered={} imported={} skipped_sessions={} errors={} warnings={} turns_backfilled={}",
        totals.providers_total,
        totals.providers_succeeded,
        totals.providers_skipped,
        totals.providers_failed,
        totals.discovered,
        totals.imported,
        totals.skipped,
        totals.errors,
        totals.warnings,
        totals.turns_backfilled,
    )
}

fn session_import_report_json(report: &SessionImportReport) -> Value {
    json!({
        "providers": report.providers.iter().map(provider_report_json).collect::<Vec<_>>(),
        "totals": totals_json(&report.totals),
    })
}

fn provider_report_json(provider: &SessionImportProviderReport) -> Value {
    json!({
        "model_name": provider.model_name,
        "provider_name": provider.provider_name,
        "settings_id": provider.settings_id,
        "status": status_json(&provider.status),
        "discovered": provider.discovered,
        "imported": provider.imported,
        "skipped": provider.skipped,
        "errors": provider.errors,
        "warnings": provider.warnings,
        "turns_backfilled": provider.turns_backfilled,
    })
}

fn totals_json(totals: &SessionImportTotals) -> Value {
    json!({
        "providers_total": totals.providers_total,
        "providers_succeeded": totals.providers_succeeded,
        "providers_skipped": totals.providers_skipped,
        "providers_failed": totals.providers_failed,
        "discovered": totals.discovered,
        "imported": totals.imported,
        "skipped": totals.skipped,
        "errors": totals.errors,
        "warnings": totals.warnings,
        "turns_backfilled": totals.turns_backfilled,
    })
}

fn status_json(status: &SessionImportProviderStatus) -> Value {
    match status {
        SessionImportProviderStatus::Succeeded => json!({ "kind": "succeeded" }),
        SessionImportProviderStatus::Skipped { reason } => {
            json!({ "kind": "skipped", "reason": reason })
        }
        SessionImportProviderStatus::Failed => json!({ "kind": "failed" }),
    }
}

fn format_status(status: &SessionImportProviderStatus) -> String {
    match status {
        SessionImportProviderStatus::Succeeded => "succeeded".to_string(),
        SessionImportProviderStatus::Skipped { reason } => {
            format!("skipped({})", sanitize_cell(reason))
        }
        SessionImportProviderStatus::Failed => "failed".to_string(),
    }
}

fn sanitize_cell(value: &str) -> String {
    value
        .chars()
        .map(|ch| match ch {
            '\t' | '\n' | '\r' => ' ',
            _ => ch,
        })
        .collect()
}

fn sanitize_line(value: &str) -> String {
    sanitize_cell(value)
}

fn format_session_import_json_error(error: serde_json::Error) -> String {
    format!("Failed to serialize session import report: {error}")
}
