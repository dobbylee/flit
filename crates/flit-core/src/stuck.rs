use std::{error::Error, fmt};

use crate::{
    activity::{Activity, ActivityProjection, EvidenceId, TimestampMs, WaitKind},
    lifecycle::{LifecycleProjection, RunLifecycle},
};

const MILLIS_PER_SECOND: u64 = 1_000;
const MIN_THRESHOLD_SECONDS: u16 = 30;
const MAX_THRESHOLD_SECONDS: u16 = 1_800;
const NOTIFICATION_DELAY_SECONDS: u16 = 300;
const STILL_WORKING_SUPPRESSION_SECONDS: u16 = 600;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StuckThresholdSeconds(u16);

impl StuckThresholdSeconds {
    pub fn new(value: u16) -> Result<Self, StuckError> {
        if !(MIN_THRESHOLD_SECONDS..=MAX_THRESHOLD_SECONDS).contains(&value) {
            return Err(StuckError::ThresholdOutOfRange(value));
        }
        Ok(Self(value))
    }

    #[must_use]
    pub const fn as_u16(self) -> u16 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StuckPolicy {
    starting: StuckThresholdSeconds,
    regular_activity: StuckThresholdSeconds,
    long_running_activity: StuckThresholdSeconds,
    unstructured_wait: StuckThresholdSeconds,
}

impl StuckPolicy {
    #[must_use]
    pub const fn new(
        starting: StuckThresholdSeconds,
        regular_activity: StuckThresholdSeconds,
        long_running_activity: StuckThresholdSeconds,
        unstructured_wait: StuckThresholdSeconds,
    ) -> Self {
        Self {
            starting,
            regular_activity,
            long_running_activity,
            unstructured_wait,
        }
    }

    #[must_use]
    pub const fn starting(self) -> StuckThresholdSeconds {
        self.starting
    }

    #[must_use]
    pub const fn regular_activity(self) -> StuckThresholdSeconds {
        self.regular_activity
    }

    #[must_use]
    pub const fn long_running_activity(self) -> StuckThresholdSeconds {
        self.long_running_activity
    }

