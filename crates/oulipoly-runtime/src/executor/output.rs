//! Retained execution-output evidence and bounded allocation primitives.
//!
//! ## Declared roles
//!
//! `accessor`, `orchestration`, `validator`, `predicate`

pub const EXECUTION_OUTPUT_RETAINED_PREFIX_MAX_BYTES: usize = 1_048_576;
pub const EXECUTION_OUTPUT_RETAINED_TAIL_MAX_BYTES: usize = 1_048_576;

const EXECUTION_OUTPUT_RETAINED_CAPACITY_MAX_BYTES: u64 = 1_114_112;
const EXECUTION_OUTPUT_RETAINED_FINAL_SLACK_BYTES: u64 = 65_536;

pub struct ExecutionOutputRetained {
    prefix: ExecutionOutputRetainedSide,
    tail: ExecutionOutputRetainedSide,
    observed_length: Option<u64>,
}

pub enum ExecutionOutputRetainedSide {
    Available(ExecutionOutputRetainedBytes),
    Unavailable(ExecutionOutputRetainedUnavailable),
}

pub struct ExecutionOutputRetainedBytes {
    bytes: Vec<u8>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct ExecutionOutputRetainedUnavailable {
    reason: ExecutionOutputRetainedUnavailableReason,
    site: ExecutionOutputRetainedUnavailableSite,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionOutputRetainedUnavailableReason {
    AllocationFailure,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionOutputRetainedUnavailableSite {
    RetainedPrefixWorking,
    RetainedTailWorking,
}

impl ExecutionOutputRetained {
    pub fn prefix(&self) -> &ExecutionOutputRetainedSide {
        &self.prefix
    }

    pub fn tail(&self) -> &ExecutionOutputRetainedSide {
        &self.tail
    }

    pub fn observed_length(&self) -> Option<u64> {
        self.observed_length
    }

    pub fn evidence_complete(&self) -> bool {
        self.prefix.is_available() && self.tail.is_available()
    }

    pub fn truncated(&self) -> bool {
        let retained_limit = u64::try_from(
            EXECUTION_OUTPUT_RETAINED_PREFIX_MAX_BYTES
                .min(EXECUTION_OUTPUT_RETAINED_TAIL_MAX_BYTES),
        )
        .unwrap_or(u64::MAX);
        !self.evidence_complete()
            || self
                .observed_length
                .is_none_or(|observed_length| observed_length > retained_limit)
    }
}

impl ExecutionOutputRetainedSide {
    pub fn is_available(&self) -> bool {
        matches!(self, Self::Available(_))
    }

    pub fn bytes(&self) -> Option<&[u8]> {
        match self {
            Self::Available(bytes) => Some(bytes.as_slice()),
            Self::Unavailable(_) => None,
        }
    }

    pub fn unavailable(&self) -> Option<&ExecutionOutputRetainedUnavailable> {
        match self {
            Self::Available(_) => None,
            Self::Unavailable(unavailable) => Some(unavailable),
        }
    }
}

impl ExecutionOutputRetainedBytes {
    pub fn as_slice(&self) -> &[u8] {
        &self.bytes
    }
}

impl ExecutionOutputRetainedUnavailable {
    pub fn reason(&self) -> ExecutionOutputRetainedUnavailableReason {
        self.reason
    }

    pub fn site(&self) -> ExecutionOutputRetainedUnavailableSite {
        self.site
    }
}

impl ExecutionOutputRetainedUnavailableReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AllocationFailure => "allocation_failure",
        }
    }
}

impl ExecutionOutputRetainedUnavailableSite {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::RetainedPrefixWorking => "retained_prefix_working",
            Self::RetainedTailWorking => "retained_tail_working",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OutputStreamKind {
    Stdout,
    Stderr,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(clippy::enum_variant_names)]
pub(crate) enum OutputAllocationSite {
    RetainedPrefixWorking,
    RetainedTailWorking,
    RetainedPrefixFinal,
    RetainedTailFinal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OutputAllocationFailureKind {
    Injected,
    LogicalLimit,
    ZeroSizedElement,
    ElementSizeOverflow,
    Reserve,
    MeasuredCapacityOverflow,
    MeasuredCapacityLimit,
    GrantMismatch,
    FillLengthOverflow,
    FillLimit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct OutputAllocationFailure {
    stream: OutputStreamKind,
    site: OutputAllocationSite,
    kind: OutputAllocationFailureKind,
    requested_elements: usize,
    logical_max_elements: usize,
    measured_capacity_bytes: Option<u64>,
    measured_capacity_max_bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct OutputAllocationGrant {
    stream: OutputStreamKind,
    site: OutputAllocationSite,
    fill_limit_elements: usize,
    capacity_elements: usize,
    capacity_bytes: u64,
    element_size_bytes: usize,
}

pub(crate) trait OutputAllocationControl: Send + Sync {
    fn injected_failure(&self, stream: OutputStreamKind, site: OutputAllocationSite) -> bool;

    fn reserve_failure(&self, stream: OutputStreamKind, site: OutputAllocationSite) -> bool;

    fn measured_capacity_elements(
        &self,
        stream: OutputStreamKind,
        site: OutputAllocationSite,
        actual_capacity_elements: usize,
    ) -> Option<usize>;
}

pub(crate) struct OutputAllocationController<'a> {
    controls: &'a dyn OutputAllocationControl,
}

pub(crate) struct ExecutionOutputRetainedBuilder {
    stream: OutputStreamKind,
    prefix: ExecutionOutputRetainedWorkingSide,
    tail: ExecutionOutputRetainedWorkingSide,
    tail_start: usize,
    tail_len: usize,
    first_allocation_failure: Option<OutputAllocationFailure>,
}

pub(crate) struct ExecutionOutputRetainedFinalization {
    retained: ExecutionOutputRetained,
    first_allocation_failure: Option<OutputAllocationFailure>,
}

enum ExecutionOutputRetainedWorkingSide {
    Uninitialized,
    Available {
        owner: Vec<u8>,
        grant: OutputAllocationGrant,
    },
    Unavailable(ExecutionOutputRetainedUnavailable),
}

struct NoOutputAllocationControls;

static NO_OUTPUT_ALLOCATION_CONTROLS: NoOutputAllocationControls = NoOutputAllocationControls;

impl OutputAllocationControl for NoOutputAllocationControls {
    fn injected_failure(&self, _stream: OutputStreamKind, _site: OutputAllocationSite) -> bool {
        false
    }

    fn reserve_failure(&self, _stream: OutputStreamKind, _site: OutputAllocationSite) -> bool {
        false
    }

    fn measured_capacity_elements(
        &self,
        _stream: OutputStreamKind,
        _site: OutputAllocationSite,
        _actual_capacity_elements: usize,
    ) -> Option<usize> {
        None
    }
}

impl OutputAllocationController<'static> {
    pub(crate) fn production() -> Self {
        Self {
            controls: &NO_OUTPUT_ALLOCATION_CONTROLS,
        }
    }
}

impl<'a> OutputAllocationController<'a> {
    #[cfg(test)]
    pub(crate) fn controlled(controls: &'a dyn OutputAllocationControl) -> Self {
        Self { controls }
    }

