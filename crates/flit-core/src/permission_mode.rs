use std::{error::Error, fmt};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PermissionPolicyOperationId(String);

impl PermissionPolicyOperationId {
    pub fn new(value: impl Into<String>) -> Result<Self, PermissionModeValueError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(PermissionModeValueError::BlankOperationId);
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PolicyFingerprint(String);

impl PolicyFingerprint {
    pub fn new(value: impl Into<String>) -> Result<Self, PermissionModeValueError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(PermissionModeValueError::BlankPolicyFingerprint);
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PermissionMode {
    Manual,
    ApproveForMe,
    Unknown,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PermissionModeSnapshot {
    mode: PermissionMode,
    version: u64,
    policy_fingerprint: Option<PolicyFingerprint>,
}

impl PermissionModeSnapshot {
    pub fn new(
        mode: PermissionMode,
        version: u64,
        policy_fingerprint: Option<PolicyFingerprint>,
    ) -> Result<Self, PermissionModeValueError> {
        if version == 0 {
            return Err(PermissionModeValueError::InvalidModeVersion);
        }
        match (mode, policy_fingerprint.as_ref()) {
            (PermissionMode::Unknown, Some(_)) => {
                return Err(PermissionModeValueError::UnknownModeHasFingerprint);
            }
            (PermissionMode::Manual | PermissionMode::ApproveForMe, None) => {
                return Err(PermissionModeValueError::VerifiedModeRequiresFingerprint);
            }
            _ => {}
        }
        Ok(Self {
            mode,
            version,
            policy_fingerprint,
        })
    }

    #[must_use]
    pub const fn mode(&self) -> PermissionMode {
        self.mode
    }

    #[must_use]
    pub const fn version(&self) -> u64 {
        self.version
    }

    #[must_use]
    pub fn policy_fingerprint(&self) -> Option<&PolicyFingerprint> {
        self.policy_fingerprint.as_ref()
    }

    #[must_use]
    pub const fn is_verified(&self) -> bool {
        !matches!(self.mode, PermissionMode::Unknown)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PermissionModeValueError {
    BlankOperationId,
    BlankPolicyFingerprint,
    InvalidModeVersion,
    UnknownModeHasFingerprint,
    VerifiedModeRequiresFingerprint,
}

impl fmt::Display for PermissionModeValueError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BlankOperationId => {
                formatter.write_str("permission policy operation ID must not be blank")
            }
            Self::BlankPolicyFingerprint => {
                formatter.write_str("provider policy fingerprint must not be blank")
            }
            Self::InvalidModeVersion => {
                formatter.write_str("permission mode version must be greater than zero")
            }
            Self::UnknownModeHasFingerprint => {
                formatter.write_str("unknown permission mode must not have a fingerprint")
            }
            Self::VerifiedModeRequiresFingerprint => {
                formatter.write_str("verified permission mode requires a fingerprint")
            }
        }
    }
}

impl Error for PermissionModeValueError {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PermissionModeChange {
    operation_id: PermissionPolicyOperationId,
    expected_mode_version: u64,
    requested: PermissionModeSnapshot,
}

impl PermissionModeChange {
    #[must_use]
    pub fn operation_id(&self) -> &PermissionPolicyOperationId {
        &self.operation_id
    }

    #[must_use]
    pub const fn expected_mode_version(&self) -> u64 {
        self.expected_mode_version
    }

