use chrono::{DateTime, Utc};
use oulipoly_agent_store::TombstoneStatus;
use uuid::Uuid;

use super::*;
use crate::ScratchpadName;
use crate::map_store_retirement_status;

const GENERATED_MAX_LEN: usize = 8;

fn fixed_uuid(value: &str) -> Uuid {
    Uuid::parse_str(value).expect("valid fixed UUID")
}

fn fixed_name(value: &str) -> ScratchpadName {
    ScratchpadName::new(value).expect("valid fixed scratchpad name")
}

fn fixed_address(invocation_uuid: &str, name: &str) -> ScratchpadAddress {
    ScratchpadAddress {
        invocation_uuid: fixed_uuid(invocation_uuid),
        name: fixed_name(name),
    }
}

fn fixed_time(value: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(value)
        .expect("valid fixed timestamp")
        .with_timezone(&Utc)
}

// Risk S2 CG HIGH; level S2 EXHAUSTIVE via S5 LEAN; proposal test-intent/contract default source.
#[test]
fn empty_delete_reduction_defaults_every_field() {
    let reduction = DeleteStatusReduction::default();

    assert!(reduction.tombstoned_versions.is_empty());
    assert!(reduction.already_tombstoned_versions.is_empty());
    assert_eq!(reduction.last_tombstoned_at, None);
}

// Risk S2/S3 and CQ-C2-F01; level S2/S3 EXHAUSTIVE via S5 LEAN; proposal partition source.
#[test]
fn mixed_delete_partition_is_stable_exactly_once_and_time_isolated() {
    let seeded_time = fixed_time("2024-01-02T03:04:05.123456789Z");
    let inputs = [
        (11, RetirementStatus::Retired),
        (12, RetirementStatus::AlreadyRetired),
        (13, RetirementStatus::Retired),
        (14, RetirementStatus::AlreadyRetired),
        (15, RetirementStatus::Retired),
    ];
    let mut reduction = DeleteStatusReduction::default();
    project_last_delete_tombstoned_at(&mut reduction, seeded_time);

    for (version, status) in inputs {
        partition_delete_version(&mut reduction, version, status);
    }

    assert_eq!(reduction.tombstoned_versions, vec![11, 13, 15]);
    assert_eq!(reduction.already_tombstoned_versions, vec![12, 14]);
    assert_eq!(
        reduction.tombstoned_versions.len() + reduction.already_tombstoned_versions.len(),
        inputs.len()
    );
    for (version, _) in inputs {
        let selected_count = reduction
            .tombstoned_versions
            .iter()
            .chain(&reduction.already_tombstoned_versions)
            .filter(|selected| **selected == version)
            .count();
        assert_eq!(selected_count, 1, "version {version} must be selected once");
    }
    assert_eq!(reduction.last_tombstoned_at, Some(seeded_time));
}

// Risk S2 CG HIGH; generated S2 EXHAUSTIVE via S5 LEAN; proposal property and contract A9 source.
#[test]
fn generated_delete_partitions_conserve_unique_versions_and_stable_subsequences() {
    for length in 0..=GENERATED_MAX_LEN {
        for pattern in 0..(1usize << length) {
            let inputs: Vec<_> = (0..length)
                .map(|index| {
                    let status = if pattern & (1usize << index) == 0 {
                        RetirementStatus::Retired
                    } else {
                        RetirementStatus::AlreadyRetired
                    };
                    (10_000 + index as u64, status)
                })
                .collect();
            let expected_retired: Vec<_> = inputs
                .iter()
                .filter_map(|(version, status)| {
                    (*status == RetirementStatus::Retired).then_some(*version)
                })
                .collect();
            let expected_already_retired: Vec<_> = inputs
                .iter()
                .filter_map(|(version, status)| {
                    (*status == RetirementStatus::AlreadyRetired).then_some(*version)
                })
                .collect();
            let mut reduction = DeleteStatusReduction::default();

            for &(version, status) in &inputs {
                partition_delete_version(&mut reduction, version, status);
            }

            assert_eq!(
                reduction.tombstoned_versions, expected_retired,
                "retired subsequence mismatch for length {length}, pattern {pattern:#b}"
            );
            assert_eq!(
                reduction.already_tombstoned_versions, expected_already_retired,
                "already-retired subsequence mismatch for length {length}, pattern {pattern:#b}"
            );
            assert_eq!(
                reduction.tombstoned_versions.len() + reduction.already_tombstoned_versions.len(),
                inputs.len(),
                "cardinality mismatch for length {length}, pattern {pattern:#b}"
            );

            let expected_versions: Vec<_> = inputs.iter().map(|(version, _)| *version).collect();
            let mut selected_versions = reduction.tombstoned_versions.clone();
            selected_versions.extend(&reduction.already_tombstoned_versions);
            selected_versions.sort_unstable();
            assert_eq!(
                selected_versions, expected_versions,
                "loss or duplication for length {length}, pattern {pattern:#b}"
            );
        }
    }
}

