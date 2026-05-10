use super::RowVersion;

pub fn same_or_higher(target_version: RowVersion, candidate_version: RowVersion) -> bool {
    target_version.0 >= candidate_version.0
}
