//! Declared roles: orchestration, accessor

use oulipoly_state::{ChainPreview, StateDb};

pub(crate) fn run_resume_list(uuid: &str) -> Result<i32, String> {
    super::validator::validate_resume_list_uuid(uuid)?;
    render_loaded_resume_list(uuid)?;
    Ok(0)
}

fn render_loaded_resume_list(uuid: &str) -> Result<(), String> {
    super::formatter::render_resume_list(uuid, &load_resume_previews(uuid)?);
    Ok(())
}

fn load_resume_previews(uuid: &str) -> Result<Vec<ChainPreview>, String> {
    let state = StateDb::open_default()?;
    state
        .resume_previews(uuid)
        .map_err(super::formatter::format_resume_list_load_error)
}
