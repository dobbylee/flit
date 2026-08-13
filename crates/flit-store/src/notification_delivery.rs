#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum NotificationKind {
    Permission,
    Question,
    Failure,
    Stuck,
    Completion,
}

impl NotificationKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Permission => "permission",
            Self::Question => "question",
            Self::Failure => "failure",
            Self::Stuck => "stuck",
            Self::Completion => "completion",
        }
    }

    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "permission" => Some(Self::Permission),
            "question" => Some(Self::Question),
            "failure" => Some(Self::Failure),
            "stuck" => Some(Self::Stuck),
            "completion" => Some(Self::Completion),
            _ => None,
        }
    }

    pub(crate) const fn catches_up(self) -> bool {
        !matches!(self, Self::Completion)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NotificationDeliveryState {
    Suppressed,
    Claimed,
    Delivered,
}

impl NotificationDeliveryState {
    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "suppressed" => Some(Self::Suppressed),
            "claimed" => Some(Self::Claimed),
            "delivered" => Some(Self::Delivered),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NotificationDeliveryCandidate {
    pub notification_id: String,
    pub run_id: String,
    pub run_version: u64,
    pub project_id: String,
    pub kind: NotificationKind,
    pub item_id: String,
    pub item_version: u64,
    pub platform_id: String,
    pub delivery_claimed: bool,
    pub catch_up: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NotificationDeliveryClaim {
    pub notification_id: String,
    pub run_id: String,
    pub expected_run_version: u64,
    pub kind: NotificationKind,
    pub item_id: String,
    pub item_version: u64,
    pub platform_id: String,
    pub local_minute: u16,
    pub claimed_at: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NotificationDeliveryClaimOutcome {
    Claimed,
    AlreadyClaimed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NotificationDeliveryFailure {
    pub notification_id: String,
    pub run_id: String,
    pub kind: NotificationKind,
    pub item_id: String,
    pub item_version: u64,
    pub platform_id: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NotificationDeliveryFailureOutcome {
    Released,
    AlreadyReleased,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NotificationDeliveryReceipt {
    pub notification_id: String,
    pub run_id: String,
    pub kind: NotificationKind,
    pub item_id: String,
    pub item_version: u64,
    pub platform_id: String,
    pub delivered_at: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NotificationDeliveryReceiptOutcome {
    Delivered,
    AlreadyDelivered,
}

pub(crate) fn local_minute_is_quiet(
    enabled: bool,
    start_minute: u16,
    end_minute: u16,
    local_minute: u16,
) -> bool {
    if !enabled || local_minute >= 24 * 60 || start_minute == end_minute {
        return false;
    }
    if start_minute < end_minute {
        (start_minute..end_minute).contains(&local_minute)
    } else {
        local_minute >= start_minute || local_minute < end_minute
    }
}

#[cfg(test)]
mod tests {
    use super::local_minute_is_quiet;

    #[test]
    fn quiet_hours_use_inclusive_start_and_exclusive_end_for_both_interval_shapes() {
        assert!(!local_minute_is_quiet(false, 9 * 60, 17 * 60, 9 * 60));
        assert!(!local_minute_is_quiet(true, 9 * 60, 9 * 60, 9 * 60));

        assert!(!local_minute_is_quiet(true, 9 * 60, 17 * 60, 9 * 60 - 1));
        assert!(local_minute_is_quiet(true, 9 * 60, 17 * 60, 9 * 60));
        assert!(local_minute_is_quiet(true, 9 * 60, 17 * 60, 17 * 60 - 1));
        assert!(!local_minute_is_quiet(true, 9 * 60, 17 * 60, 17 * 60));

        assert!(!local_minute_is_quiet(true, 22 * 60, 8 * 60, 22 * 60 - 1));
        assert!(local_minute_is_quiet(true, 22 * 60, 8 * 60, 22 * 60));
        assert!(local_minute_is_quiet(true, 22 * 60, 8 * 60, 8 * 60 - 1));
        assert!(!local_minute_is_quiet(true, 22 * 60, 8 * 60, 8 * 60));
    }
}