    #[must_use]
    pub const fn requested(&self) -> &PermissionModeSnapshot {
        &self.requested
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PolicyConfigurationState {
    Stable,
    Pending(PermissionModeChange),
    Unknown(PermissionModeChange),
}

impl PolicyConfigurationState {
    fn active_change(&self) -> Option<&PermissionModeChange> {
        match self {
            Self::Pending(change) | Self::Unknown(change) => Some(change),
            Self::Stable => None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PermissionModeProjection {
    current: PermissionModeSnapshot,
    configuration_state: PolicyConfigurationState,
    last_ingest_seq: u64,
    used_operation_ids: Vec<PermissionPolicyOperationId>,
}

impl PermissionModeProjection {
    pub fn new(
        initial: PermissionModeSnapshot,
        initial_ingest_seq: u64,
    ) -> Result<Self, PermissionModeError> {
        if initial_ingest_seq == 0 {
            return Err(PermissionModeError::InvalidInitialIngestSequence);
        }
        Ok(Self {
            current: initial,
            configuration_state: PolicyConfigurationState::Stable,
            last_ingest_seq: initial_ingest_seq,
            used_operation_ids: Vec::new(),
        })
    }

    #[must_use]
    pub const fn current(&self) -> &PermissionModeSnapshot {
        &self.current
    }

    #[must_use]
    pub const fn configuration_state(&self) -> &PolicyConfigurationState {
        &self.configuration_state
    }

    #[must_use]
    pub const fn last_ingest_seq(&self) -> u64 {
        self.last_ingest_seq
    }

    #[must_use]
    pub fn used_operation_ids(&self) -> &[PermissionPolicyOperationId] {
        &self.used_operation_ids
    }

    #[must_use]
    pub fn permission_response_enabled(&self) -> bool {
        matches!(self.configuration_state, PolicyConfigurationState::Stable)
            && self.current.is_verified()
    }

    #[must_use]
    pub fn policy_observation_enabled(&self) -> bool {
        matches!(self.configuration_state, PolicyConfigurationState::Stable)
            && self.current.is_verified()
    }

    pub fn apply(
        &mut self,
        ingest_seq: u64,
        event: PermissionModeEvent,
    ) -> Result<PermissionModeDisposition, PermissionModeError> {
        if ingest_seq <= self.last_ingest_seq {
            return Err(PermissionModeError::NonMonotonicIngestSequence {
                current: self.last_ingest_seq,
                received: ingest_seq,
            });
        }

        let disposition = self.apply_ordered(event);
        self.last_ingest_seq = ingest_seq;
        debug_assert!(self.invariants_hold());
        Ok(disposition)
    }

    fn apply_ordered(&mut self, event: PermissionModeEvent) -> PermissionModeDisposition {
        match event {
            PermissionModeEvent::ChangeSubmitted {
                operation_id,
                expected_mode_version,
                requested,
            } => self.submit_change(operation_id, expected_mode_version, requested),
            PermissionModeEvent::ConfigurationSucceeded {
                operation_id,
                applied,
            } => self.configuration_succeeded(&operation_id, applied),
            PermissionModeEvent::ConfigurationRejectedNotApplied { operation_id } => {
                self.configuration_rejected(&operation_id)
            }
            PermissionModeEvent::ConfigurationApplicationUnknown { operation_id } => {
                self.configuration_unknown(&operation_id)
            }
        }
    }

    fn submit_change(
        &mut self,
        operation_id: PermissionPolicyOperationId,
        expected_mode_version: u64,
        requested: PermissionModeSnapshot,
    ) -> PermissionModeDisposition {
        if expected_mode_version != self.current.version() {
            return PermissionModeDisposition::Ignored(
                IgnoredPermissionModeReason::StaleExpectedModeVersion {
                    current: self.current.version(),
                    received: expected_mode_version,
                },
            );
        }
        match self.configuration_state {
            PolicyConfigurationState::Pending(_) => {
                return PermissionModeDisposition::Ignored(
                    IgnoredPermissionModeReason::ConfigurationAlreadyPending,
                );
            }
            PolicyConfigurationState::Unknown(_) => {
                return PermissionModeDisposition::Ignored(
                    IgnoredPermissionModeReason::ConfigurationUnknownLocked,
                );
            }
            PolicyConfigurationState::Stable => {}
        }
        if !requested.is_verified() {
            return PermissionModeDisposition::Ignored(
                IgnoredPermissionModeReason::RequestedModeMustBeVerified,
            );
        }
        let Some(expected_next_version) = self.current.version().checked_add(1) else {
            return PermissionModeDisposition::Ignored(
                IgnoredPermissionModeReason::ModeVersionExhausted,
            );
        };
        if requested.version() != expected_next_version {
            return PermissionModeDisposition::Ignored(
                IgnoredPermissionModeReason::InvalidNextModeVersion {
                    expected: expected_next_version,
                    received: requested.version(),
                },
            );
        }
        if self.used_operation_ids.contains(&operation_id) {
            return PermissionModeDisposition::Ignored(
                IgnoredPermissionModeReason::OperationAlreadyUsed,
            );
        }

        self.used_operation_ids.push(operation_id.clone());
        self.configuration_state = PolicyConfigurationState::Pending(PermissionModeChange {
            operation_id,
            expected_mode_version,
            requested,
        });
        PermissionModeDisposition::Applied
    }

    fn configuration_succeeded(
        &mut self,
        operation_id: &PermissionPolicyOperationId,
        applied: PermissionModeSnapshot,
    ) -> PermissionModeDisposition {
        let Some(change) = self.match_active_operation(operation_id) else {
            return self.handle_non_active_receipt(operation_id);
        };
        if applied != change.requested {
            return self.lock_or_preserve_unknown();
        }

        self.current = applied;
        self.configuration_state = PolicyConfigurationState::Stable;
        PermissionModeDisposition::Applied
    }

    fn configuration_rejected(
        &mut self,
        operation_id: &PermissionPolicyOperationId,
    ) -> PermissionModeDisposition {
        if self.match_active_operation(operation_id).is_none() {
            return self.handle_non_active_receipt(operation_id);
        }

        self.configuration_state = PolicyConfigurationState::Stable;
        PermissionModeDisposition::Applied
    }

    fn configuration_unknown(
        &mut self,
        operation_id: &PermissionPolicyOperationId,
    ) -> PermissionModeDisposition {
        if self.match_active_operation(operation_id).is_none() {
            return self.handle_non_active_receipt(operation_id);
        }
        self.lock_or_preserve_unknown()
    }

    fn match_active_operation(
        &self,
        operation_id: &PermissionPolicyOperationId,
    ) -> Option<PermissionModeChange> {
        self.configuration_state
            .active_change()
            .filter(|change| change.operation_id() == operation_id)
            .cloned()
    }

    fn handle_non_active_receipt(
        &mut self,
        operation_id: &PermissionPolicyOperationId,
    ) -> PermissionModeDisposition {
        if self.used_operation_ids.contains(operation_id) {
            return PermissionModeDisposition::Ignored(
                IgnoredPermissionModeReason::StaleOrDuplicateOperationReceipt,
            );
        }
        if self.configuration_state.active_change().is_some() {
            return self.lock_or_preserve_unknown();
        }
        PermissionModeDisposition::Ignored(IgnoredPermissionModeReason::NoActiveConfiguration)
    }

    fn lock_or_preserve_unknown(&mut self) -> PermissionModeDisposition {
        match &self.configuration_state {
            PolicyConfigurationState::Pending(change) => {
                self.configuration_state = PolicyConfigurationState::Unknown(change.clone());
                PermissionModeDisposition::Applied
            }
            PolicyConfigurationState::Unknown(_) => PermissionModeDisposition::Ignored(
                IgnoredPermissionModeReason::ConfigurationAlreadyUnknown,
            ),
            PolicyConfigurationState::Stable => PermissionModeDisposition::Ignored(
                IgnoredPermissionModeReason::NoActiveConfiguration,
            ),
        }
    }

    fn invariants_hold(&self) -> bool {
        let operation_ids_unique = self
            .used_operation_ids
            .iter()
            .enumerate()
            .all(|(index, operation_id)| !self.used_operation_ids[..index].contains(operation_id));
        let active_change_valid = self
            .configuration_state
            .active_change()
            .is_none_or(|change| {
                self.used_operation_ids.contains(change.operation_id())
                    && change.expected_mode_version() == self.current.version()
                    && change.requested().is_verified()
                    && self
                        .current
                        .version()
                        .checked_add(1)
                        .is_some_and(|next| change.requested().version() == next)
            });
        operation_ids_unique && active_change_valid
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PermissionModeEvent {
    ChangeSubmitted {
        operation_id: PermissionPolicyOperationId,
        expected_mode_version: u64,
        requested: PermissionModeSnapshot,
    },
    ConfigurationSucceeded {
        operation_id: PermissionPolicyOperationId,
        applied: PermissionModeSnapshot,
    },
    ConfigurationRejectedNotApplied {
        operation_id: PermissionPolicyOperationId,
    },
    ConfigurationApplicationUnknown {
        operation_id: PermissionPolicyOperationId,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PermissionModeDisposition {
    Applied,
    Ignored(IgnoredPermissionModeReason),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IgnoredPermissionModeReason {
    StaleExpectedModeVersion { current: u64, received: u64 },
    ConfigurationAlreadyPending,
    ConfigurationUnknownLocked,
    RequestedModeMustBeVerified,
    ModeVersionExhausted,
    InvalidNextModeVersion { expected: u64, received: u64 },
    OperationAlreadyUsed,
    StaleOrDuplicateOperationReceipt,
    NoActiveConfiguration,
    ConfigurationAlreadyUnknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PermissionModeError {
    InvalidInitialIngestSequence,
    NonMonotonicIngestSequence { current: u64, received: u64 },
}

impl fmt::Display for PermissionModeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidInitialIngestSequence => {
                formatter.write_str("initial ingest sequence must be greater than zero")
            }
            Self::NonMonotonicIngestSequence { current, received } => write!(
                formatter,
                "ingest sequence must increase: current={current}, received={received}"
            ),
        }
    }
}

impl Error for PermissionModeError {}

pub fn replay_permission_mode<I>(
    initial: PermissionModeSnapshot,
    initial_ingest_seq: u64,
    events: I,
) -> Result<PermissionModeProjection, PermissionModeError>
where
    I: IntoIterator<Item = (u64, PermissionModeEvent)>,
{
    let mut projection = PermissionModeProjection::new(initial, initial_ingest_seq)?;
    for (ingest_seq, event) in events {
        projection.apply(ingest_seq, event)?;
    }
    Ok(projection)
}