    pub(crate) fn try_vec<T>(
        &mut self,
        stream: OutputStreamKind,
        site: OutputAllocationSite,
        requested_elements: usize,
    ) -> Result<(Vec<T>, OutputAllocationGrant), OutputAllocationFailure> {
        let element_size_bytes = std::mem::size_of::<T>();
        if element_size_bytes == 0 {
            return Err(new_allocation_failure(
                stream,
                site,
                OutputAllocationFailureKind::ZeroSizedElement,
                requested_elements,
                site_logical_max_elements(site),
                None,
                zero_sized_measured_max(site),
            ));
        }

        let logical_max_elements = site_logical_max_elements(site);
        if requested_elements > logical_max_elements {
            return Err(new_allocation_failure(
                stream,
                site,
                OutputAllocationFailureKind::LogicalLimit,
                requested_elements,
                logical_max_elements,
                None,
                EXECUTION_OUTPUT_RETAINED_CAPACITY_MAX_BYTES,
            ));
        }

        let requested_bytes = checked_element_bytes(requested_elements, element_size_bytes)
            .ok_or_else(|| {
                new_allocation_failure(
                    stream,
                    site,
                    OutputAllocationFailureKind::ElementSizeOverflow,
                    requested_elements,
                    logical_max_elements,
                    None,
                    EXECUTION_OUTPUT_RETAINED_CAPACITY_MAX_BYTES,
                )
            })?;
        let measured_capacity_max_bytes = site_measured_capacity_max_bytes(site, requested_bytes);

        if self.controls.injected_failure(stream, site) {
            return Err(new_allocation_failure(
                stream,
                site,
                OutputAllocationFailureKind::Injected,
                requested_elements,
                logical_max_elements,
                None,
                measured_capacity_max_bytes,
            ));
        }

        let mut owner = Vec::new();
        if requested_elements == 0 {
            return Ok((
                owner,
                OutputAllocationGrant {
                    stream,
                    site,
                    fill_limit_elements: 0,
                    capacity_elements: 0,
                    capacity_bytes: 0,
                    element_size_bytes,
                },
            ));
        }

        if self.controls.reserve_failure(stream, site)
            || owner.try_reserve_exact(requested_elements).is_err()
        {
            return Err(new_allocation_failure(
                stream,
                site,
                OutputAllocationFailureKind::Reserve,
                requested_elements,
                logical_max_elements,
                None,
                measured_capacity_max_bytes,
            ));
        }

        let actual_capacity_elements = owner.capacity();
        let controlled_capacity_elements =
            self.controls
                .measured_capacity_elements(stream, site, actual_capacity_elements);
        let measured_capacity_elements =
            controlled_capacity_elements.unwrap_or(actual_capacity_elements);
        let measured_capacity_bytes =
            checked_element_bytes(measured_capacity_elements, element_size_bytes).ok_or_else(
                || {
                    new_allocation_failure(
                        stream,
                        site,
                        OutputAllocationFailureKind::MeasuredCapacityOverflow,
                        requested_elements,
                        logical_max_elements,
                        None,
                        measured_capacity_max_bytes,
                    )
                },
            )?;
        if measured_capacity_bytes > measured_capacity_max_bytes {
            return Err(new_allocation_failure(
                stream,
                site,
                OutputAllocationFailureKind::MeasuredCapacityLimit,
                requested_elements,
                logical_max_elements,
                Some(measured_capacity_bytes),
                measured_capacity_max_bytes,
            ));
        }
        if controlled_capacity_elements.is_some() {
            return Err(new_allocation_failure(
                stream,
                site,
                OutputAllocationFailureKind::GrantMismatch,
                requested_elements,
                logical_max_elements,
                Some(measured_capacity_bytes),
                measured_capacity_max_bytes,
            ));
        }

        Ok((
            owner,
            OutputAllocationGrant {
                stream,
                site,
                fill_limit_elements: requested_elements,
                capacity_elements: actual_capacity_elements,
                capacity_bytes: measured_capacity_bytes,
                element_size_bytes,
            },
        ))
    }

    pub(crate) fn push_granted<T>(
        &mut self,
        stream: OutputStreamKind,
        site: OutputAllocationSite,
        grant: &OutputAllocationGrant,
        owner: &mut Vec<T>,
        value: T,
    ) -> Result<(), OutputAllocationFailure> {
        validate_grant(
            stream,
            site,
            grant,
            owner.capacity(),
            std::mem::size_of::<T>(),
        )?;
        checked_fill_length(stream, site, grant, owner.len(), 1)?;
        owner.push(value);
        Ok(())
    }

    pub(crate) fn extend_granted_bytes(
        &mut self,
        stream: OutputStreamKind,
        site: OutputAllocationSite,
        grant: &OutputAllocationGrant,
        owner: &mut Vec<u8>,
        bytes: &[u8],
    ) -> Result<(), OutputAllocationFailure> {
        validate_grant(
            stream,
            site,
            grant,
            owner.capacity(),
            std::mem::size_of::<u8>(),
        )?;
        checked_fill_length(stream, site, grant, owner.len(), bytes.len())?;
        owner.extend_from_slice(bytes);
        Ok(())
    }
}

impl OutputAllocationFailure {
    pub(crate) fn stream(&self) -> OutputStreamKind {
        self.stream
    }

    pub(crate) fn site(&self) -> OutputAllocationSite {
        self.site
    }

    pub(crate) fn kind(&self) -> OutputAllocationFailureKind {
        self.kind
    }

    pub(crate) fn requested_elements(&self) -> usize {
        self.requested_elements
    }

    pub(crate) fn logical_max_elements(&self) -> usize {
        self.logical_max_elements
    }

    pub(crate) fn measured_capacity_bytes(&self) -> Option<u64> {
        self.measured_capacity_bytes
    }

    pub(crate) fn measured_capacity_max_bytes(&self) -> u64 {
        self.measured_capacity_max_bytes
    }
}

impl OutputAllocationGrant {
    pub(crate) fn stream(&self) -> OutputStreamKind {
        self.stream
    }

    pub(crate) fn site(&self) -> OutputAllocationSite {
        self.site
    }

    pub(crate) fn fill_limit_elements(&self) -> usize {
        self.fill_limit_elements
    }

    pub(crate) fn capacity_elements(&self) -> usize {
        self.capacity_elements
    }

    pub(crate) fn capacity_bytes(&self) -> u64 {
        self.capacity_bytes
    }
}

impl ExecutionOutputRetainedBuilder {
    pub(crate) fn new(stream: OutputStreamKind) -> Self {
        Self {
            stream,
            prefix: ExecutionOutputRetainedWorkingSide::Uninitialized,
            tail: ExecutionOutputRetainedWorkingSide::Uninitialized,
            tail_start: 0,
            tail_len: 0,
            first_allocation_failure: None,
        }
    }

    pub(crate) fn observe(
        &mut self,
        bytes: &[u8],
        allocations: &mut OutputAllocationController<'_>,
    ) {
        if bytes.is_empty() {
            return;
        }

        if matches!(
            self.prefix,
            ExecutionOutputRetainedWorkingSide::Uninitialized
        ) {
            let (side, failure) = initialize_working_side(
                self.stream,
                OutputAllocationSite::RetainedPrefixWorking,
                allocations,
            );
            self.prefix = side;
            retain_first_failure(&mut self.first_allocation_failure, failure);
        }
        if matches!(self.tail, ExecutionOutputRetainedWorkingSide::Uninitialized) {
            let (side, failure) = initialize_working_side(
                self.stream,
                OutputAllocationSite::RetainedTailWorking,
                allocations,
            );
            self.tail = side;
            retain_first_failure(&mut self.first_allocation_failure, failure);
        }

        let prefix_failure = match &mut self.prefix {
            ExecutionOutputRetainedWorkingSide::Available { owner, grant } => {
                let remaining = EXECUTION_OUTPUT_RETAINED_PREFIX_MAX_BYTES
                    .checked_sub(owner.len())
                    .expect("prefix owner length stays within its grant");
                let copy_len = remaining.min(bytes.len());
                (copy_len != 0)
                    .then(|| {
                        allocations.extend_granted_bytes(
                            self.stream,
                            OutputAllocationSite::RetainedPrefixWorking,
                            grant,
                            owner,
                            &bytes[..copy_len],
                        )
                    })
                    .transpose()
                    .err()
            }
            ExecutionOutputRetainedWorkingSide::Uninitialized
            | ExecutionOutputRetainedWorkingSide::Unavailable(_) => None,
        };
        if let Some(failure) = prefix_failure {
            self.prefix = unavailable_working_side(OutputAllocationSite::RetainedPrefixWorking);
            retain_first_failure(&mut self.first_allocation_failure, Some(failure));
        }

        let mut tail_failure = None;
        let mut consumed = 0;
        if let ExecutionOutputRetainedWorkingSide::Available { owner, grant } = &mut self.tail {
            if self.tail_len < EXECUTION_OUTPUT_RETAINED_TAIL_MAX_BYTES {
                let remaining = EXECUTION_OUTPUT_RETAINED_TAIL_MAX_BYTES
                    .checked_sub(self.tail_len)
                    .expect("tail length stays within its grant");
                let append_len = remaining.min(bytes.len());
                tail_failure = allocations
                    .extend_granted_bytes(
                        self.stream,
                        OutputAllocationSite::RetainedTailWorking,
                        grant,
                        owner,
                        &bytes[..append_len],
                    )
                    .err();
                if tail_failure.is_none() {
                    self.tail_len = owner.len();
                    consumed = append_len;
                }
            }

            if tail_failure.is_none() {
                for byte in &bytes[consumed..] {
                    owner[self.tail_start] = *byte;
                    match checked_tail_start_increment(self.tail_start) {
                        Some(next) => self.tail_start = next,
                        None => {
                            tail_failure = Some(new_allocation_failure(
                                self.stream,
                                OutputAllocationSite::RetainedTailWorking,
                                OutputAllocationFailureKind::FillLengthOverflow,
                                self.tail_start,
                                EXECUTION_OUTPUT_RETAINED_TAIL_MAX_BYTES,
                                Some(grant.capacity_bytes),
                                EXECUTION_OUTPUT_RETAINED_CAPACITY_MAX_BYTES,
                            ));
                            break;
                        }
                    }
                }
            }
        }
        if let Some(failure) = tail_failure {
            self.tail = unavailable_working_side(OutputAllocationSite::RetainedTailWorking);
            self.tail_start = 0;
            self.tail_len = 0;
            retain_first_failure(&mut self.first_allocation_failure, Some(failure));
        }
    }

    pub(crate) fn first_allocation_failure(&self) -> Option<OutputAllocationFailure> {
        self.first_allocation_failure
    }