// Risk S2/S3 last Store time; level S2/S3 EXHAUSTIVE via S5 LEAN; proposal replacement source.
#[test]
fn last_delete_timestamp_is_replaced_by_each_exact_supplied_value() {
    let first = fixed_time("2024-02-03T04:05:06.123456789Z");
    let second = fixed_time("2025-06-07T08:09:10.987654321Z");
    let mut reduction = DeleteStatusReduction::default();

    assert_eq!(reduction.last_tombstoned_at, None);
    project_last_delete_tombstoned_at(&mut reduction, first);
    assert_eq!(reduction.last_tombstoned_at, Some(first));
    project_last_delete_tombstoned_at(&mut reduction, second);
    assert_eq!(reduction.last_tombstoned_at, Some(second));
}

// Risk S2/S3 and CQ-C2-F01; level S2/S3 EXHAUSTIVE via S5 LEAN; proposal split/assumption A4.
#[test]
fn already_retired_delete_partition_precedes_separate_timestamp_projection() {
    let prior = fixed_time("2023-03-04T05:06:07.111111111Z");
    let replacement = fixed_time("2026-07-08T09:10:11.222222222Z");
    let mut reduction = DeleteStatusReduction::default();
    project_last_delete_tombstoned_at(&mut reduction, prior);

    partition_delete_version(&mut reduction, 77, RetirementStatus::AlreadyRetired);

    assert!(reduction.tombstoned_versions.is_empty());
    assert_eq!(reduction.already_tombstoned_versions, vec![77]);
    assert_eq!(reduction.last_tombstoned_at, Some(prior));

    project_last_delete_tombstoned_at(&mut reduction, replacement);
    assert_eq!(reduction.last_tombstoned_at, Some(replacement));
}

// Risk S2 CG HIGH; level S2 EXHAUSTIVE via S5 LEAN; proposal test-intent/contract default source.
#[test]
fn empty_gc_reduction_defaults_both_vectors() {
    let reduction = GcStatusReduction::default();

    assert!(reduction.tombstoned_rows.is_empty());
    assert!(reduction.already_tombstoned_rows.is_empty());
}

// Risk S2/S4 stable GC routing; level S2/S4 EXHAUSTIVE via S5 LEAN; proposal partition source.
#[test]
fn mixed_gc_partition_is_stable_and_exactly_once() {
    let inputs = [
        (
            fixed_address("00000000-0000-4000-8000-000000000001", "alpha"),
            RetirementStatus::AlreadyRetired,
        ),
        (
            fixed_address("00000000-0000-4000-8000-000000000002", "beta"),
            RetirementStatus::Retired,
        ),
        (
            fixed_address("00000000-0000-4000-8000-000000000003", "gamma"),
            RetirementStatus::AlreadyRetired,
        ),
        (
            fixed_address("00000000-0000-4000-8000-000000000004", "delta"),
            RetirementStatus::Retired,
        ),
    ];
    let mut reduction = GcStatusReduction::default();

    for (address, status) in inputs.iter().cloned() {
        partition_gc_address(&mut reduction, address, status);
    }

    assert_eq!(
        reduction.tombstoned_rows,
        vec![inputs[1].0.clone(), inputs[3].0.clone()]
    );
    assert_eq!(
        reduction.already_tombstoned_rows,
        vec![inputs[0].0.clone(), inputs[2].0.clone()]
    );
    assert_eq!(
        reduction.tombstoned_rows.len() + reduction.already_tombstoned_rows.len(),
        inputs.len()
    );
    for (address, _) in &inputs {
        let selected_count = reduction
            .tombstoned_rows
            .iter()
            .chain(&reduction.already_tombstoned_rows)
            .filter(|selected| *selected == address)
            .count();
        assert_eq!(selected_count, 1, "address must be selected once");
    }
}

