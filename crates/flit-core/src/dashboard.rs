use crate::{
    attention::{AttentionProjection, AttentionSeverity},
    lifecycle::{LifecycleProjection, RunLifecycle},
    stuck::StuckAssessment,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DashboardBucket {
    NeedsAttention,
    PossiblyStuck,
    Working,
    Finished,
}

#[must_use]
pub fn dashboard_bucket(
    lifecycle: &LifecycleProjection,
    attention: &AttentionProjection,
    stuck: &StuckAssessment,
) -> DashboardBucket {
    if matches!(
        attention.highest_active_severity(),
        Some(AttentionSeverity::ActionRequired | AttentionSeverity::Critical)
    ) {
        return DashboardBucket::NeedsAttention;
    }

    let lifecycle_state = lifecycle.lifecycle();
    if matches!(
        lifecycle_state,
        RunLifecycle::Starting | RunLifecycle::Running
    ) && matches!(stuck, StuckAssessment::PossiblyStuck(_))
    {
        return DashboardBucket::PossiblyStuck;
    }

    if matches!(
        lifecycle_state,
        RunLifecycle::Starting | RunLifecycle::Running
    ) {
        DashboardBucket::Working
    } else {
        DashboardBucket::Finished
    }
}
