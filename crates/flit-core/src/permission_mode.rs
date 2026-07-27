use std::{error::Error, fmt};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PermissionModeOperationId(String);

impl PermissionModeOperationId {
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
pub struct ProviderConfigurationIdentity(String);

impl ProviderConfigurationIdentity {
    pub fn new(value: impl Into<String>) -> Result<Self, PermissionModeValueError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(PermissionModeValueError::BlankProviderConfigurationIdentity);
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
    ProviderAuto,
    Unknown,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PermissionModeSnapshot {
    mode: PermissionMode,
    version: u64,
    provider_configuration_identity: Option<ProviderConfigurationIdentity>,
}

impl PermissionModeSnapshot {
    pub fn new(
        mode: PermissionMode,
        version: u64,
        provider_configuration_identity: Option<ProviderConfigurationIdentity>,
    ) -> Result<Self, PermissionModeValueError> {
        if version == 0 {
            return Err(PermissionModeValueError::InvalidModeVersion);
        }
        match (mode, provider_configuration_identity.as_ref()) {
            (PermissionMode::Unknown, Some(_)) => {
                return Err(PermissionModeValueError::UnknownModeHasProviderConfiguration);
            }
            (PermissionMode::Manual | PermissionMode::ProviderAuto, None) => {
                return Err(PermissionModeValueError::VerifiedModeRequiresProviderConfiguration);
            }
            _ => {}
        }
        Ok(Self {
            mode,
            version,
            provider_configuration_identity,
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
    pub fn provider_configuration_identity(&self) -> Option<&ProviderConfigurationIdentity> {
        self.provider_configuration_identity.as_ref()
    }

    #[must_use]
    pub const fn is_verified(&self) -> bool {
        !matches!(self.mode, PermissionMode::Unknown)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PermissionModeValueError {
    BlankOperationId,
    BlankProviderConfigurationIdentity,
    BlankProviderStreamId,
    InvalidModeVersion,
    UnknownModeHasProviderConfiguration,
    VerifiedModeRequiresProviderConfiguration,
}

impl fmt::Display for PermissionModeValueError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BlankOperationId => {
                formatter.write_str("permission mode operation ID must not be blank")
            }
            Self::BlankProviderConfigurationIdentity => {
                formatter.write_str("provider configuration identity must not be blank")
            }
            Self::BlankProviderStreamId => {
                formatter.write_str("provider stream ID must not be blank")
            }
            Self::InvalidModeVersion => {
                formatter.write_str("permission mode version must be greater than zero")
            }
            Self::UnknownModeHasProviderConfiguration => formatter.write_str(
                "unknown permission mode must not have a provider configuration identity",
            ),
            Self::VerifiedModeRequiresProviderConfiguration => formatter
                .write_str("verified permission mode requires a provider configuration identity"),
        }
    }
}

impl Error for PermissionModeValueError {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PermissionModeChange {
    operation_id: PermissionModeOperationId,
    expected_mode_version: u64,
    prior: PermissionModeSnapshot,
    requested: PermissionModeSnapshot,
}

impl PermissionModeChange {
    #[must_use]
    pub fn operation_id(&self) -> &PermissionModeOperationId {
        &self.operation_id
    }

    #[must_use]
    pub const fn expected_mode_version(&self) -> u64 {
        self.expected_mode_version
    }

    #[must_use]
    pub const fn prior(&self) -> &PermissionModeSnapshot {
        &self.prior
    }

    #[must_use]
    pub const fn requested(&self) -> &PermissionModeSnapshot {
        &self.requested
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ModeConfigurationState {
    Stable,
    Pending(PermissionModeChange),
    Unknown(PermissionModeChange),
}

impl ModeConfigurationState {
    fn active_change(&self) -> Option<&PermissionModeChange> {
        match self {
            Self::Pending(change) | Self::Unknown(change) => Some(change),
            Self::Stable => None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProviderStreamId(String);

impl ProviderStreamId {
    pub fn new(value: impl Into<String>) -> Result<Self, PermissionModeValueError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(PermissionModeValueError::BlankProviderStreamId);
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OrderedProviderCursor {
    stream_id: ProviderStreamId,
    position: u64,
}

impl OrderedProviderCursor {
    #[must_use]
    pub const fn new(stream_id: ProviderStreamId, position: u64) -> Self {
        Self {
            stream_id,
            position,
        }
    }

    #[must_use]
    pub const fn stream_id(&self) -> &ProviderStreamId {
        &self.stream_id
    }

    #[must_use]
    pub const fn position(&self) -> u64 {
        self.position
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PendingModeObservation {
    operation_id: PermissionModeOperationId,
    cursor: Option<OrderedProviderCursor>,
}

impl PendingModeObservation {
    #[must_use]
    pub const fn new(
        operation_id: PermissionModeOperationId,
        cursor: Option<OrderedProviderCursor>,
    ) -> Self {
        Self {
            operation_id,
            cursor,
        }
    }

    #[must_use]
    pub const fn operation_id(&self) -> &PermissionModeOperationId {
        &self.operation_id
    }

    #[must_use]
    pub const fn cursor(&self) -> Option<&OrderedProviderCursor> {
        self.cursor.as_ref()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CompletedPermissionModeOutcome {
    Configured {
        effective_cursor: Option<OrderedProviderCursor>,
    },
    RejectedNotApplied,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompletedPermissionModeChange {
    change: PermissionModeChange,
    outcome: CompletedPermissionModeOutcome,
}

impl CompletedPermissionModeChange {
    #[must_use]
    pub const fn change(&self) -> &PermissionModeChange {
        &self.change
    }

    #[must_use]
    pub const fn outcome(&self) -> &CompletedPermissionModeOutcome {
        &self.outcome
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ModeObservationBinding {
    AwaitingConfiguration,
    Bound(PermissionModeSnapshot),
    ProviderOutcomeUnknown(ModeObservationUnknownReason),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ModeObservationUnknownReason {
    ConfigurationApplicationUnknown,
    MissingObservationCursor,
    MissingEffectiveCursor,
    IncomparableProviderStream,
    UnknownOperation,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PermissionModeProjection {
    current: PermissionModeSnapshot,
    configuration_state: ModeConfigurationState,
    last_ingest_seq: u64,
    used_operation_ids: Vec<PermissionModeOperationId>,
    completed_changes: Vec<CompletedPermissionModeChange>,
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
            configuration_state: ModeConfigurationState::Stable,
            last_ingest_seq: initial_ingest_seq,
            used_operation_ids: Vec::new(),
            completed_changes: Vec::new(),
        })
    }

    #[must_use]
    pub const fn current(&self) -> &PermissionModeSnapshot {
        &self.current
    }

    #[must_use]
    pub const fn configuration_state(&self) -> &ModeConfigurationState {
        &self.configuration_state
    }

    #[must_use]
    pub const fn last_ingest_seq(&self) -> u64 {
        self.last_ingest_seq
    }

    #[must_use]
    pub fn used_operation_ids(&self) -> &[PermissionModeOperationId] {
        &self.used_operation_ids
    }

    #[must_use]
    pub fn completed_changes(&self) -> &[CompletedPermissionModeChange] {
        &self.completed_changes
    }

    #[must_use]
    pub fn bind_pending_observation(
        &self,
        observation: &PendingModeObservation,
    ) -> ModeObservationBinding {
        if let Some(active) = self.configuration_state.active_change()
            && active.operation_id() == observation.operation_id()
        {
            return match self.configuration_state {
                ModeConfigurationState::Pending(_) => ModeObservationBinding::AwaitingConfiguration,
                ModeConfigurationState::Unknown(_) => {
                    ModeObservationBinding::ProviderOutcomeUnknown(
                        ModeObservationUnknownReason::ConfigurationApplicationUnknown,
                    )
                }
                ModeConfigurationState::Stable => unreachable!("active change is not stable"),
            };
        }

        let Some(completed) = self
            .completed_changes
            .iter()
            .find(|completed| completed.change().operation_id() == observation.operation_id())
        else {
            return ModeObservationBinding::ProviderOutcomeUnknown(
                ModeObservationUnknownReason::UnknownOperation,
            );
        };
        match completed.outcome() {
            CompletedPermissionModeOutcome::RejectedNotApplied => {
                ModeObservationBinding::Bound(completed.change().prior().clone())
            }
            CompletedPermissionModeOutcome::Configured { effective_cursor } => {
                Self::bind_configured_observation(
                    completed.change(),
                    effective_cursor.as_ref(),
                    observation,
                )
            }
        }
    }

    fn bind_configured_observation(
        change: &PermissionModeChange,
        effective_cursor: Option<&OrderedProviderCursor>,
        observation: &PendingModeObservation,
    ) -> ModeObservationBinding {
        let Some(observation_cursor) = observation.cursor() else {
            return ModeObservationBinding::ProviderOutcomeUnknown(
                ModeObservationUnknownReason::MissingObservationCursor,
            );
        };
        let Some(effective_cursor) = effective_cursor else {
            return ModeObservationBinding::ProviderOutcomeUnknown(
                ModeObservationUnknownReason::MissingEffectiveCursor,
            );
        };
        if observation_cursor.stream_id() != effective_cursor.stream_id() {
            return ModeObservationBinding::ProviderOutcomeUnknown(
                ModeObservationUnknownReason::IncomparableProviderStream,
            );
        }
        if observation_cursor.position() < effective_cursor.position() {
            ModeObservationBinding::Bound(change.prior().clone())
        } else {
            ModeObservationBinding::Bound(change.requested().clone())
        }
    }

    #[must_use]
    pub fn permission_response_enabled(&self, request_mode: &PermissionModeSnapshot) -> bool {
        matches!(self.configuration_state, ModeConfigurationState::Stable)
            && self.current.is_verified()
            && matches!(request_mode.mode(), PermissionMode::Manual)
    }

    #[must_use]
    pub fn provider_outcome_observation_enabled(
        &self,
        request_mode: &PermissionModeSnapshot,
    ) -> bool {
        matches!(self.configuration_state, ModeConfigurationState::Stable)
            && self.current.is_verified()
            && matches!(request_mode.mode(), PermissionMode::ProviderAuto)
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
                effective_cursor,
            } => self.configuration_succeeded(&operation_id, applied, effective_cursor),
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
        operation_id: PermissionModeOperationId,
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
            ModeConfigurationState::Pending(_) => {
                return PermissionModeDisposition::Ignored(
                    IgnoredPermissionModeReason::ConfigurationAlreadyPending,
                );
            }
            ModeConfigurationState::Unknown(_) => {
                return PermissionModeDisposition::Ignored(
                    IgnoredPermissionModeReason::ConfigurationUnknownLocked,
                );
            }
            ModeConfigurationState::Stable => {}
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
        self.configuration_state = ModeConfigurationState::Pending(PermissionModeChange {
            operation_id,
            expected_mode_version,
            prior: self.current.clone(),
            requested,
        });
        PermissionModeDisposition::Applied
    }

    fn configuration_succeeded(
        &mut self,
        operation_id: &PermissionModeOperationId,
        applied: PermissionModeSnapshot,
        effective_cursor: Option<OrderedProviderCursor>,
    ) -> PermissionModeDisposition {
        let Some(change) = self.match_active_operation(operation_id) else {
            return self.handle_non_active_receipt(operation_id);
        };
        if applied != change.requested {
            return self.lock_or_preserve_unknown();
        }

        self.current = applied;
        self.configuration_state = ModeConfigurationState::Stable;
        self.completed_changes.push(CompletedPermissionModeChange {
            change,
            outcome: CompletedPermissionModeOutcome::Configured { effective_cursor },
        });
        PermissionModeDisposition::Applied
    }

    fn configuration_rejected(
        &mut self,
        operation_id: &PermissionModeOperationId,
    ) -> PermissionModeDisposition {
        let Some(change) = self.match_active_operation(operation_id) else {
            return self.handle_non_active_receipt(operation_id);
        };

        self.configuration_state = ModeConfigurationState::Stable;
        self.completed_changes.push(CompletedPermissionModeChange {
            change,
            outcome: CompletedPermissionModeOutcome::RejectedNotApplied,
        });
        PermissionModeDisposition::Applied
    }

    fn configuration_unknown(
        &mut self,
        operation_id: &PermissionModeOperationId,
    ) -> PermissionModeDisposition {
        if self.match_active_operation(operation_id).is_none() {
            return self.handle_non_active_receipt(operation_id);
        }
        self.lock_or_preserve_unknown()
    }

    fn match_active_operation(
        &self,
        operation_id: &PermissionModeOperationId,
    ) -> Option<PermissionModeChange> {
        self.configuration_state
            .active_change()
            .filter(|change| change.operation_id() == operation_id)
            .cloned()
    }

    fn handle_non_active_receipt(
        &mut self,
        operation_id: &PermissionModeOperationId,
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
            ModeConfigurationState::Pending(change) => {
                self.configuration_state = ModeConfigurationState::Unknown(change.clone());
                PermissionModeDisposition::Applied
            }
            ModeConfigurationState::Unknown(_) => PermissionModeDisposition::Ignored(
                IgnoredPermissionModeReason::ConfigurationAlreadyUnknown,
            ),
            ModeConfigurationState::Stable => PermissionModeDisposition::Ignored(
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
                    && change.prior() == &self.current
                    && change.requested().is_verified()
                    && self
                        .current
                        .version()
                        .checked_add(1)
                        .is_some_and(|next| change.requested().version() == next)
            });
        let completed_changes_valid =
            self.completed_changes
                .iter()
                .enumerate()
                .all(|(index, completed)| {
                    self.used_operation_ids
                        .contains(completed.change().operation_id())
                        && completed.change().requested().is_verified()
                        && !self.completed_changes[..index].iter().any(|earlier| {
                            earlier.change().operation_id() == completed.change().operation_id()
                        })
                });
        let every_used_operation_accounted_for =
            self.used_operation_ids.iter().all(|operation_id| {
                self.configuration_state
                    .active_change()
                    .is_some_and(|change| change.operation_id() == operation_id)
                    || self
                        .completed_changes
                        .iter()
                        .any(|completed| completed.change().operation_id() == operation_id)
            });
        operation_ids_unique
            && active_change_valid
            && completed_changes_valid
            && every_used_operation_accounted_for
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PermissionModeEvent {
    ChangeSubmitted {
        operation_id: PermissionModeOperationId,
        expected_mode_version: u64,
        requested: PermissionModeSnapshot,
    },
    ConfigurationSucceeded {
        operation_id: PermissionModeOperationId,
        applied: PermissionModeSnapshot,
        effective_cursor: Option<OrderedProviderCursor>,
    },
    ConfigurationRejectedNotApplied {
        operation_id: PermissionModeOperationId,
    },
    ConfigurationApplicationUnknown {
        operation_id: PermissionModeOperationId,
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
