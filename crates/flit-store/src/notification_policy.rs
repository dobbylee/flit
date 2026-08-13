use serde::{Deserialize, Serialize};

pub const NOTIFICATION_POLICY_SETTINGS_KEY: &str = "notification_policy";
pub const MAX_NOTIFICATION_POLICY_JSON_BYTES: usize = 16 * 1_024;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct NotificationKinds {
    pub permission: bool,
    pub question: bool,
    pub failure: bool,
    pub completion: bool,
    pub stuck: bool,
}

impl Default for NotificationKinds {
    fn default() -> Self {
        Self {
            permission: true,
            question: true,
            failure: true,
            completion: false,
            stuck: true,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct QuietHours {
    pub enabled: bool,
    pub start_minute: u16,
    pub end_minute: u16,
}

impl Default for QuietHours {
    fn default() -> Self {
        Self {
            enabled: false,
            start_minute: 22 * 60,
            end_minute: 8 * 60,
        }
    }
}

impl QuietHours {
    pub(crate) fn validate(self) -> Result<(), &'static str> {
        if self.start_minute >= 24 * 60 {
            return Err("quiet_hours.start_minute");
        }
        if self.end_minute >= 24 * 60 {
            return Err("quiet_hours.end_minute");
        }
        if self.enabled && self.start_minute == self.end_minute {
            return Err("quiet_hours.interval");
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GlobalNotificationPolicy {
    pub version: u64,
    pub kinds: NotificationKinds,
    pub quiet_hours: QuietHours,
}

impl GlobalNotificationPolicy {
    pub(crate) fn validate(&self) -> Result<(), &'static str> {
        self.quiet_hours.validate()
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NotificationOverride {
    #[default]
    Inherit,
    On,
    Off,
}

impl NotificationOverride {
    fn resolve(self, inherited: bool) -> bool {
        match self {
            Self::Inherit => inherited,
            Self::On => true,
            Self::Off => false,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectNotificationMaster {
    #[default]
    Inherit,
    Off,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct NotificationKindOverrides {
    pub permission: NotificationOverride,
    pub question: NotificationOverride,
    pub failure: NotificationOverride,
    pub completion: NotificationOverride,
    pub stuck: NotificationOverride,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectNotificationPolicy {
    pub version: u64,
    pub master: ProjectNotificationMaster,
    pub kinds: NotificationKindOverrides,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EffectiveNotificationPolicy {
    pub global_version: u64,
    pub project_version: Option<u64>,
    pub kinds: NotificationKinds,
    pub quiet_hours: QuietHours,
}

impl EffectiveNotificationPolicy {
    #[must_use]
    pub fn resolve(
        global: &GlobalNotificationPolicy,
        project: Option<&ProjectNotificationPolicy>,
    ) -> Self {
        let kinds = match project {
            None => global.kinds,
            Some(project) if project.master == ProjectNotificationMaster::Off => {
                NotificationKinds {
                    permission: false,
                    question: false,
                    failure: false,
                    completion: false,
                    stuck: false,
                }
            }
            Some(project) => NotificationKinds {
                permission: project.kinds.permission.resolve(global.kinds.permission),
                question: project.kinds.question.resolve(global.kinds.question),
                failure: project.kinds.failure.resolve(global.kinds.failure),
                completion: project.kinds.completion.resolve(global.kinds.completion),
                stuck: project.kinds.stuck.resolve(global.kinds.stuck),
            },
        };
        Self {
            global_version: global.version,
            project_version: project.map(|policy| policy.version),
            kinds,
            quiet_hours: global.quiet_hours,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NotificationPolicySnapshot {
    pub global: GlobalNotificationPolicy,
    pub project: Option<ProjectNotificationPolicy>,
    pub effective: EffectiveNotificationPolicy,
}

impl NotificationPolicySnapshot {
    #[must_use]
    pub fn new(
        global: GlobalNotificationPolicy,
        project: Option<ProjectNotificationPolicy>,
    ) -> Self {
        let effective = EffectiveNotificationPolicy::resolve(&global, project.as_ref());
        Self {
            global,
            project,
            effective,
        }
    }
}
