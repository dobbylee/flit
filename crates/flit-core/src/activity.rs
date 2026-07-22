use std::{error::Error, fmt};

const SCORE_SCALE: u64 = 1_000;
const CORROBORATION_WINDOW_MS: u64 = 500;
const ACTIVITY_HYSTERESIS_MS: u64 = 2_000;
const INACTIVITY_TIMEOUT_MS: u64 = 60_000;
const MAX_PENDING_CORROBORATION: usize = 42;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct TimestampMs(u64);

impl TimestampMs {
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn as_u64(self) -> u64 {
        self.0
    }

    const fn elapsed_since(self, earlier: Self) -> u64 {
        self.0 - earlier.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ScoreFactor(u16);

impl ScoreFactor {
    pub fn new(value: u16) -> Result<Self, ActivityValueError> {
        if value > SCORE_SCALE as u16 {
            return Err(ActivityValueError::ScoreFactorOutOfRange(value));
        }
        Ok(Self(value))
    }

    #[must_use]
    pub const fn as_milli(self) -> u16 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ConfidenceScore(u16);

impl ConfidenceScore {
    #[must_use]
    pub const fn as_milli(self) -> u16 {
        self.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EvidenceId(String);

impl EvidenceId {
    pub fn new(value: impl Into<String>) -> Result<Self, ActivityValueError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(ActivityValueError::BlankEvidenceId);
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Activity {
    Planning,
    Reading,
    Editing,
    Testing,
    Building,
    Reviewing,
    Waiting,
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SignalSource {
    StructuredActivity,
    BlockingRequest,
    KnownCommand,
    TestBuildMarker,
    FileChangePattern,
    UnstructuredProviderText,
}

impl SignalSource {
    #[must_use]
    pub const fn base_reliability(self) -> ConfidenceScore {
        ConfidenceScore(match self {
            Self::StructuredActivity => 1_000,
            Self::BlockingRequest => 950,
            Self::KnownCommand => 850,
            Self::TestBuildMarker => 750,
            Self::FileChangePattern => 550,
            Self::UnstructuredProviderText => 400,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WaitKind {
    Unstructured,
    BlockingRequest,
    External,
    Service,
}

impl WaitKind {
    const fn holds_inactivity(self) -> bool {
        matches!(self, Self::BlockingRequest | Self::External | Self::Service)
    }

    const fn corroboration_priority(self) -> u8 {
        match self {
            Self::Unstructured => 0,
            Self::Service => 1,
            Self::External => 2,
            Self::BlockingRequest => 3,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActivitySignal {
    activity: Activity,
    source: SignalSource,
    recency: ScoreFactor,
    specificity: ScoreFactor,
    evidence_id: EvidenceId,
    wait_kind: Option<WaitKind>,
}

impl ActivitySignal {
    pub fn new(
        activity: Activity,
        source: SignalSource,
        recency: ScoreFactor,
        specificity: ScoreFactor,
        evidence_id: EvidenceId,
        wait_kind: Option<WaitKind>,
    ) -> Result<Self, ActivityValueError> {
        if activity == Activity::Unknown {
            return Err(ActivityValueError::InvalidUnknownSignal);
        }
        if (activity == Activity::Waiting) != wait_kind.is_some() {
            return Err(ActivityValueError::InvalidWaitKind);
        }
        if source == SignalSource::BlockingRequest
            && (activity != Activity::Waiting || wait_kind != Some(WaitKind::BlockingRequest))
        {
            return Err(ActivityValueError::InvalidBlockingRequestSignal);
        }
        Ok(Self {
            activity,
            source,
            recency,
            specificity,
            evidence_id,
            wait_kind,
        })
    }

    #[must_use]
    pub fn score(&self) -> ConfidenceScore {
        let base = u64::from(self.source.base_reliability().as_milli());
        let recency = u64::from(self.recency.as_milli());
        let specificity = u64::from(self.specificity.as_milli());
        ConfidenceScore(((base * recency * specificity) / SCORE_SCALE.pow(2)) as u16)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProgressKind {
    CommandStarted,
    CommandFinished,
    FileContentChanged,
    TestBuildStageChanged,
    AdapterStepChanged,
    AgentOutputResumed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ActivityEvent {
    Signal(ActivitySignal),
    MeaningfulProgress {
        kind: ProgressKind,
        evidence_id: EvidenceId,
    },
    LivenessObserved {
        evidence_id: EvidenceId,
    },
    Tick {
        evidence_id: EvidenceId,
    },
    LifecycleTerminated {
        evidence_id: EvidenceId,
    },
    LifecycleActivated {
        evidence_id: EvidenceId,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TimedEvidence {
    observed_at: TimestampMs,
    evidence_id: EvidenceId,
}

impl TimedEvidence {
    #[must_use]
    pub const fn observed_at(&self) -> TimestampMs {
        self.observed_at
    }

    #[must_use]
    pub fn evidence_id(&self) -> &EvidenceId {
        &self.evidence_id
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct CandidateObservation {
    activity: Activity,
    source: SignalSource,
    score: ConfidenceScore,
    wait_kind: Option<WaitKind>,
    evidence_id: EvidenceId,
    observed_at: TimestampMs,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ConfirmedActivity {
    activity: Activity,
    score: ConfidenceScore,
    wait_kind: Option<WaitKind>,
    evidence_ids: Vec<EvidenceId>,
    bypasses_hysteresis: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActivityProjection {
    version: u64,
    last_event_at: TimestampMs,
    activity: Activity,
    confidence: Option<ConfidenceScore>,
    activity_evidence_ids: Vec<EvidenceId>,
    wait_kind: Option<WaitKind>,
    last_activity_changed_at: TimestampMs,
    last_meaningful_signal: TimedEvidence,
    last_progress: TimedEvidence,
    last_liveness: TimedEvidence,
    last_progress_kind: Option<ProgressKind>,
    pending_corroboration: Vec<CandidateObservation>,
    pending_transition: Option<ConfirmedActivity>,
    terminal: bool,
}

impl ActivityProjection {
    pub fn new(
        run_created_ingest_seq: u64,
        created_at: TimestampMs,
        evidence_id: EvidenceId,
    ) -> Result<Self, ActivityError> {
        if run_created_ingest_seq == 0 {
            return Err(ActivityError::InvalidInitialIngestSequence);
        }
        let initial_evidence = TimedEvidence {
            observed_at: created_at,
            evidence_id: evidence_id.clone(),
        };
        Ok(Self {
            version: run_created_ingest_seq,
            last_event_at: created_at,
            activity: Activity::Unknown,
            confidence: None,
            activity_evidence_ids: vec![evidence_id],
            wait_kind: None,
            last_activity_changed_at: created_at,
            last_meaningful_signal: initial_evidence.clone(),
            last_progress: initial_evidence.clone(),
            last_liveness: initial_evidence,
            last_progress_kind: None,
            pending_corroboration: Vec::new(),
            pending_transition: None,
            terminal: false,
        })
    }

    #[must_use]
    pub const fn version(&self) -> u64 {
        self.version
    }

    #[must_use]
    pub const fn activity(&self) -> Activity {
        self.activity
    }

    #[must_use]
    pub const fn confidence(&self) -> Option<ConfidenceScore> {
        self.confidence
    }

    #[must_use]
    pub fn activity_evidence_ids(&self) -> &[EvidenceId] {
        &self.activity_evidence_ids
    }

    #[must_use]
    pub const fn wait_kind(&self) -> Option<WaitKind> {
        self.wait_kind
    }

    #[must_use]
    pub const fn last_activity_changed_at(&self) -> TimestampMs {
        self.last_activity_changed_at
    }

    #[must_use]
    pub const fn last_meaningful_signal(&self) -> &TimedEvidence {
        &self.last_meaningful_signal
    }

    #[must_use]
    pub const fn last_progress(&self) -> &TimedEvidence {
        &self.last_progress
    }

    #[must_use]
    pub const fn last_liveness(&self) -> &TimedEvidence {
        &self.last_liveness
    }

    #[must_use]
    pub const fn last_progress_kind(&self) -> Option<ProgressKind> {
        self.last_progress_kind
    }

    #[must_use]
    pub const fn is_terminal(&self) -> bool {
        self.terminal
    }

    pub fn apply(
        &mut self,
        ingest_seq: u64,
        observed_at: TimestampMs,
        event: ActivityEvent,
    ) -> Result<ActivityDisposition, ActivityError> {
        if ingest_seq <= self.version {
            return Err(ActivityError::NonMonotonicIngestSequence {
                current: self.version,
                received: ingest_seq,
            });
        }
        if observed_at < self.last_event_at {
            return Err(ActivityError::NonMonotonicTimestamp {
                current: self.last_event_at,
                received: observed_at,
            });
        }

        let disposition = self.apply_ordered(observed_at, event);
        self.version = ingest_seq;
        self.last_event_at = observed_at;
        debug_assert!(self.invariants_hold());
        Ok(disposition)
    }

    fn apply_ordered(
        &mut self,
        observed_at: TimestampMs,
        event: ActivityEvent,
    ) -> ActivityDisposition {
        match event {
            ActivityEvent::Signal(signal) => self.apply_signal(observed_at, signal),
            ActivityEvent::MeaningfulProgress { kind, evidence_id } => {
                self.record_progress(observed_at, kind, evidence_id)
            }
            ActivityEvent::LivenessObserved { evidence_id } => {
                self.last_liveness = TimedEvidence {
                    observed_at,
                    evidence_id,
                };
                ActivityDisposition::LivenessRecorded
            }
            ActivityEvent::Tick { evidence_id } => self.apply_tick(observed_at, evidence_id),
            ActivityEvent::LifecycleTerminated { evidence_id } => {
                self.terminal = true;
                self.pending_corroboration.clear();
                self.pending_transition = None;
                self.transition_to_unknown(observed_at, evidence_id);
                ActivityDisposition::ActivityChanged
            }
            ActivityEvent::LifecycleActivated { evidence_id } => {
                self.terminal = false;
                self.pending_corroboration.clear();
                self.pending_transition = None;
                self.transition_to_unknown(observed_at, evidence_id);
                ActivityDisposition::ActivityChanged
            }
        }
    }

    fn apply_signal(
        &mut self,
        observed_at: TimestampMs,
        signal: ActivitySignal,
    ) -> ActivityDisposition {
        self.last_liveness = TimedEvidence {
            observed_at,
            evidence_id: signal.evidence_id.clone(),
        };
        let score = signal.score();
        if score.as_milli() < 700 || self.terminal {
            return ActivityDisposition::ObservedOnly;
        }
        self.last_meaningful_signal = TimedEvidence {
            observed_at,
            evidence_id: signal.evidence_id.clone(),
        };

        let candidate = CandidateObservation {
            activity: signal.activity,
            source: signal.source,
            score,
            wait_kind: signal.wait_kind,
            evidence_id: signal.evidence_id,
            observed_at,
        };
        if score.as_milli() >= 900 {
            self.pending_corroboration.clear();
            return self.confirm_activity(candidate.into_confirmed(), observed_at);
        }

        self.pending_corroboration.retain(|previous| {
            observed_at.elapsed_since(previous.observed_at) <= CORROBORATION_WINDOW_MS
        });
        let corroborated_index = self.pending_corroboration.iter().rposition(|previous| {
            previous.activity == candidate.activity
                && previous.source != candidate.source
                && previous.evidence_id != candidate.evidence_id
        });
        if let Some(corroborated_index) = corroborated_index {
            let previous = self.pending_corroboration.remove(corroborated_index);
            let confirmed = ConfirmedActivity {
                activity: candidate.activity,
                score: previous.score.max(candidate.score),
                wait_kind: merge_wait_kinds(previous.wait_kind, candidate.wait_kind),
                evidence_ids: vec![previous.evidence_id, candidate.evidence_id],
                bypasses_hysteresis: previous.source == SignalSource::BlockingRequest
                    || candidate.source == SignalSource::BlockingRequest,
            };
            self.pending_corroboration.clear();
            return self.confirm_activity(confirmed, observed_at);
        }

        self.pending_corroboration.retain(|previous| {
            previous.activity != candidate.activity || previous.source != candidate.source
        });
        self.pending_corroboration.push(candidate);
        ActivityDisposition::PendingCorroboration
    }

    fn confirm_activity(
        &mut self,
        confirmed: ConfirmedActivity,
        observed_at: TimestampMs,
    ) -> ActivityDisposition {
        if self.activity == confirmed.activity {
            self.confidence = Some(confirmed.score);
            self.activity_evidence_ids = confirmed.evidence_ids;
            self.wait_kind = confirmed.wait_kind;
            self.pending_transition = None;
            return ActivityDisposition::ActivityReinforced;
        }

        let held_long_enough =
            observed_at.elapsed_since(self.last_activity_changed_at) >= ACTIVITY_HYSTERESIS_MS;
        if self.activity == Activity::Unknown || confirmed.bypasses_hysteresis || held_long_enough {
            self.apply_confirmed_activity(confirmed, observed_at);
            ActivityDisposition::ActivityChanged
        } else {
            self.pending_transition = Some(confirmed);
            ActivityDisposition::TransitionDeferred
        }
    }

    fn apply_confirmed_activity(&mut self, confirmed: ConfirmedActivity, observed_at: TimestampMs) {
        let progress_evidence = confirmed
            .evidence_ids
            .last()
            .expect("confirmed activity has evidence")
            .clone();
        self.activity = confirmed.activity;
        self.confidence = Some(confirmed.score);
        self.activity_evidence_ids = confirmed.evidence_ids;
        self.wait_kind = confirmed.wait_kind;
        self.last_activity_changed_at = observed_at;
        self.last_progress = TimedEvidence {
            observed_at,
            evidence_id: progress_evidence,
        };
        self.last_progress_kind = None;
        self.pending_transition = None;
    }

    fn record_progress(
        &mut self,
        observed_at: TimestampMs,
        kind: ProgressKind,
        evidence_id: EvidenceId,
    ) -> ActivityDisposition {
        let observation = TimedEvidence {
            observed_at,
            evidence_id,
        };
        self.last_meaningful_signal = observation.clone();
        self.last_progress = observation.clone();
        self.last_liveness = observation;
        self.last_progress_kind = Some(kind);
        if self.activity == Activity::Waiting {
            self.wait_kind = Some(WaitKind::Unstructured);
        }
        ActivityDisposition::ProgressRecorded
    }

    fn apply_tick(
        &mut self,
        observed_at: TimestampMs,
        evidence_id: EvidenceId,
    ) -> ActivityDisposition {
        if self.terminal {
            return ActivityDisposition::NoChange;
        }
        self.pending_corroboration.retain(|candidate| {
            observed_at.elapsed_since(candidate.observed_at) <= CORROBORATION_WINDOW_MS
        });

        let holds_inactivity = self.wait_kind.is_some_and(WaitKind::holds_inactivity);
        if !holds_inactivity
            && self.activity != Activity::Unknown
            && observed_at.elapsed_since(self.last_meaningful_signal.observed_at)
                >= INACTIVITY_TIMEOUT_MS
        {
            self.pending_transition = None;
            self.transition_to_unknown(observed_at, evidence_id);
            return ActivityDisposition::ActivityChanged;
        }

        if observed_at.elapsed_since(self.last_activity_changed_at) >= ACTIVITY_HYSTERESIS_MS
            && let Some(pending) = self.pending_transition.take()
        {
            self.apply_confirmed_activity(pending, observed_at);
            return ActivityDisposition::ActivityChanged;
        }
        ActivityDisposition::NoChange
    }

    fn transition_to_unknown(&mut self, observed_at: TimestampMs, evidence_id: EvidenceId) {
        self.activity = Activity::Unknown;
        self.confidence = None;
        self.activity_evidence_ids = vec![evidence_id];
        self.wait_kind = None;
        self.last_activity_changed_at = observed_at;
    }

    fn invariants_hold(&self) -> bool {
        let evidence_consistent = !self.activity_evidence_ids.is_empty()
            && self
                .activity_evidence_ids
                .iter()
                .all(|evidence_id| !evidence_id.as_str().trim().is_empty());
        let wait_consistent = (self.activity == Activity::Waiting) == self.wait_kind.is_some();
        let unknown_consistent = self.activity != Activity::Unknown || self.confidence.is_none();
        let terminal_consistent = !self.terminal || self.activity == Activity::Unknown;
        let corroboration_bounded = self.pending_corroboration.len() <= MAX_PENDING_CORROBORATION;
        evidence_consistent
            && wait_consistent
            && unknown_consistent
            && terminal_consistent
            && corroboration_bounded
    }
}

fn merge_wait_kinds(left: Option<WaitKind>, right: Option<WaitKind>) -> Option<WaitKind> {
    match (left, right) {
        (Some(left), Some(right)) => Some(
            if left.corroboration_priority() >= right.corroboration_priority() {
                left
            } else {
                right
            },
        ),
        (Some(wait_kind), None) | (None, Some(wait_kind)) => Some(wait_kind),
        (None, None) => None,
    }
}

impl CandidateObservation {
    fn into_confirmed(self) -> ConfirmedActivity {
        ConfirmedActivity {
            activity: self.activity,
            score: self.score,
            wait_kind: self.wait_kind,
            evidence_ids: vec![self.evidence_id],
            bypasses_hysteresis: self.source == SignalSource::BlockingRequest,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ActivityDisposition {
    ActivityChanged,
    ActivityReinforced,
    ProgressRecorded,
    LivenessRecorded,
    PendingCorroboration,
    TransitionDeferred,
    ObservedOnly,
    NoChange,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ActivityValueError {
    BlankEvidenceId,
    ScoreFactorOutOfRange(u16),
    InvalidUnknownSignal,
    InvalidWaitKind,
    InvalidBlockingRequestSignal,
}

impl fmt::Display for ActivityValueError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BlankEvidenceId => formatter.write_str("evidence identifiers must not be blank"),
            Self::ScoreFactorOutOfRange(value) => {
                write!(formatter, "score factor must be at most 1000: {value}")
            }
            Self::InvalidUnknownSignal => {
                formatter.write_str("Unknown activity is reserved for reducer transitions")
            }
            Self::InvalidWaitKind => {
                formatter.write_str("wait kind must be present only for Waiting activity")
            }
            Self::InvalidBlockingRequestSignal => formatter.write_str(
                "blocking request source must classify Waiting with BlockingRequest wait kind",
            ),
        }
    }
}

impl Error for ActivityValueError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ActivityError {
    InvalidInitialIngestSequence,
    NonMonotonicIngestSequence {
        current: u64,
        received: u64,
    },
    NonMonotonicTimestamp {
        current: TimestampMs,
        received: TimestampMs,
    },
}

impl fmt::Display for ActivityError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidInitialIngestSequence => {
                formatter.write_str("initial ingest sequence must be greater than zero")
            }
            Self::NonMonotonicIngestSequence { current, received } => write!(
                formatter,
                "ingest sequence must increase: current={current}, received={received}"
            ),
            Self::NonMonotonicTimestamp { current, received } => write!(
                formatter,
                "observation time must not decrease: current={}, received={}",
                current.as_u64(),
                received.as_u64()
            ),
        }
    }
}

impl Error for ActivityError {}

pub fn replay_activity<I>(
    run_created_ingest_seq: u64,
    created_at: TimestampMs,
    creation_evidence_id: EvidenceId,
    events: I,
) -> Result<ActivityProjection, ActivityError>
where
    I: IntoIterator<Item = (u64, TimestampMs, ActivityEvent)>,
{
    let mut projection =
        ActivityProjection::new(run_created_ingest_seq, created_at, creation_evidence_id)?;
    for (ingest_seq, observed_at, event) in events {
        projection.apply(ingest_seq, observed_at, event)?;
    }
    Ok(projection)
}