    pub(crate) fn finish(
        self,
        observed_length: Option<u64>,
        allocations: &mut OutputAllocationController<'_>,
    ) -> ExecutionOutputRetainedFinalization {
        let Self {
            stream,
            prefix,
            mut tail,
            mut tail_start,
            tail_len,
            mut first_allocation_failure,
        } = self;
        let prefix = finalize_working_side(
            prefix,
            stream,
            OutputAllocationSite::RetainedPrefixFinal,
            allocations,
            &mut first_allocation_failure,
        );
        match &mut tail {
            ExecutionOutputRetainedWorkingSide::Available { owner, .. } if tail_len != 0 => {
                owner[..tail_len].rotate_left(tail_start);
                tail_start = 0;
            }
            ExecutionOutputRetainedWorkingSide::Uninitialized
            | ExecutionOutputRetainedWorkingSide::Available { .. }
            | ExecutionOutputRetainedWorkingSide::Unavailable(_) => {}
        }
        debug_assert_eq!(tail_start, 0);
        let tail = finalize_working_side(
            tail,
            stream,
            OutputAllocationSite::RetainedTailFinal,
            allocations,
            &mut first_allocation_failure,
        );
        ExecutionOutputRetainedFinalization {
            retained: ExecutionOutputRetained {
                prefix,
                tail,
                observed_length,
            },
            first_allocation_failure,
        }
    }
}

impl ExecutionOutputRetainedFinalization {
    pub(crate) fn into_parts(self) -> (ExecutionOutputRetained, Option<OutputAllocationFailure>) {
        (self.retained, self.first_allocation_failure)
    }
}

fn site_logical_max_elements(_site: OutputAllocationSite) -> usize {
    EXECUTION_OUTPUT_RETAINED_PREFIX_MAX_BYTES
}

fn zero_sized_measured_max(site: OutputAllocationSite) -> u64 {
    match site {
        OutputAllocationSite::RetainedPrefixWorking | OutputAllocationSite::RetainedTailWorking => {
            EXECUTION_OUTPUT_RETAINED_CAPACITY_MAX_BYTES
        }
        OutputAllocationSite::RetainedPrefixFinal | OutputAllocationSite::RetainedTailFinal => {
            EXECUTION_OUTPUT_RETAINED_FINAL_SLACK_BYTES
        }
    }
}

fn site_measured_capacity_max_bytes(site: OutputAllocationSite, requested_bytes: u64) -> u64 {
    match site {
        OutputAllocationSite::RetainedPrefixWorking | OutputAllocationSite::RetainedTailWorking => {
            EXECUTION_OUTPUT_RETAINED_CAPACITY_MAX_BYTES
        }
        OutputAllocationSite::RetainedPrefixFinal | OutputAllocationSite::RetainedTailFinal => {
            requested_bytes
                .checked_add(EXECUTION_OUTPUT_RETAINED_FINAL_SLACK_BYTES)
                .unwrap_or(EXECUTION_OUTPUT_RETAINED_CAPACITY_MAX_BYTES)
                .min(EXECUTION_OUTPUT_RETAINED_CAPACITY_MAX_BYTES)
        }
    }
}

fn checked_element_bytes(elements: usize, element_size_bytes: usize) -> Option<u64> {
    elements
        .checked_mul(element_size_bytes)
        .and_then(|bytes| u64::try_from(bytes).ok())
}

fn new_allocation_failure(
    stream: OutputStreamKind,
    site: OutputAllocationSite,
    kind: OutputAllocationFailureKind,
    requested_elements: usize,
    logical_max_elements: usize,
    measured_capacity_bytes: Option<u64>,
    measured_capacity_max_bytes: u64,
) -> OutputAllocationFailure {
    OutputAllocationFailure {
        stream,
        site,
        kind,
        requested_elements,
        logical_max_elements,
        measured_capacity_bytes,
        measured_capacity_max_bytes,
    }
}

fn validate_grant(
    stream: OutputStreamKind,
    site: OutputAllocationSite,
    grant: &OutputAllocationGrant,
    owner_capacity_elements: usize,
    element_size_bytes: usize,
) -> Result<(), OutputAllocationFailure> {
    let measured_capacity_bytes =
        checked_element_bytes(owner_capacity_elements, element_size_bytes);
    let measured_capacity_max_bytes =
        checked_element_bytes(grant.fill_limit_elements, element_size_bytes)
            .map(|requested_bytes| site_measured_capacity_max_bytes(site, requested_bytes))
            .unwrap_or(EXECUTION_OUTPUT_RETAINED_CAPACITY_MAX_BYTES);
    let matches = grant.stream == stream
        && grant.site == site
        && grant.element_size_bytes == element_size_bytes
        && grant.capacity_elements == owner_capacity_elements
        && measured_capacity_bytes == Some(grant.capacity_bytes)
        && grant.fill_limit_elements <= grant.capacity_elements
        && grant.fill_limit_elements <= site_logical_max_elements(site)
        && grant.capacity_bytes <= measured_capacity_max_bytes;
    if matches {
        Ok(())
    } else {
        Err(new_allocation_failure(
            stream,
            site,
            OutputAllocationFailureKind::GrantMismatch,
            grant.fill_limit_elements,
            site_logical_max_elements(site),
            measured_capacity_bytes,
            measured_capacity_max_bytes,
        ))
    }
}

fn checked_fill_length(
    stream: OutputStreamKind,
    site: OutputAllocationSite,
    grant: &OutputAllocationGrant,
    current_elements: usize,
    additional_elements: usize,
) -> Result<usize, OutputAllocationFailure> {
    let measured_capacity_max_bytes =
        checked_element_bytes(grant.fill_limit_elements, grant.element_size_bytes)
            .map(|requested_bytes| site_measured_capacity_max_bytes(site, requested_bytes))
            .unwrap_or(EXECUTION_OUTPUT_RETAINED_CAPACITY_MAX_BYTES);
    let requested_elements = current_elements
        .checked_add(additional_elements)
        .ok_or_else(|| {
            new_allocation_failure(
                stream,
                site,
                OutputAllocationFailureKind::FillLengthOverflow,
                additional_elements,
                site_logical_max_elements(site),
                Some(grant.capacity_bytes),
                measured_capacity_max_bytes,
            )
        })?;
    if requested_elements > grant.fill_limit_elements
        || requested_elements > grant.capacity_elements
    {
        return Err(new_allocation_failure(
            stream,
            site,
            OutputAllocationFailureKind::FillLimit,
            requested_elements,
            site_logical_max_elements(site),
            Some(grant.capacity_bytes),
            measured_capacity_max_bytes,
        ));
    }
    Ok(requested_elements)
}

fn initialize_working_side(
    stream: OutputStreamKind,
    site: OutputAllocationSite,
    allocations: &mut OutputAllocationController<'_>,
) -> (
    ExecutionOutputRetainedWorkingSide,
    Option<OutputAllocationFailure>,
) {
    match allocations.try_vec::<u8>(stream, site, site_logical_max_elements(site)) {
        Ok((owner, grant)) => (
            ExecutionOutputRetainedWorkingSide::Available { owner, grant },
            None,
        ),
        Err(failure) => (unavailable_working_side(site), Some(failure)),
    }
}

fn unavailable_working_side(site: OutputAllocationSite) -> ExecutionOutputRetainedWorkingSide {
    let site = match site {
        OutputAllocationSite::RetainedPrefixWorking => {
            ExecutionOutputRetainedUnavailableSite::RetainedPrefixWorking
        }
        OutputAllocationSite::RetainedTailWorking => {
            ExecutionOutputRetainedUnavailableSite::RetainedTailWorking
        }
        OutputAllocationSite::RetainedPrefixFinal | OutputAllocationSite::RetainedTailFinal => {
            unreachable!("final allocation sites do not make retained evidence unavailable")
        }
    };
    ExecutionOutputRetainedWorkingSide::Unavailable(ExecutionOutputRetainedUnavailable {
        reason: ExecutionOutputRetainedUnavailableReason::AllocationFailure,
        site,
    })
}

fn finalize_working_side(
    side: ExecutionOutputRetainedWorkingSide,
    stream: OutputStreamKind,
    final_site: OutputAllocationSite,
    allocations: &mut OutputAllocationController<'_>,
    first_allocation_failure: &mut Option<OutputAllocationFailure>,
) -> ExecutionOutputRetainedSide {
    match side {
        ExecutionOutputRetainedWorkingSide::Uninitialized => {
            ExecutionOutputRetainedSide::Available(ExecutionOutputRetainedBytes {
                bytes: Vec::new(),
            })
        }
        ExecutionOutputRetainedWorkingSide::Unavailable(unavailable) => {
            ExecutionOutputRetainedSide::Unavailable(unavailable)
        }
        ExecutionOutputRetainedWorkingSide::Available { owner, .. } if owner.is_empty() => {
            ExecutionOutputRetainedSide::Available(ExecutionOutputRetainedBytes { bytes: owner })
        }
        ExecutionOutputRetainedWorkingSide::Available { owner, .. } => {
            match allocations.try_vec::<u8>(stream, final_site, owner.len()) {
                Ok((mut final_owner, grant)) => match allocations.extend_granted_bytes(
                    stream,
                    final_site,
                    &grant,
                    &mut final_owner,
                    &owner,
                ) {
                    Ok(()) => {
                        ExecutionOutputRetainedSide::Available(ExecutionOutputRetainedBytes {
                            bytes: final_owner,
                        })
                    }
                    Err(failure) => {
                        retain_first_failure(first_allocation_failure, Some(failure));
                        ExecutionOutputRetainedSide::Available(ExecutionOutputRetainedBytes {
                            bytes: owner,
                        })
                    }
                },
                Err(failure) => {
                    retain_first_failure(first_allocation_failure, Some(failure));
                    ExecutionOutputRetainedSide::Available(ExecutionOutputRetainedBytes {
                        bytes: owner,
                    })
                }
            }
        }
    }
}

fn retain_first_failure(
    first: &mut Option<OutputAllocationFailure>,
    candidate: Option<OutputAllocationFailure>,
) {
    if first.is_none() {
        *first = candidate;
    }
}

fn checked_tail_start_increment(tail_start: usize) -> Option<usize> {
    tail_start.checked_add(1).map(|next| {
        if next == EXECUTION_OUTPUT_RETAINED_TAIL_MAX_BYTES {
            0
        } else {
            next
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};
    use std::sync::atomic::{AtomicUsize, Ordering};

    const L: usize = 1_048_576;
    const SHORT: &[u8] = b"fault-prefix";

    struct FixedOutputAllocationControls {
        injected: [u8; 2],
        reserve: [u8; 2],
        measured: [[Option<usize>; 4]; 2],
        injected_calls: [[AtomicUsize; 4]; 2],
        reserve_calls: [[AtomicUsize; 4]; 2],
        measured_calls: [[AtomicUsize; 4]; 2],
    }

    impl FixedOutputAllocationControls {
        fn new() -> Self {
            Self {
                injected: [0; 2],
                reserve: [0; 2],
                measured: [[None; 4]; 2],
                injected_calls: atomic_call_matrix(),
                reserve_calls: atomic_call_matrix(),
                measured_calls: atomic_call_matrix(),
            }
        }

        fn inject(mut self, stream: OutputStreamKind, site: OutputAllocationSite) -> Self {
            self.injected[stream_index(stream)] |= site_bit(site);
            self
        }

        fn fail_reserve(mut self, stream: OutputStreamKind, site: OutputAllocationSite) -> Self {
            self.reserve[stream_index(stream)] |= site_bit(site);
            self
        }

        fn measure(
            mut self,
            stream: OutputStreamKind,
            site: OutputAllocationSite,
            capacity: usize,
        ) -> Self {
            self.measured[stream_index(stream)][site_index(site)] = Some(capacity);
            self
        }

        fn total_calls(&self) -> usize {
            [
                &self.injected_calls,
                &self.reserve_calls,
                &self.measured_calls,
            ]
            .into_iter()
            .flat_map(|matrix| matrix.iter())
            .flat_map(|row| row.iter())
            .map(|count| count.load(Ordering::Relaxed))
            .sum()
        }

        fn calls(
            &self,
            family: ControlFamily,
            stream: OutputStreamKind,
            site: OutputAllocationSite,
        ) -> usize {
            let matrix = match family {
                ControlFamily::Injected => &self.injected_calls,
                ControlFamily::Reserve => &self.reserve_calls,
                ControlFamily::Measured => &self.measured_calls,
            };
            matrix[stream_index(stream)][site_index(site)].load(Ordering::Relaxed)
        }
    }

    impl OutputAllocationControl for FixedOutputAllocationControls {
        fn injected_failure(&self, stream: OutputStreamKind, site: OutputAllocationSite) -> bool {
            self.injected_calls[stream_index(stream)][site_index(site)]
                .fetch_add(1, Ordering::Relaxed);
            self.injected[stream_index(stream)] & site_bit(site) != 0
        }

        fn reserve_failure(&self, stream: OutputStreamKind, site: OutputAllocationSite) -> bool {
            self.reserve_calls[stream_index(stream)][site_index(site)]
                .fetch_add(1, Ordering::Relaxed);
            self.reserve[stream_index(stream)] & site_bit(site) != 0
        }

        fn measured_capacity_elements(
            &self,
            stream: OutputStreamKind,
            site: OutputAllocationSite,
            _actual_capacity_elements: usize,
        ) -> Option<usize> {
            self.measured_calls[stream_index(stream)][site_index(site)]
                .fetch_add(1, Ordering::Relaxed);
            self.measured[stream_index(stream)][site_index(site)]
        }
    }

    #[derive(Clone, Copy)]
    enum ControlFamily {
        Injected,
        Reserve,
        Measured,
    }

    fn atomic_call_matrix() -> [[AtomicUsize; 4]; 2] {
        [
            [
                AtomicUsize::new(0),
                AtomicUsize::new(0),
                AtomicUsize::new(0),
                AtomicUsize::new(0),
            ],
            [
                AtomicUsize::new(0),
                AtomicUsize::new(0),
                AtomicUsize::new(0),
                AtomicUsize::new(0),
            ],
        ]
    }

    fn stream_index(stream: OutputStreamKind) -> usize {
        match stream {
            OutputStreamKind::Stdout => 0,
            OutputStreamKind::Stderr => 1,
        }
    }

    fn site_index(site: OutputAllocationSite) -> usize {
        match site {
            OutputAllocationSite::RetainedPrefixWorking => 0,
            OutputAllocationSite::RetainedTailWorking => 1,
            OutputAllocationSite::RetainedPrefixFinal => 2,
            OutputAllocationSite::RetainedTailFinal => 3,
        }
    }

    fn site_bit(site: OutputAllocationSite) -> u8 {
        1u8 << site_index(site)
    }

    fn run_case(function: &str, case: &str, executed: &mut usize, assertions: impl FnOnce()) {
        assertions();
        println!("\nCASE {function}::{case}");
        *executed = executed.checked_add(1).expect("case count fits usize");
    }

    fn finish_cases(function: &str, expected: usize, executed: usize) {
        println!("CASE_COUNT {function} expected={expected} actual={executed}");
        assert_eq!(executed, expected);
    }

    fn digest(bytes: &[u8]) -> String {
        format!("{:x}", Sha256::digest(bytes))
    }

    fn assert_fixture(bytes: &[u8], expected_len: usize, expected_digest: &str) {
        assert_eq!(bytes.len(), expected_len);
        assert_eq!(digest(bytes), expected_digest);
    }

    fn one_over_full() -> Vec<u8> {
        let mut bytes = vec![b'A'; L];
        bytes.push(b'B');
        bytes
    }

    fn one_over_tail() -> Vec<u8> {
        let mut bytes = vec![b'A'; L - 1];
        bytes.push(b'B');
        bytes
    }

    fn one_over_physical() -> Vec<u8> {
        let mut bytes = vec![b'B'];
        bytes.extend_from_slice(&vec![b'A'; L - 1]);
        bytes
    }

    fn multiple_wrap_full() -> Vec<u8> {
        let mut bytes = vec![b'A'; L];
        bytes.extend_from_slice(&vec![b'B'; L]);
        bytes.extend_from_slice(b"CDEF");
        bytes
    }

    fn multiple_wrap_tail() -> Vec<u8> {
        let mut bytes = vec![b'B'; L - 4];
        bytes.extend_from_slice(b"CDEF");
        bytes
    }

    fn multiple_wrap_physical() -> Vec<u8> {
        let mut bytes = b"CDEF".to_vec();
        bytes.extend_from_slice(&vec![b'B'; L - 4]);
        bytes
    }

    fn wrapped_nonuniform_full() -> Vec<u8> {
        let mut bytes = vec![b'A'; L];
        bytes.extend_from_slice(b"BCDE");
        bytes
    }

    fn wrapped_nonuniform_tail() -> Vec<u8> {
        let mut bytes = vec![b'A'; L - 4];
        bytes.extend_from_slice(b"BCDE");
        bytes
    }

    fn wrapped_nonuniform_physical() -> Vec<u8> {
        let mut bytes = b"BCDE".to_vec();
        bytes.extend_from_slice(&vec![b'A'; L - 4]);
        bytes
    }

    fn retain(
        bytes: &[u8],
        observed_length: Option<u64>,
        controls: &FixedOutputAllocationControls,
    ) -> (ExecutionOutputRetained, Option<OutputAllocationFailure>) {
        let mut allocations = OutputAllocationController::controlled(controls);
        let mut builder = ExecutionOutputRetainedBuilder::new(OutputStreamKind::Stdout);
        builder.observe(bytes, &mut allocations);
        builder
            .finish(observed_length, &mut allocations)
            .into_parts()
    }

    fn assert_unavailable(
        side: &ExecutionOutputRetainedSide,
        site: ExecutionOutputRetainedUnavailableSite,
    ) {
        assert!(!side.is_available());
        assert_eq!(side.bytes(), None);
        let unavailable = side.unavailable().expect("side must be unavailable");
        assert_eq!(
            unavailable.reason(),
            ExecutionOutputRetainedUnavailableReason::AllocationFailure
        );
        assert_eq!(unavailable.site(), site);
    }

    fn assert_failure(
        failure: Option<OutputAllocationFailure>,
        stream: OutputStreamKind,
        site: OutputAllocationSite,
        kind: OutputAllocationFailureKind,
    ) -> OutputAllocationFailure {
        let failure = failure.expect("allocation failure must be retained");
        assert_eq!(failure.stream(), stream);
        assert_eq!(failure.site(), site);
        assert_eq!(failure.kind(), kind);
        failure
    }

    #[test]
    fn retained_availability_truth_table_is_exhaustive() {
        const FUNCTION: &str =
            "executor::output::tests::retained_availability_truth_table_is_exhaustive";
        assert_fixture(
            SHORT,
            12,
            "a4cd019fb75bc5a166561aa7bc1adc7d1d96c694d3b2c39a9d1119f6660a9562",
        );
        let large = one_over_full();
        let rows = [
            ("known_short_aa", SHORT, Some(12), true, true),
            ("known_short_ua", SHORT, Some(12), false, true),
            ("known_short_au", SHORT, Some(12), true, false),
            ("known_short_uu", SHORT, Some(12), false, false),
            (
                "known_large_aa",
                large.as_slice(),
                Some(1_048_577),
                true,
                true,
            ),
            (
                "known_large_ua",
                large.as_slice(),
                Some(1_048_577),
                false,
                true,
            ),
            (
                "known_large_au",
                large.as_slice(),
                Some(1_048_577),
                true,
                false,
            ),
            (
                "known_large_uu",
                large.as_slice(),
                Some(1_048_577),
                false,
                false,
            ),
            ("unknown_aa", SHORT, None, true, true),
            ("unknown_ua", SHORT, None, false, true),
            ("unknown_au", SHORT, None, true, false),
            ("unknown_uu", SHORT, None, false, false),
        ];
        let mut executed = 0;
        for (case, bytes, observed, prefix_available, tail_available) in rows {
            run_case(FUNCTION, case, &mut executed, || {
                let mut controls = FixedOutputAllocationControls::new();
                if !prefix_available {
                    controls = controls.inject(
                        OutputStreamKind::Stdout,
                        OutputAllocationSite::RetainedPrefixWorking,
                    );
                }
                if !tail_available {
                    controls = controls.inject(
                        OutputStreamKind::Stdout,
                        OutputAllocationSite::RetainedTailWorking,
                    );
                }
                let (retained, _) = retain(bytes, observed, &controls);
                assert_eq!(retained.observed_length(), observed);
                assert_eq!(retained.prefix().is_available(), prefix_available);
                assert_eq!(retained.tail().is_available(), tail_available);
                assert_eq!(
                    retained.evidence_complete(),
                    prefix_available && tail_available
                );
                assert_eq!(
                    retained.truncated(),
                    !(prefix_available && tail_available)
                        || observed.is_none()
                        || observed > Some(1_048_576)
                );
                if prefix_available {
                    assert_eq!(
                        retained.prefix().bytes(),
                        Some(&bytes[..bytes.len().min(L)])
                    );
                } else {
                    assert_unavailable(
                        retained.prefix(),
                        ExecutionOutputRetainedUnavailableSite::RetainedPrefixWorking,
                    );
                }
                if tail_available {
                    let start = if bytes.len() > L {
                        bytes
                            .len()
                            .checked_sub(L)
                            .expect("large fixture exceeds retained tail length")
                    } else {
                        0
                    };
                    assert_eq!(retained.tail().bytes(), Some(&bytes[start..]));
                } else {
                    assert_unavailable(
                        retained.tail(),
                        ExecutionOutputRetainedUnavailableSite::RetainedTailWorking,
                    );
                }
            });
        }
        finish_cases(FUNCTION, 12, executed);
    }

    #[test]
    fn retained_boundaries_and_chronology_are_exact() {
        const FUNCTION: &str =
            "executor::output::tests::retained_boundaries_and_chronology_are_exact";
        let mut executed = 0;

        run_case(FUNCTION, "empty", &mut executed, || {
            let controls = FixedOutputAllocationControls::new();
            let (retained, failure) = retain(&[], Some(0), &controls);
            assert_fixture(
                retained.prefix().bytes().expect("available prefix"),
                0,
                "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
            );
            assert_eq!(retained.tail().bytes(), Some([].as_slice()));
            match retained.prefix() {
                ExecutionOutputRetainedSide::Available(bytes) => {
                    assert_eq!(bytes.bytes.capacity(), 0)
                }
                ExecutionOutputRetainedSide::Unavailable(_) => panic!("empty prefix unavailable"),
            }
            match retained.tail() {
                ExecutionOutputRetainedSide::Available(bytes) => {
                    assert_eq!(bytes.bytes.capacity(), 0)
                }
                ExecutionOutputRetainedSide::Unavailable(_) => panic!("empty tail unavailable"),
            }
            assert!(failure.is_none());
            assert_eq!(controls.total_calls(), 0);
            assert!(retained.evidence_complete());
            assert!(!retained.truncated());
        });

        run_case(FUNCTION, "short", &mut executed, || {
            let controls = FixedOutputAllocationControls::new();
            let (retained, failure) = retain(SHORT, Some(12), &controls);
            assert_eq!(retained.prefix().bytes(), Some(SHORT));
            assert_eq!(retained.tail().bytes(), Some(SHORT));
            assert!(failure.is_none());
        });

        run_case(FUNCTION, "exact_limit", &mut executed, || {
            let bytes = vec![b'A'; L];
            assert_fixture(
                &bytes,
                L,
                "4e29ad18ab9f42d7c233500771a39d7c852b200baf328fd00fbbe3fecea1eb56",
            );
            let controls = FixedOutputAllocationControls::new();
            let mut allocations = OutputAllocationController::controlled(&controls);
            let mut builder = ExecutionOutputRetainedBuilder::new(OutputStreamKind::Stdout);
            builder.observe(&bytes, &mut allocations);
            assert_eq!(builder.tail_start, 0);
            let (retained, failure) = builder
                .finish(Some(1_048_576), &mut allocations)
                .into_parts();
            assert_eq!(retained.prefix().bytes(), Some(bytes.as_slice()));
            assert_eq!(retained.tail().bytes(), Some(bytes.as_slice()));
            assert!(failure.is_none());
            assert!(!retained.truncated());
        });

        run_case(FUNCTION, "one_over", &mut executed, || {
            let bytes = one_over_full();
            let tail = one_over_tail();
            let physical = one_over_physical();
            assert_fixture(
                &bytes,
                L + 1,
                "fa9e7b91b1a668fe7b51f015de84297767f7411cd1b1eb6c3fecbb425569c17a",
            );
            assert_fixture(
                &tail,
                L,
                "2c38ddfea23f78ab424f4583fd2c59cbc357632c09f4393cb4ae197519db3aa9",
            );
            assert_fixture(
                &physical,
                L,
                "a13e4ff1a975003add65149d0993694b7b0d4241426d7240f77206078e06371b",
            );
            assert_ne!(tail, physical);
            let (retained, failure) = retain(
                &bytes,
                Some(1_048_577),
                &FixedOutputAllocationControls::new(),
            );
            assert_eq!(retained.prefix().bytes(), Some(&bytes[..L]));
            assert_eq!(retained.tail().bytes(), Some(tail.as_slice()));
            assert_ne!(retained.tail().bytes(), Some(physical.as_slice()));
            assert!(failure.is_none());
        });

        run_case(FUNCTION, "multiple_wrap", &mut executed, || {
            let bytes = multiple_wrap_full();
            let tail = multiple_wrap_tail();
            let physical = multiple_wrap_physical();
            assert_fixture(
                &bytes,
                2_097_156,
                "96cbdc20407e20a5b58cd1afdea52c979366215e56e7358116b729bb4a8dc85a",
            );
            assert_fixture(
                &tail,
                L,
                "16887f25d7d7a237739eb9ac5746ff85f7725c34c91fd7eb9e994526b83dd123",
            );
            assert_fixture(
                &physical,
                L,
                "5eb1459d518987d81cda89aec542dad89270c6b35a360a31881f2d08af5a4ad1",
            );
            let controls = FixedOutputAllocationControls::new();
            let mut allocations = OutputAllocationController::controlled(&controls);
            let mut builder = ExecutionOutputRetainedBuilder::new(OutputStreamKind::Stderr);
            builder.observe(&bytes[..L], &mut allocations);
            builder.observe(&[], &mut allocations);
            builder.observe(&bytes[L..2_097_152], &mut allocations);
            builder.observe(&bytes[2_097_152..], &mut allocations);
            assert_eq!(builder.tail_start, 4);
            let (retained, failure) = builder
                .finish(Some(2_097_156), &mut allocations)
                .into_parts();
            assert_eq!(retained.prefix().bytes(), Some(&bytes[..L]));
            assert_eq!(retained.tail().bytes(), Some(tail.as_slice()));
            assert_ne!(digest(retained.tail().bytes().unwrap()), digest(&physical));
            assert!(failure.is_none());
        });

        run_case(
            FUNCTION,
            "wrapped_nonuniform_normal_finalization",
            &mut executed,
            || {
                let bytes = wrapped_nonuniform_full();
                let tail = wrapped_nonuniform_tail();
                let physical = wrapped_nonuniform_physical();
                assert_fixture(
                    &bytes,
                    1_048_580,
                    "33d7547a13c4c40361b5e32e5a9af6eacb23b61c3d394fea939c6a82ab1b2f5b",
                );
                assert_fixture(
                    &tail,
                    L,
                    "af9ddbd89b1a998d9a20e3e67c9109bca9bacae998e371b911501ede77e2d93c",
                );
                assert_fixture(
                    &physical,
                    L,
                    "f1ef5beea0e48e996d350368e882de4e1881200b9f7878e33b8b7f2eafe19d35",
                );
                let (retained, failure) = retain(
                    &bytes,
                    Some(1_048_580),
                    &FixedOutputAllocationControls::new(),
                );
                assert_eq!(retained.prefix().bytes(), Some(&bytes[..L]));
                assert_eq!(retained.tail().bytes(), Some(tail.as_slice()));
                assert_ne!(retained.tail().bytes(), Some(physical.as_slice()));
                assert!(failure.is_none());
            },
        );
        finish_cases(FUNCTION, 6, executed);
    }

    #[test]
    fn retained_allocation_failure_truth_is_exact() {
        const FUNCTION: &str =
            "executor::output::tests::retained_allocation_failure_truth_is_exact";
        let mut executed = 0;

        run_case(
            FUNCTION,
            "prefix_working_unavailable",
            &mut executed,
            || {
                let controls = FixedOutputAllocationControls::new().inject(
                    OutputStreamKind::Stdout,
                    OutputAllocationSite::RetainedPrefixWorking,
                );
                let (retained, failure) = retain(SHORT, Some(12), &controls);
                assert_unavailable(
                    retained.prefix(),
                    ExecutionOutputRetainedUnavailableSite::RetainedPrefixWorking,
                );
                assert_eq!(retained.tail().bytes(), Some(SHORT));
                assert_failure(
                    failure,
                    OutputStreamKind::Stdout,
                    OutputAllocationSite::RetainedPrefixWorking,
                    OutputAllocationFailureKind::Injected,
                );
            },
        );

        run_case(FUNCTION, "tail_working_unavailable", &mut executed, || {
            let controls = FixedOutputAllocationControls::new().inject(
                OutputStreamKind::Stdout,
                OutputAllocationSite::RetainedTailWorking,
            );
            let (retained, failure) = retain(SHORT, Some(12), &controls);
            assert_eq!(retained.prefix().bytes(), Some(SHORT));
            assert_unavailable(
                retained.tail(),
                ExecutionOutputRetainedUnavailableSite::RetainedTailWorking,
            );
            assert_failure(
                failure,
                OutputStreamKind::Stdout,
                OutputAllocationSite::RetainedTailWorking,
                OutputAllocationFailureKind::Injected,
            );
        });

        run_case(
            FUNCTION,
            "both_working_unavailable_first_prefix",
            &mut executed,
            || {
                let controls = FixedOutputAllocationControls::new()
                    .inject(
                        OutputStreamKind::Stdout,
                        OutputAllocationSite::RetainedPrefixWorking,
                    )
                    .inject(
                        OutputStreamKind::Stdout,
                        OutputAllocationSite::RetainedTailWorking,
                    );
                let (retained, failure) = retain(SHORT, Some(12), &controls);
                assert_unavailable(
                    retained.prefix(),
                    ExecutionOutputRetainedUnavailableSite::RetainedPrefixWorking,
                );
                assert_unavailable(
                    retained.tail(),
                    ExecutionOutputRetainedUnavailableSite::RetainedTailWorking,
                );
                assert_failure(
                    failure,
                    OutputStreamKind::Stdout,
                    OutputAllocationSite::RetainedPrefixWorking,
                    OutputAllocationFailureKind::Injected,
                );
            },
        );

        run_case(FUNCTION, "prefix_final_fallback", &mut executed, || {
            let controls = FixedOutputAllocationControls::new().inject(
                OutputStreamKind::Stdout,
                OutputAllocationSite::RetainedPrefixFinal,
            );
            let (retained, failure) = retain(SHORT, Some(12), &controls);
            assert_eq!(retained.prefix().bytes(), Some(SHORT));
            assert_eq!(retained.tail().bytes(), Some(SHORT));
            assert_failure(
                failure,
                OutputStreamKind::Stdout,
                OutputAllocationSite::RetainedPrefixFinal,
                OutputAllocationFailureKind::Injected,
            );
            assert_eq!(
                controls.calls(
                    ControlFamily::Reserve,
                    OutputStreamKind::Stdout,
                    OutputAllocationSite::RetainedPrefixFinal,
                ),
                0
            );
        });

        run_case(
            FUNCTION,
            "tail_final_nonuniform_fallback",
            &mut executed,
            || {
                let bytes = wrapped_nonuniform_full();
                let tail = wrapped_nonuniform_tail();
                let physical = wrapped_nonuniform_physical();
                let controls = FixedOutputAllocationControls::new().inject(
                    OutputStreamKind::Stdout,
                    OutputAllocationSite::RetainedTailFinal,
                );
                let (retained, failure) = retain(&bytes, Some(1_048_580), &controls);
                let retained_tail = retained.tail().bytes().expect("tail fallback");
                assert_eq!(retained_tail, tail);
                assert_ne!(retained_tail, physical);
                assert_eq!(
                    digest(retained_tail),
                    "af9ddbd89b1a998d9a20e3e67c9109bca9bacae998e371b911501ede77e2d93c"
                );
                assert_ne!(digest(retained_tail), digest(&physical));
                assert_failure(
                    failure,
                    OutputStreamKind::Stdout,
                    OutputAllocationSite::RetainedTailFinal,
                    OutputAllocationFailureKind::Injected,
                );
            },
        );

        run_case(
            FUNCTION,
            "both_final_fallback_first_prefix",
            &mut executed,
            || {
                let controls = FixedOutputAllocationControls::new()
                    .inject(
                        OutputStreamKind::Stdout,
                        OutputAllocationSite::RetainedPrefixFinal,
                    )
                    .inject(
                        OutputStreamKind::Stdout,
                        OutputAllocationSite::RetainedTailFinal,
                    );
                let (retained, failure) = retain(SHORT, Some(12), &controls);
                assert_eq!(retained.prefix().bytes(), Some(SHORT));
                assert_eq!(retained.tail().bytes(), Some(SHORT));
                assert_failure(
                    failure,
                    OutputStreamKind::Stdout,
                    OutputAllocationSite::RetainedPrefixFinal,
                    OutputAllocationFailureKind::Injected,
                );
                assert_eq!(
                    controls.calls(
                        ControlFamily::Injected,
                        OutputStreamKind::Stdout,
                        OutputAllocationSite::RetainedTailFinal,
                    ),
                    1
                );
            },
        );

        run_case(
            FUNCTION,
            "working_failure_precedes_later_final_failure",
            &mut executed,
            || {
                let controls = FixedOutputAllocationControls::new()
                    .inject(
                        OutputStreamKind::Stdout,
                        OutputAllocationSite::RetainedPrefixWorking,
                    )
                    .inject(
                        OutputStreamKind::Stdout,
                        OutputAllocationSite::RetainedTailFinal,
                    );
                let mut allocations = OutputAllocationController::controlled(&controls);
                let mut builder = ExecutionOutputRetainedBuilder::new(OutputStreamKind::Stdout);
                builder.observe(SHORT, &mut allocations);
                assert_failure(
                    builder.first_allocation_failure(),
                    OutputStreamKind::Stdout,
                    OutputAllocationSite::RetainedPrefixWorking,
                    OutputAllocationFailureKind::Injected,
                );
                let (retained, failure) = builder.finish(Some(12), &mut allocations).into_parts();
                assert_unavailable(
                    retained.prefix(),
                    ExecutionOutputRetainedUnavailableSite::RetainedPrefixWorking,
                );
                assert_eq!(retained.tail().bytes(), Some(SHORT));
                assert_failure(
                    failure,
                    OutputStreamKind::Stdout,
                    OutputAllocationSite::RetainedPrefixWorking,
                    OutputAllocationFailureKind::Injected,
                );
                assert_eq!(
                    controls.calls(
                        ControlFamily::Injected,
                        OutputStreamKind::Stdout,
                        OutputAllocationSite::RetainedTailFinal,
                    ),
                    1
                );
            },
        );
        finish_cases(FUNCTION, 7, executed);
    }

    #[test]
    fn allocation_controller_is_fallible_bounded_and_growth_closed() {
        const FUNCTION: &str =
            "executor::output::tests::allocation_controller_is_fallible_bounded_and_growth_closed";
        let mut executed = 0;

        for (case, site) in [
            (
                "prefix_working_injected",
                OutputAllocationSite::RetainedPrefixWorking,
            ),
            (
                "tail_working_injected",
                OutputAllocationSite::RetainedTailWorking,
            ),
            (
                "prefix_final_injected",
                OutputAllocationSite::RetainedPrefixFinal,
            ),
            (
                "tail_final_injected",
                OutputAllocationSite::RetainedTailFinal,
            ),
        ] {
            run_case(FUNCTION, case, &mut executed, || {
                let requested = match site {
                    OutputAllocationSite::RetainedPrefixWorking
                    | OutputAllocationSite::RetainedTailWorking => L,
                    OutputAllocationSite::RetainedPrefixFinal
                    | OutputAllocationSite::RetainedTailFinal => 12,
                };
                let controls =
                    FixedOutputAllocationControls::new().inject(OutputStreamKind::Stderr, site);
                let mut controller = OutputAllocationController::controlled(&controls);
                let failure = controller
                    .try_vec::<u8>(OutputStreamKind::Stderr, site, requested)
                    .expect_err("injected failure");
                assert_failure(
                    Some(failure),
                    OutputStreamKind::Stderr,
                    site,
                    OutputAllocationFailureKind::Injected,
                );
                assert_eq!(
                    controls.calls(ControlFamily::Reserve, OutputStreamKind::Stderr, site),
                    0
                );
            });
        }

        run_case(FUNCTION, "exact_logical_bound", &mut executed, || {
            let controls = FixedOutputAllocationControls::new();
            let mut controller = OutputAllocationController::controlled(&controls);
            let (owner, grant) = controller
                .try_vec::<u8>(
                    OutputStreamKind::Stdout,
                    OutputAllocationSite::RetainedPrefixWorking,
                    L,
                )
                .expect("logical bound is accepted");
            assert!(owner.is_empty());
            assert_eq!(grant.stream(), OutputStreamKind::Stdout);
            assert_eq!(grant.site(), OutputAllocationSite::RetainedPrefixWorking);
            assert_eq!(grant.fill_limit_elements(), L);
            assert_eq!(grant.capacity_elements(), owner.capacity());
            assert_eq!(
                grant.capacity_bytes(),
                u64::try_from(owner.capacity()).unwrap()
            );
            assert!(grant.capacity_bytes() <= EXECUTION_OUTPUT_RETAINED_CAPACITY_MAX_BYTES);
        });

        run_case(FUNCTION, "above_logical_bound", &mut executed, || {
            let controls = FixedOutputAllocationControls::new();
            let mut controller = OutputAllocationController::controlled(&controls);
            let failure = controller
                .try_vec::<u8>(
                    OutputStreamKind::Stdout,
                    OutputAllocationSite::RetainedTailWorking,
                    L + 1,
                )
                .expect_err("above logical bound");
            let failure = assert_failure(
                Some(failure),
                OutputStreamKind::Stdout,
                OutputAllocationSite::RetainedTailWorking,
                OutputAllocationFailureKind::LogicalLimit,
            );
            assert_eq!(failure.requested_elements(), L + 1);
            assert_eq!(failure.logical_max_elements(), L);
            assert_eq!(controls.total_calls(), 0);
        });

        run_case(FUNCTION, "zero_sized_element", &mut executed, || {
            let controls = FixedOutputAllocationControls::new();
            let mut controller = OutputAllocationController::controlled(&controls);
            let failure = controller
                .try_vec::<()>(
                    OutputStreamKind::Stdout,
                    OutputAllocationSite::RetainedPrefixFinal,
                    1,
                )
                .expect_err("zero-sized elements are rejected");
            assert_failure(
                Some(failure),
                OutputStreamKind::Stdout,
                OutputAllocationSite::RetainedPrefixFinal,
                OutputAllocationFailureKind::ZeroSizedElement,
            );
            assert_eq!(controls.total_calls(), 0);
        });

        run_case(FUNCTION, "element_size_overflow", &mut executed, || {
            type OverflowElement = [u8; usize::MAX / L + 1];
            let controls = FixedOutputAllocationControls::new();
            let mut controller = OutputAllocationController::controlled(&controls);
            let failure = controller
                .try_vec::<OverflowElement>(
                    OutputStreamKind::Stdout,
                    OutputAllocationSite::RetainedPrefixFinal,
                    L,
                )
                .expect_err("requested element bytes overflow usize");
            assert_failure(
                Some(failure),
                OutputStreamKind::Stdout,
                OutputAllocationSite::RetainedPrefixFinal,
                OutputAllocationFailureKind::ElementSizeOverflow,
            );
            assert_eq!(controls.total_calls(), 0);
        });

        run_case(FUNCTION, "reserve_failure", &mut executed, || {
            let controls = FixedOutputAllocationControls::new().fail_reserve(
                OutputStreamKind::Stderr,
                OutputAllocationSite::RetainedTailFinal,
            );
            let mut controller = OutputAllocationController::controlled(&controls);
            let failure = controller
                .try_vec::<u8>(
                    OutputStreamKind::Stderr,
                    OutputAllocationSite::RetainedTailFinal,
                    12,
                )
                .expect_err("controlled reserve failure");
            assert_failure(
                Some(failure),
                OutputStreamKind::Stderr,
                OutputAllocationSite::RetainedTailFinal,
                OutputAllocationFailureKind::Reserve,
            );
            assert_eq!(
                controls.calls(
                    ControlFamily::Measured,
                    OutputStreamKind::Stderr,
                    OutputAllocationSite::RetainedTailFinal,
                ),
                0
            );
        });

        run_case(
            FUNCTION,
            "measured_capacity_overflow",
            &mut executed,
            || {
                let controls = FixedOutputAllocationControls::new().measure(
                    OutputStreamKind::Stdout,
                    OutputAllocationSite::RetainedPrefixFinal,
                    usize::MAX,
                );
                let mut controller = OutputAllocationController::controlled(&controls);
                let failure = controller
                    .try_vec::<u16>(
                        OutputStreamKind::Stdout,
                        OutputAllocationSite::RetainedPrefixFinal,
                        1,
                    )
                    .expect_err("controlled measured overflow");
                assert_failure(
                    Some(failure),
                    OutputStreamKind::Stdout,
                    OutputAllocationSite::RetainedPrefixFinal,
                    OutputAllocationFailureKind::MeasuredCapacityOverflow,
                );
            },
        );

        run_case(FUNCTION, "measured_capacity_limit", &mut executed, || {
            let controlled_capacity =
                usize::try_from(EXECUTION_OUTPUT_RETAINED_CAPACITY_MAX_BYTES + 1).unwrap();
            let controls = FixedOutputAllocationControls::new().measure(
                OutputStreamKind::Stdout,
                OutputAllocationSite::RetainedPrefixWorking,
                controlled_capacity,
            );
            let mut controller = OutputAllocationController::controlled(&controls);
            let failure = controller
                .try_vec::<u8>(
                    OutputStreamKind::Stdout,
                    OutputAllocationSite::RetainedPrefixWorking,
                    1,
                )
                .expect_err("controlled measured limit");
            let failure = assert_failure(
                Some(failure),
                OutputStreamKind::Stdout,
                OutputAllocationSite::RetainedPrefixWorking,
                OutputAllocationFailureKind::MeasuredCapacityLimit,
            );
            assert_eq!(
                failure.measured_capacity_bytes(),
                Some(EXECUTION_OUTPUT_RETAINED_CAPACITY_MAX_BYTES + 1)
            );
            assert_eq!(
                failure.measured_capacity_max_bytes(),
                EXECUTION_OUTPUT_RETAINED_CAPACITY_MAX_BYTES
            );
        });

        run_case(
            FUNCTION,
            "spare_capacity_fill_rejected",
            &mut executed,
            || {
                let controls = FixedOutputAllocationControls::new();
                let mut controller = OutputAllocationController::controlled(&controls);
                let mut owner = Vec::with_capacity(2);
                let capacity_elements = owner.capacity();
                let grant = OutputAllocationGrant {
                    stream: OutputStreamKind::Stdout,
                    site: OutputAllocationSite::RetainedPrefixFinal,
                    fill_limit_elements: 1,
                    capacity_elements,
                    capacity_bytes: u64::try_from(capacity_elements).unwrap(),
                    element_size_bytes: 1,
                };
                controller
                    .push_granted(
                        OutputStreamKind::Stdout,
                        OutputAllocationSite::RetainedPrefixFinal,
                        &grant,
                        &mut owner,
                        b'A',
                    )
                    .unwrap();
                let failure = controller
                    .push_granted(
                        OutputStreamKind::Stdout,
                        OutputAllocationSite::RetainedPrefixFinal,
                        &grant,
                        &mut owner,
                        b'B',
                    )
                    .expect_err("allocator spare capacity is not fill authority");
                assert_failure(
                    Some(failure),
                    OutputStreamKind::Stdout,
                    OutputAllocationSite::RetainedPrefixFinal,
                    OutputAllocationFailureKind::FillLimit,
                );
                assert_eq!(owner, b"A");
            },
        );

        run_case(FUNCTION, "grant_mismatch", &mut executed, || {
            let controls = FixedOutputAllocationControls::new();
            let mut controller = OutputAllocationController::controlled(&controls);
            let (mut owner, grant) = controller
                .try_vec::<u8>(
                    OutputStreamKind::Stdout,
                    OutputAllocationSite::RetainedPrefixFinal,
                    1,
                )
                .unwrap();
            let failure = controller
                .push_granted(
                    OutputStreamKind::Stdout,
                    OutputAllocationSite::RetainedTailFinal,
                    &grant,
                    &mut owner,
                    b'A',
                )
                .expect_err("site mismatch");
            assert_failure(
                Some(failure),
                OutputStreamKind::Stdout,
                OutputAllocationSite::RetainedTailFinal,
                OutputAllocationFailureKind::GrantMismatch,
            );
            assert!(owner.is_empty());
        });

        run_case(FUNCTION, "fill_length_overflow", &mut executed, || {
            let grant = OutputAllocationGrant {
                stream: OutputStreamKind::Stderr,
                site: OutputAllocationSite::RetainedTailWorking,
                fill_limit_elements: L,
                capacity_elements: L,
                capacity_bytes: 1_048_576,
                element_size_bytes: 1,
            };
            let failure = checked_fill_length(
                OutputStreamKind::Stderr,
                OutputAllocationSite::RetainedTailWorking,
                &grant,
                usize::MAX,
                1,
            )
            .expect_err("checked fill overflow");
            assert_failure(
                Some(failure),
                OutputStreamKind::Stderr,
                OutputAllocationSite::RetainedTailWorking,
                OutputAllocationFailureKind::FillLengthOverflow,
            );
        });
        finish_cases(FUNCTION, 14, executed);
    }

    #[test]
    fn retained_successor_surface_is_compile_complete_and_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}

        assert_send_sync::<ExecutionOutputRetained>();
        assert_send_sync::<ExecutionOutputRetainedSide>();
        assert_send_sync::<ExecutionOutputRetainedBytes>();
        assert_send_sync::<ExecutionOutputRetainedUnavailable>();
        assert_send_sync::<OutputAllocationFailure>();
        assert_send_sync::<OutputAllocationGrant>();
        assert_send_sync::<ExecutionOutputRetainedBuilder>();
        assert_send_sync::<ExecutionOutputRetainedFinalization>();
        assert_send_sync::<OutputAllocationController<'static>>();

        let _: fn(&ExecutionOutputRetained) -> &ExecutionOutputRetainedSide =
            ExecutionOutputRetained::prefix;
        let _: fn(&ExecutionOutputRetained) -> &ExecutionOutputRetainedSide =
            ExecutionOutputRetained::tail;
        let _: fn(&ExecutionOutputRetained) -> Option<u64> =
            ExecutionOutputRetained::observed_length;
        let _: fn(&ExecutionOutputRetained) -> bool = ExecutionOutputRetained::evidence_complete;
        let _: fn(&ExecutionOutputRetained) -> bool = ExecutionOutputRetained::truncated;
        let _: fn(OutputStreamKind) -> ExecutionOutputRetainedBuilder =
            ExecutionOutputRetainedBuilder::new;
        let _: fn(&ExecutionOutputRetainedBuilder) -> Option<OutputAllocationFailure> =
            ExecutionOutputRetainedBuilder::first_allocation_failure;
        let _: fn(
            ExecutionOutputRetainedFinalization,
        ) -> (ExecutionOutputRetained, Option<OutputAllocationFailure>) =
            ExecutionOutputRetainedFinalization::into_parts;

        let mut production = OutputAllocationController::production();
        let (mut owner, grant) = production
            .try_vec::<u8>(
                OutputStreamKind::Stdout,
                OutputAllocationSite::RetainedPrefixFinal,
                0,
            )
            .expect("zero grant");
        production
            .extend_granted_bytes(
                OutputStreamKind::Stdout,
                OutputAllocationSite::RetainedPrefixFinal,
                &grant,
                &mut owner,
                &[],
            )
            .expect("zero fill");
    }

    #[test]
    fn private_module_has_no_facade_reexport_or_execution_route() {
        let source = include_str!("mod.rs");
        let declaration = "#[allow(dead_code)]\nmod output;";
        assert_eq!(source.matches(declaration).count(), 1);
        assert!(!source.contains("pub mod output"));
        assert!(!source.contains("pub use self::output"));
        let below = source
            .split_once(declaration)
            .expect("private output declaration")
            .1;
        for symbol in [
            "ExecutionOutputRetained",
            "OutputAllocationController",
            "ExecutionOutputRetainedBuilder",
        ] {
            assert!(!below.contains(symbol), "route contains {symbol}");
        }
    }

    #[test]
    fn production_allocation_families_are_closed_and_classified() {
        struct ExpectedOccurrence {
            snippet: &'static str,
            count: usize,
            classification: &'static str,
            allowed_sites: &'static str,
        }

        const EXPECTED: &[ExpectedOccurrence] = &[
            ExpectedOccurrence {
                snippet: "bytes: Vec<u8>,",
                count: 1,
                classification: "published_owner",
                allowed_sites: "all_four",
            },
            ExpectedOccurrence {
                snippet: "owner: Vec<u8>,",
                count: 1,
                classification: "concrete_site",
                allowed_sites: "working",
            },
            ExpectedOccurrence {
                snippet: "Result<(Vec<T>, OutputAllocationGrant)",
                count: 1,
                classification: "shared_controller",
                allowed_sites: "all_four",
            },
            ExpectedOccurrence {
                snippet: "owner: &mut Vec<T>,",
                count: 1,
                classification: "shared_controller",
                allowed_sites: "all_four",
            },
            ExpectedOccurrence {
                snippet: "owner: &mut Vec<u8>,",
                count: 1,
                classification: "shared_controller",
                allowed_sites: "all_four",
            },
            ExpectedOccurrence {
                snippet: "let mut owner = Vec::new();",
                count: 1,
                classification: "empty_constructor",
                allowed_sites: "all_four",
            },
            ExpectedOccurrence {
                snippet: "owner.try_reserve_exact(requested_elements)",
                count: 1,
                classification: "shared_controller",
                allowed_sites: "all_four",
            },
            ExpectedOccurrence {
                snippet: "owner.push(value);",
                count: 1,
                classification: "shared_controller",
                allowed_sites: "all_four",
            },
            ExpectedOccurrence {
                snippet: "owner.extend_from_slice(bytes);",
                count: 1,
                classification: "shared_controller",
                allowed_sites: "all_four",
            },
            ExpectedOccurrence {
                snippet: "bytes: Vec::new(),",
                count: 1,
                classification: "empty_constructor",
                allowed_sites: "none",
            },
        ];

        let source = include_str!("output.rs");
        let marker = "#[cfg(test)]\nmod tests {";
        assert_eq!(
            source.matches(marker).count(),
            1,
            "one terminal test module"
        );
        let production = source
            .split_once(marker)
            .expect("terminal test module marker")
            .0;
        for expected in EXPECTED {
            assert_eq!(
                production.matches(expected.snippet).count(),
                expected.count,
                "unexpected occurrence count for {} ({}, {})",
                expected.snippet,
                expected.classification,
                expected.allowed_sites,
            );
        }

        let allocation_family_line = |line: &str| {
            line.contains("Vec")
                || line.contains(".try_reserve_exact(")
                || line.contains(".push(")
                || line.contains(".extend_from_slice(")
                || line.contains("String")
                || line.contains("Box")
                || line.contains("Cow")
                || line.contains("HashMap")
                || line.contains("BTreeMap")
                || line.contains("VecDeque")
                || line.contains("PathBuf")
                || line.contains("File::")
                || line.contains("OpenOptions")
                || line.contains("tempfile")
                || line.contains("mmap")
                || line.contains("alloc::alloc")
        };
        for (line_number, line) in production.lines().enumerate() {
            if allocation_family_line(line) {
                assert!(
                    EXPECTED
                        .iter()
                        .any(|expected| line.contains(expected.snippet)),
                    "unclassified production allocation family at line {}: {}",
                    line_number.checked_add(1).unwrap(),
                    line,
                );
            }
        }
    }
}
