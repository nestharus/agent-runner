//! Declared roles: formatter, predicate

use oulipoly_state::ChainPreview;

pub(super) fn render_resume_list(uuid: &str, previews: &[ChainPreview]) {
    if resume_preview_list_is_empty(previews) {
        render_empty_resume_list(uuid);
        return;
    }
    render_resume_preview_lines(previews);
}

fn resume_preview_list_is_empty(previews: &[ChainPreview]) -> bool {
    previews.is_empty()
}

fn render_empty_resume_list(uuid: &str) {
    println!("No chains found for {uuid}");
}

fn render_resume_preview_lines(previews: &[ChainPreview]) {
    for preview in previews {
        println!("{}", format_resume_list_line(preview));
    }
}

pub(super) fn format_resume_list_line(preview: &ChainPreview) -> String {
    format!(
        "chain_id={} last_used_at={} active_provider={} active_session_id={} turn_count={} recent_turns_count={}",
        preview.chain_id,
        preview.last_used_at.to_rfc3339(),
        preview.active_provider,
        preview.active_session_id,
        preview.turn_count,
        preview.recent_turns.len()
    )
}

pub(super) fn format_resume_list_load_error(error: impl std::fmt::Display) -> String {
    format!("Failed to list resume chains: {error}")
}

pub(super) fn resume_list_subcommand_name() -> String {
    "resume-list".to_string()
}

pub(super) fn format_invalid_session_uuid_error(
    uuid: &str,
    error: impl std::fmt::Display,
) -> String {
    format!("invalid session UUID: {uuid}: {error}")
}
