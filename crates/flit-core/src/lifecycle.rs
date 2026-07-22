use std::{error::Error, fmt};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionId(String);

impl SessionId {
    pub fn new(value: impl Into<String>) -> Result<Self, DomainIdError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(DomainIdError);
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResumeIntentId(String);

impl ResumeIntentId {
    pub fn new(value: impl Into<String>) -> Result<Self, DomainIdError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(DomainIdError);
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DomainIdError;

impl fmt::Display for DomainIdError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("domain identifiers must not be blank")
    }
}

impl Error for DomainIdError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RunLifecycle {
    Starting,
    Running,
    Finished,
    Failed,
    Stopped,
    Interrupted,
}

impl RunLifecycle {
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Finished | Self::Failed | Self::Stopped | Self::Interrupted
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResumeIntent {
    id: ResumeIntentId,
    expected_version: u64,
    prior_lifecycle: RunLifecycle,
    previous_session_id: Option<SessionId>,
}

impl ResumeIntent {
    #[must_use]
    pub fn id(&self) -> &ResumeIntentId {
        &self.id
    }

    #[must_use]
    pub const fn expected_version(&self) -> u64 {
        self.expected_version
    }

    #[must_use]
    pub const fn prior_lifecycle(&self) -> RunLifecycle {
        self.prior_lifecycle
    }

