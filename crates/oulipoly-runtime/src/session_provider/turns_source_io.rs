//! Native read accounting. The opt-in warning is a resource declaration only:
//! it grants neither delivery authority nor permission to decode opaque cursors.
use super::{ProviderReadPageResult, SessionProviderError, SessionProviderReadPageRequest};
use super::{SessionProviderTurnProjection, page_error};

const DECLARATION: &str = "codex_observation_io_v1";
const RECONSTRUCTION_CEILING: u64 = 8_388_608;

pub(super) fn validate_source_io(
    result: &ProviderReadPageResult,
    request: &SessionProviderReadPageRequest<'_>,
) -> Result<(), SessionProviderError> {
    validate_accounting(
        &result.warnings,
        request.projection,
        request.max_source_bytes,
        result.source_bytes_examined,
    )
}

fn validate_accounting(
    warnings: &[String],
    projection: SessionProviderTurnProjection,
    quantum: u64,
    total: u64,
) -> Result<(), SessionProviderError> {
    let mut declarations = warnings
        .iter()
        .filter(|value| value.starts_with(DECLARATION));
    let Some(declaration) = declarations.next() else {
        // Providers not declaring reconstruction retain the original total quota.
        return validate_ordinary_quota(total, quantum);
    };
    if declarations.next().is_some() || projection != SessionProviderTurnProjection::UserObservation
    {
        return Err(accounting_error());
    }
    let io = parse_accounting(declaration).ok_or_else(accounting_error)?;
    validate_declared_quota(io, quantum, total)
}

fn validate_ordinary_quota(total: u64, quantum: u64) -> Result<(), SessionProviderError> {
    if total > quantum {
        return Err(page_error("provider_page_source_budget_exceeded"));
    }
    Ok(())
}

struct NativeIo {
    forward: u64,
    reconstruction: u64,
    metadata: u64,
}

fn parse_accounting(declaration: &str) -> Option<NativeIo> {
    let fields = declaration
        .strip_prefix(DECLARATION)?
        .strip_prefix(":forward=")?;
    let (forward, fields) = fields.split_once(";reconstruction=")?;
    let (reconstruction, metadata) = fields.split_once(";metadata=")?;
    Some(NativeIo {
        forward: decimal(forward)?,
        reconstruction: decimal(reconstruction)?,
        metadata: decimal(metadata)?,
    })
}

fn decimal(value: &str) -> Option<u64> {
    if value.is_empty() || value.len() > 20 || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    value.parse().ok()
}

fn validate_declared_quota(
    io: NativeIo,
    quantum: u64,
    total: u64,
) -> Result<(), SessionProviderError> {
    let forward_and_metadata = io
        .forward
        .checked_add(io.metadata)
        .ok_or_else(accounting_error)?;
    let checked_total = forward_and_metadata
        .checked_add(io.reconstruction)
        .ok_or_else(accounting_error)?;
    if forward_and_metadata > quantum
        || io.reconstruction >= RECONSTRUCTION_CEILING
        || checked_total != total
    {
        return Err(accounting_error());
    }
    Ok(())
}

fn accounting_error() -> SessionProviderError {
    page_error("provider_page_observation_io_invalid")
}

#[cfg(test)]
mod tests {
    use super::*;
    use SessionProviderTurnProjection::{CanonicalIngest, UserObservation};

    fn warning(fields: &str) -> String {
        format!("{DECLARATION}:{fields}")
    }

    #[test]
    fn age347_resource_declaration_is_observation_only_and_not_provider_routing() {
        let valid = warning("forward=100;reconstruction=8388607;metadata=28");
        assert!(
            validate_accounting(std::slice::from_ref(&valid), UserObservation, 128, 8388735)
                .is_ok()
        );
        assert!(validate_accounting(&[valid], CanonicalIngest, 128, 8388735).is_err());
        assert!(validate_accounting(&[], UserObservation, 128, 129).is_err());
        assert!(validate_accounting(&[], CanonicalIngest, 128, 129).is_err());
        for projection in [CanonicalIngest, UserObservation] {
            assert!(
                validate_accounting(&["ordinary warning".into()], projection, 128, 128).is_ok()
            );
        }
        let zero = warning("forward=0;reconstruction=0;metadata=0");
        assert!(validate_accounting(std::slice::from_ref(&zero), UserObservation, 0, 0).is_ok());
        assert!(validate_accounting(&[zero], CanonicalIngest, 128, 0).is_err());
    }

    #[test]
    fn age347_malformed_missing_duplicate_and_overflow_never_bypass_quota() {
        for fields in [
            "forward=-1;reconstruction=1;metadata=0",
            "forward=+1;reconstruction=0;metadata=0",
            "forward=1.0;reconstruction=0;metadata=0",
            "forward=1e0;reconstruction=0;metadata=0",
            "forward= 1;reconstruction=0;metadata=0",
            "forward=;reconstruction=0;metadata=0",
            "forward=1;reconstruction=0",
            "metadata=0;forward=1;reconstruction=0",
            "forward=1;reconstruction=0;metadata=0;metadata=0",
            "forward=1;reconstruction=0;metadata=0\n",
            "forward=18446744073709551616;reconstruction=0;metadata=0",
            "forward=18446744073709551615;reconstruction=0;metadata=1",
            "forward=18446744073709551615;reconstruction=1;metadata=0",
            "forward=1;reconstruction=8388608;metadata=0",
        ] {
            assert!(
                validate_accounting(&[warning(fields)], UserObservation, u64::MAX, 1).is_err(),
                "{fields}"
            );
        }
        let valid = warning("forward=1;reconstruction=0;metadata=0");
        assert!(
            validate_accounting(&[valid.clone(), valid.clone()], UserObservation, 1, 1).is_err()
        );
        assert!(
            validate_accounting(&[valid.clone(), DECLARATION.into()], UserObservation, 1, 1)
                .is_err()
        );
        assert!(validate_accounting(std::slice::from_ref(&valid), UserObservation, 0, 1).is_err());
        assert!(validate_accounting(&[valid], UserObservation, 1, 0).is_err());
        assert!(
            validate_accounting(
                &[warning("forward=1;reconstruction=0;metadata=1")],
                UserObservation,
                1,
                2
            )
            .is_err()
        );
    }
}