    #[must_use]
    pub const fn unstructured_wait(self) -> StuckThresholdSeconds {
        self.unstructured_wait
    }
}

impl Default for StuckPolicy {
    fn default() -> Self {
        Self {
            starting: StuckThresholdSeconds(30),
            regular_activity: StuckThresholdSeconds(120),
            long_running_activity: StuckThresholdSeconds(300),
            unstructured_wait: StuckThresholdSeconds(300),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProcessState {
    NotSpawned,
    Alive,
    Unavailable,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StuckContext {
    lifecycle: RunLifecycle,
    activity: Activity,
    wait_kind: Option<WaitKind>,
    process_state: ProcessState,
    has_open_blocking_request: bool,
    last_progress_at: TimestampMs,
    last_progress_evidence_id: EvidenceId,
}

impl StuckContext {
    pub fn new(
        lifecycle: RunLifecycle,
        activity: Activity,
        wait_kind: Option<WaitKind>,
        process_state: ProcessState,
        has_open_blocking_request: bool,
        last_progress_at: TimestampMs,
        last_progress_evidence_id: EvidenceId,
    ) -> Result<Self, StuckError> {
        if (activity == Activity::Waiting) != wait_kind.is_some() {
            return Err(StuckError::InvalidWaitKind);
        }
        Ok(Self {
            lifecycle,
            activity,
            wait_kind,
            process_state,
            has_open_blocking_request,
            last_progress_at,
            last_progress_evidence_id,
        })
    }

    #[must_use]
    pub fn from_projections(
        lifecycle: &LifecycleProjection,
        activity: &ActivityProjection,
        process_state: ProcessState,
        has_open_blocking_request: bool,
    ) -> Self {
        Self {
            lifecycle: lifecycle.lifecycle(),
            activity: activity.activity(),
            wait_kind: activity.wait_kind(),
            process_state,
            has_open_blocking_request,
            last_progress_at: activity.last_progress().observed_at(),
            last_progress_evidence_id: activity.last_progress().evidence_id().clone(),
        }
    }

    #[must_use]
    pub const fn lifecycle(&self) -> RunLifecycle {
        self.lifecycle
    }

    #[must_use]
    pub const fn activity(&self) -> Activity {
        self.activity
    }

    #[must_use]
    pub const fn wait_kind(&self) -> Option<WaitKind> {
        self.wait_kind
    }

    #[must_use]
    pub const fn process_state(&self) -> ProcessState {
        self.process_state
    }

    #[must_use]
    pub const fn has_open_blocking_request(&self) -> bool {
        self.has_open_blocking_request
    }

    #[must_use]
    pub const fn last_progress_at(&self) -> TimestampMs {
        self.last_progress_at
    }

    #[must_use]
    pub fn last_progress_evidence_id(&self) -> &EvidenceId {
        &self.last_progress_evidence_id
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StuckCause {
    Starting,
    Activity(Activity),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StuckOccurrenceId {
    cause: StuckCause,
    progress_at: TimestampMs,
    progress_evidence_id: EvidenceId,
    baseline_at: TimestampMs,
    stuck_since: TimestampMs,
}

impl StuckOccurrenceId {
    #[must_use]
    pub const fn cause(&self) -> StuckCause {
        self.cause
    }

    #[must_use]
    pub const fn progress_at(&self) -> TimestampMs {
        self.progress_at
    }

    #[must_use]
    pub fn progress_evidence_id(&self) -> &EvidenceId {
        &self.progress_evidence_id
    }

    #[must_use]
    pub const fn baseline_at(&self) -> TimestampMs {
        self.baseline_at
    }

    #[must_use]
    pub const fn stuck_since(&self) -> TimestampMs {
        self.stuck_since
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StuckNotificationState {
    NotDue { due_at: TimestampMs },
    Suppressed { until: TimestampMs },
    Due,
    Delivered,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StuckOccurrence {
    id: StuckOccurrenceId,
    notification: StuckNotificationState,
}

impl StuckOccurrence {
    #[must_use]
    pub const fn id(&self) -> &StuckOccurrenceId {
        &self.id
    }

    #[must_use]
    pub const fn notification(&self) -> StuckNotificationState {
        self.notification
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StuckClearReason {
    LifecycleInactive,
    BlockingRequestOpen,
    ProcessUnavailable,
    StructuredWait(WaitKind),
    WithinDeadline {
        cause: StuckCause,
        deadline_at: TimestampMs,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StuckAssessment {
    Clear(StuckClearReason),
    PossiblyStuck(StuckOccurrence),
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct StillWorkingReset {
    cause: StuckCause,
    progress_at: TimestampMs,
    progress_evidence_id: EvidenceId,
    baseline_at: TimestampMs,
    suppress_notification_until: TimestampMs,
}

impl StillWorkingReset {
    fn matches(&self, cause: StuckCause, context: &StuckContext) -> bool {
        self.cause == cause
            && self.progress_at == context.last_progress_at
            && self.progress_evidence_id == context.last_progress_evidence_id
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct StuckProjection {
    last_action_at: Option<TimestampMs>,
    reset: Option<StillWorkingReset>,
    notification_receipt: Option<StuckOccurrenceId>,
}

impl StuckProjection {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            last_action_at: None,
            reset: None,
            notification_receipt: None,
        }
    }

    pub fn assess(
        &self,
        now: TimestampMs,
        context: &StuckContext,
        policy: StuckPolicy,
    ) -> Result<StuckAssessment, StuckError> {
        self.validate_time(now, context)?;
        let (cause, threshold) = match eligible_cause(context, policy) {
            Ok(eligible) => eligible,
            Err(reason) => return Ok(StuckAssessment::Clear(reason)),
        };
        let applicable_reset = self
            .reset
            .as_ref()
            .filter(|reset| reset.matches(cause, context));
        let baseline_at =
            applicable_reset.map_or(context.last_progress_at, |reset| reset.baseline_at);
        if now < baseline_at {
            return Err(StuckError::NonMonotonicTime {
                current: baseline_at,
                received: now,
            });
        }

        let stuck_since = add_seconds(baseline_at, threshold.as_u16())?;
        if now < stuck_since {
            return Ok(StuckAssessment::Clear(StuckClearReason::WithinDeadline {
                cause,
                deadline_at: stuck_since,
            }));
        }

        let id = StuckOccurrenceId {
            cause,
            progress_at: context.last_progress_at,
            progress_evidence_id: context.last_progress_evidence_id.clone(),
            baseline_at,
            stuck_since,
        };
        let due_at = add_seconds(stuck_since, NOTIFICATION_DELAY_SECONDS)?;
        let notification = if self.notification_receipt.as_ref() == Some(&id) {
            StuckNotificationState::Delivered
        } else if now < due_at {
            StuckNotificationState::NotDue { due_at }
        } else if let Some(reset) = applicable_reset
            && now < reset.suppress_notification_until
        {
            StuckNotificationState::Suppressed {
                until: reset.suppress_notification_until,
            }
        } else {
            StuckNotificationState::Due
        };

        Ok(StuckAssessment::PossiblyStuck(StuckOccurrence {
            id,
            notification,
        }))
    }

    pub fn still_working(
        &mut self,
        now: TimestampMs,
        context: &StuckContext,
        policy: StuckPolicy,
    ) -> Result<StuckActionDisposition, StuckError> {
        let occurrence = match self.assess(now, context, policy)? {
            StuckAssessment::PossiblyStuck(occurrence) => occurrence,
            StuckAssessment::Clear(_) => {
                return Ok(StuckActionDisposition::Ignored(
                    StuckActionIgnoredReason::NotCurrentlyStuck,
                ));
            }
        };
        let suppress_notification_until = add_seconds(now, STILL_WORKING_SUPPRESSION_SECONDS)?;
        self.reset = Some(StillWorkingReset {
            cause: occurrence.id.cause,
            progress_at: context.last_progress_at,
            progress_evidence_id: context.last_progress_evidence_id.clone(),
            baseline_at: now,
            suppress_notification_until,
        });
        self.notification_receipt = None;
        self.last_action_at = Some(now);
        Ok(StuckActionDisposition::Applied)
    }

    pub fn notification_delivered(
        &mut self,
        now: TimestampMs,
        context: &StuckContext,
        policy: StuckPolicy,
        dispatched_occurrence_id: &StuckOccurrenceId,
    ) -> Result<StuckActionDisposition, StuckError> {
        let occurrence = match self.assess(now, context, policy)? {
            StuckAssessment::Clear(_) => {
                return Ok(StuckActionDisposition::Ignored(
                    StuckActionIgnoredReason::NotCurrentlyStuck,
                ));
            }
            StuckAssessment::PossiblyStuck(occurrence) => occurrence,
        };
        if occurrence.id != *dispatched_occurrence_id {
            return Ok(StuckActionDisposition::Ignored(
                StuckActionIgnoredReason::NotificationOccurrenceMismatch,
            ));
        }
        match occurrence.notification {
            StuckNotificationState::Due => {
                self.notification_receipt = Some(occurrence.id);
                self.last_action_at = Some(now);
                Ok(StuckActionDisposition::Applied)
            }
            StuckNotificationState::Delivered => Ok(StuckActionDisposition::Ignored(
                StuckActionIgnoredReason::NotificationAlreadyDelivered,
            )),
            StuckNotificationState::NotDue { .. } | StuckNotificationState::Suppressed { .. } => {
                Ok(StuckActionDisposition::Ignored(
                    StuckActionIgnoredReason::NotificationNotDue,
                ))
            }
        }
    }

    fn validate_time(&self, now: TimestampMs, context: &StuckContext) -> Result<(), StuckError> {
        if let Some(last_action_at) = self.last_action_at
            && now < last_action_at
        {
            return Err(StuckError::NonMonotonicTime {
                current: last_action_at,
                received: now,
            });
        }
        if now < context.last_progress_at {
            return Err(StuckError::NonMonotonicTime {
                current: context.last_progress_at,
                received: now,
            });
        }
        Ok(())
    }
}

fn eligible_cause(
    context: &StuckContext,
    policy: StuckPolicy,
) -> Result<(StuckCause, StuckThresholdSeconds), StuckClearReason> {
    if context.lifecycle.is_terminal() {
        return Err(StuckClearReason::LifecycleInactive);
    }
    if context.has_open_blocking_request {
        return Err(StuckClearReason::BlockingRequestOpen);
    }
    if let Some(wait_kind) = context.wait_kind
        && matches!(
            wait_kind,
            WaitKind::BlockingRequest | WaitKind::External | WaitKind::Service
        )
    {
        return Err(StuckClearReason::StructuredWait(wait_kind));
    }

    match context.lifecycle {
        RunLifecycle::Starting => {
            if context.process_state == ProcessState::Unavailable {
                Err(StuckClearReason::ProcessUnavailable)
            } else {
                Ok((StuckCause::Starting, policy.starting))
            }
        }
        RunLifecycle::Running => {
            if context.process_state != ProcessState::Alive {
                return Err(StuckClearReason::ProcessUnavailable);
            }
            let threshold = match context.activity {
                Activity::Testing | Activity::Building => policy.long_running_activity,
                Activity::Waiting => policy.unstructured_wait,
                Activity::Planning
                | Activity::Reading
                | Activity::Editing
                | Activity::Reviewing
                | Activity::Unknown => policy.regular_activity,
            };
            Ok((StuckCause::Activity(context.activity), threshold))
        }
        lifecycle if lifecycle.is_terminal() => Err(StuckClearReason::LifecycleInactive),
        _ => unreachable!("all lifecycle variants are handled"),
    }
}

fn add_seconds(base: TimestampMs, seconds: u16) -> Result<TimestampMs, StuckError> {
    let milliseconds = u64::from(seconds) * MILLIS_PER_SECOND;
    base.as_u64()
        .checked_add(milliseconds)
        .map(TimestampMs::new)
        .ok_or(StuckError::TimestampOverflow { base, seconds })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StuckActionDisposition {
    Applied,
    Ignored(StuckActionIgnoredReason),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StuckActionIgnoredReason {
    NotCurrentlyStuck,
    NotificationNotDue,
    NotificationAlreadyDelivered,
    NotificationOccurrenceMismatch,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StuckError {
    ThresholdOutOfRange(u16),
    InvalidWaitKind,
    NonMonotonicTime {
        current: TimestampMs,
        received: TimestampMs,
    },
    TimestampOverflow {
        base: TimestampMs,
        seconds: u16,
    },
}

impl fmt::Display for StuckError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ThresholdOutOfRange(value) => write!(
                formatter,
                "stuck threshold must be between 30 and 1800 seconds: {value}"
            ),
            Self::InvalidWaitKind => {
                formatter.write_str("wait kind must be present only for Waiting activity context")
            }
            Self::NonMonotonicTime { current, received } => write!(
                formatter,
                "stuck clock must not decrease: current={}, received={}",
                current.as_u64(),
                received.as_u64()
            ),
            Self::TimestampOverflow { base, seconds } => write!(
                formatter,
                "stuck deadline overflow: base={}, seconds={seconds}",
                base.as_u64()
            ),
        }
    }
}

impl Error for StuckError {}