    #[must_use]
    pub fn previous_session_id(&self) -> Option<&SessionId> {
        self.previous_session_id.as_ref()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LifecycleProjection {
    version: u64,
    lifecycle: RunLifecycle,
    active_session_id: Option<SessionId>,
    last_session_id: Option<SessionId>,
    session_history: Vec<SessionId>,
    resume_intent: Option<ResumeIntent>,
}

impl LifecycleProjection {
    pub fn new(run_created_ingest_seq: u64) -> Result<Self, LifecycleError> {
        if run_created_ingest_seq == 0 {
            return Err(LifecycleError::InvalidInitialIngestSequence);
        }
        Ok(Self {
            version: run_created_ingest_seq,
            lifecycle: RunLifecycle::Starting,
            active_session_id: None,
            last_session_id: None,
            session_history: Vec::new(),
            resume_intent: None,
        })
    }

    #[must_use]
    pub const fn version(&self) -> u64 {
        self.version
    }

    #[must_use]
    pub const fn lifecycle(&self) -> RunLifecycle {
        self.lifecycle
    }

    #[must_use]
    pub fn active_session_id(&self) -> Option<&SessionId> {
        self.active_session_id.as_ref()
    }

    #[must_use]
    pub fn last_session_id(&self) -> Option<&SessionId> {
        self.last_session_id.as_ref()
    }

    #[must_use]
    pub fn session_history(&self) -> &[SessionId] {
        &self.session_history
    }

    #[must_use]
    pub fn resume_intent(&self) -> Option<&ResumeIntent> {
        self.resume_intent.as_ref()
    }

    pub fn apply(
        &mut self,
        ingest_seq: u64,
        event: LifecycleEvent,
    ) -> Result<LifecycleDisposition, LifecycleError> {
        if ingest_seq <= self.version {
            return Err(LifecycleError::NonMonotonicIngestSequence {
                current: self.version,
                received: ingest_seq,
            });
        }

        let disposition = self.apply_ordered(event);
        self.version = ingest_seq;
        debug_assert!(self.invariants_hold());
        Ok(disposition)
    }

    fn apply_ordered(&mut self, event: LifecycleEvent) -> LifecycleDisposition {
        match event {
            LifecycleEvent::SessionConnected { session_id } => {
                self.connect_initial_session(session_id)
            }
            LifecycleEvent::RunEventObserved => LifecycleDisposition::Applied,
            LifecycleEvent::RunCompleted => self.enter_terminal(RunLifecycle::Finished, false),
            LifecycleEvent::RunFailed => self.enter_terminal(RunLifecycle::Failed, true),
            LifecycleEvent::RunStopped => self.enter_terminal(RunLifecycle::Stopped, true),
            LifecycleEvent::RunInterrupted => self.enter_terminal(RunLifecycle::Interrupted, false),
            LifecycleEvent::ResumeRequested {
                intent_id,
                expected_version,
            } => self.request_resume(intent_id, expected_version),
            LifecycleEvent::SessionResumed {
                intent_id,
                session_id,
            } => self.resume_session(intent_id, session_id),
            LifecycleEvent::ResumeFailed { intent_id } => self.fail_resume(intent_id),
        }
    }

    fn connect_initial_session(&mut self, session_id: SessionId) -> LifecycleDisposition {
        match self.lifecycle {
            RunLifecycle::Starting => {
                self.session_history.push(session_id.clone());
                self.active_session_id = Some(session_id);
                self.lifecycle = RunLifecycle::Running;
                LifecycleDisposition::Applied
            }
            RunLifecycle::Running => {
                if self.active_session_id.as_ref() == Some(&session_id) {
                    LifecycleDisposition::Ignored(
                        IgnoredLifecycleReason::DuplicateSessionConnection,
                    )
                } else {
                    LifecycleDisposition::Ignored(IgnoredLifecycleReason::LiveSessionAlreadyExists)
                }
            }
            lifecycle if lifecycle.is_terminal() => {
                LifecycleDisposition::Ignored(IgnoredLifecycleReason::TerminalStateRequiresResume)
            }
            _ => unreachable!("all lifecycle variants are handled"),
        }
    }

    fn enter_terminal(
        &mut self,
        target: RunLifecycle,
        allowed_from_starting: bool,
    ) -> LifecycleDisposition {
        if self.lifecycle.is_terminal() {
            return if self.lifecycle == target {
                LifecycleDisposition::Ignored(IgnoredLifecycleReason::DuplicateTerminalEvent)
            } else {
                LifecycleDisposition::Ignored(IgnoredLifecycleReason::FirstTerminalStatePreserved)
            };
        }

        if self.lifecycle == RunLifecycle::Starting && !allowed_from_starting {
            return LifecycleDisposition::Ignored(
                IgnoredLifecycleReason::TransitionRequiresRunning,
            );
        }

        if let Some(session_id) = self.active_session_id.take() {
            self.last_session_id = Some(session_id);
        }
        self.lifecycle = target;
        LifecycleDisposition::Applied
    }

    fn request_resume(
        &mut self,
        intent_id: ResumeIntentId,
        expected_version: u64,
    ) -> LifecycleDisposition {
        if !self.lifecycle.is_terminal() {
            return LifecycleDisposition::Ignored(IgnoredLifecycleReason::ResumeRequiresTerminal);
        }
        if expected_version != self.version {
            return LifecycleDisposition::Ignored(IgnoredLifecycleReason::StaleExpectedVersion);
        }
        if self.active_session_id.is_some() {
            return LifecycleDisposition::Ignored(IgnoredLifecycleReason::LiveSessionAlreadyExists);
        }
        if self.resume_intent.is_some() {
            return LifecycleDisposition::Ignored(IgnoredLifecycleReason::ResumeAlreadyPending);
        }

        self.resume_intent = Some(ResumeIntent {
            id: intent_id,
            expected_version,
            prior_lifecycle: self.lifecycle,
            previous_session_id: self.last_session_id.clone(),
        });
        LifecycleDisposition::Applied
    }

    fn resume_session(
        &mut self,
        intent_id: ResumeIntentId,
        session_id: SessionId,
    ) -> LifecycleDisposition {
        let Some(intent) = self.resume_intent.as_ref() else {
            return LifecycleDisposition::Ignored(IgnoredLifecycleReason::MissingResumeIntent);
        };
        if intent.id != intent_id {
            return LifecycleDisposition::Ignored(IgnoredLifecycleReason::MismatchedResumeIntent);
        }
        if self.session_history.contains(&session_id) {
            return LifecycleDisposition::Ignored(IgnoredLifecycleReason::PreviousSessionReused);
        }

        self.resume_intent = None;
        self.session_history.push(session_id.clone());
        self.active_session_id = Some(session_id);
        self.lifecycle = RunLifecycle::Running;
        LifecycleDisposition::Applied
    }

    fn fail_resume(&mut self, intent_id: ResumeIntentId) -> LifecycleDisposition {
        let Some(intent) = self.resume_intent.as_ref() else {
            return LifecycleDisposition::Ignored(IgnoredLifecycleReason::MissingResumeIntent);
        };
        if intent.id != intent_id {
            return LifecycleDisposition::Ignored(IgnoredLifecycleReason::MismatchedResumeIntent);
        }

        self.resume_intent = None;
        LifecycleDisposition::Applied
    }

    fn invariants_hold(&self) -> bool {
        let session_consistent = match self.lifecycle {
            RunLifecycle::Running => self.active_session_id.is_some(),
            RunLifecycle::Starting => self.active_session_id.is_none(),
            lifecycle if lifecycle.is_terminal() => self.active_session_id.is_none(),
            _ => false,
        };
        let intent_consistent = self.resume_intent.is_none() || self.lifecycle.is_terminal();
        let history_consistent = self
            .session_history
            .iter()
            .enumerate()
            .all(|(index, session_id)| !self.session_history[..index].contains(session_id))
            && self
                .active_session_id
                .as_ref()
                .is_none_or(|session_id| self.session_history.contains(session_id))
            && self
                .last_session_id
                .as_ref()
                .is_none_or(|session_id| self.session_history.contains(session_id));
        session_consistent && intent_consistent && history_consistent
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LifecycleEvent {
    RunEventObserved,
    SessionConnected {
        session_id: SessionId,
    },
    RunCompleted,
    RunFailed,
    RunStopped,
    RunInterrupted,
    ResumeRequested {
        intent_id: ResumeIntentId,
        expected_version: u64,
    },
    SessionResumed {
        intent_id: ResumeIntentId,
        session_id: SessionId,
    },
    ResumeFailed {
        intent_id: ResumeIntentId,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LifecycleDisposition {
    Applied,
    Ignored(IgnoredLifecycleReason),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IgnoredLifecycleReason {
    DuplicateSessionConnection,
    LiveSessionAlreadyExists,
    TerminalStateRequiresResume,
    TransitionRequiresRunning,
    DuplicateTerminalEvent,
    FirstTerminalStatePreserved,
    ResumeRequiresTerminal,
    StaleExpectedVersion,
    ResumeAlreadyPending,
    MissingResumeIntent,
    MismatchedResumeIntent,
    PreviousSessionReused,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LifecycleError {
    InvalidInitialIngestSequence,
    NonMonotonicIngestSequence { current: u64, received: u64 },
}

impl fmt::Display for LifecycleError {
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

impl Error for LifecycleError {}

pub fn replay_lifecycle<I>(
    run_created_ingest_seq: u64,
    events: I,
) -> Result<LifecycleProjection, LifecycleError>
where
    I: IntoIterator<Item = (u64, LifecycleEvent)>,
{
    let mut projection = LifecycleProjection::new(run_created_ingest_seq)?;
    for (ingest_seq, event) in events {
        projection.apply(ingest_seq, event)?;
    }
    Ok(projection)
}
