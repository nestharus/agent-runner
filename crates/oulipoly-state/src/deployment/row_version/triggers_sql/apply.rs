use super::generate::generate_triggers_for_table;
use crate::deployment::row_version::registry::REGISTRY;

pub fn generate_all_triggers() -> String {
    REGISTRY
        .iter()
        .map(generate_triggers_for_table)
        .collect::<Vec<_>>()
        .join("\n")
}