// Risk S2 CG HIGH duplicates; generated S2 EXHAUSTIVE via S5 LEAN; proposal property/contract A9.
#[test]
fn generated_gc_partitions_conserve_order_and_repeated_equal_addresses() {
    let first = fixed_address("00000000-0000-4000-8000-000000000011", "repeat-a");
    let second = fixed_address("00000000-0000-4000-8000-000000000012", "repeat-b");

    for length in 0..=GENERATED_MAX_LEN {
        for pattern in 0..(1usize << length) {
            let inputs: Vec<_> = (0..length)
                .map(|index| {
                    let address = if index % 2 == 0 {
                        first.clone()
                    } else {
                        second.clone()
                    };
                    let status = if pattern & (1usize << index) == 0 {
                        RetirementStatus::Retired
                    } else {
                        RetirementStatus::AlreadyRetired
                    };
                    (address, status)
                })
                .collect();
            if length >= 3 {
                assert_eq!(inputs[0].0, inputs[2].0, "generated input must repeat");
            }
            let expected_retired: Vec<_> = inputs
                .iter()
                .filter_map(|(address, status)| match status {
                    RetirementStatus::Retired => Some(address.clone()),
                    RetirementStatus::AlreadyRetired => None,
                })
                .collect();
            let expected_already_retired: Vec<_> = inputs
                .iter()
                .filter_map(|(address, status)| match status {
                    RetirementStatus::Retired => None,
                    RetirementStatus::AlreadyRetired => Some(address.clone()),
                })
                .collect();
            let mut reduction = GcStatusReduction::default();

            for (address, status) in inputs.iter().cloned() {
                partition_gc_address(&mut reduction, address, status);
            }

            assert_eq!(
                reduction.tombstoned_rows, expected_retired,
                "retired subsequence mismatch for length {length}, pattern {pattern:#b}"
            );
            assert_eq!(
                reduction.already_tombstoned_rows, expected_already_retired,
                "already-retired subsequence mismatch for length {length}, pattern {pattern:#b}"
            );
            assert_eq!(
                reduction.tombstoned_rows.len() + reduction.already_tombstoned_rows.len(),
                inputs.len(),
                "cardinality mismatch for length {length}, pattern {pattern:#b}"
            );
            for repeated in [&first, &second] {
                let input_count = inputs
                    .iter()
                    .filter(|(address, _)| address == repeated)
                    .count();
                let selected_count = reduction
                    .tombstoned_rows
                    .iter()
                    .chain(&reduction.already_tombstoned_rows)
                    .filter(|address| *address == repeated)
                    .count();
                assert_eq!(
                    selected_count, input_count,
                    "duplicate count mismatch for length {length}, pattern {pattern:#b}"
                );
            }
        }
    }
}

// Risk S2/S4 complete dry-run mapping; level S2/S4 EXHAUSTIVE via S5 LEAN; proposal mapper source.
#[test]
fn gc_dry_run_mapping_preserves_complete_ordered_duplicate_inputs() {
    let empty = map_gc_dry_run_addresses(Vec::new());
    assert!(empty.tombstoned_rows.is_empty());
    assert!(empty.already_tombstoned_rows.is_empty());

    let first = fixed_address("00000000-0000-4000-8000-000000000021", "dry-a");
    let second = fixed_address("00000000-0000-4000-8000-000000000022", "dry-b");
    let addresses = vec![first.clone(), second, first];
    let expected = addresses.clone();
    let mapped = map_gc_dry_run_addresses(addresses);

    assert_eq!(mapped.tombstoned_rows, expected);
    assert!(mapped.already_tombstoned_rows.is_empty());
}

// Risk S2/S3/S4 mapper consolidation; level S2/S3/S4 EXHAUSTIVE via S5 LEAN; proposal/A2.
#[test]
fn store_status_translation_maps_both_variants_exactly() {
    assert_eq!(
        map_store_retirement_status(&TombstoneStatus::Tombstoned),
        RetirementStatus::Retired
    );
    assert_eq!(
        map_store_retirement_status(&TombstoneStatus::AlreadyTombstoned),
        RetirementStatus::AlreadyRetired
    );
}
