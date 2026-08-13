use std::{
    fs,
    path::PathBuf,
    process,
    sync::atomic::{AtomicU64, Ordering},
};

use flit_store::{
    GlobalNotificationPolicy, NotificationKindOverrides, NotificationKinds, NotificationOverride,
    ProjectNotificationMaster, ProjectRegistration, ProjectRegistrationOutcome, QuietHours, Store,
    StoreError,
};
use rusqlite::{Connection, params};

const APPLIED_AT: &str = "2026-08-13T00:00:00.000Z";
static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(1);

struct TestWorkspace {
    directory: PathBuf,
    database_path: PathBuf,
    project_path: PathBuf,
}

impl TestWorkspace {
    fn new(label: &str) -> Self {
        let nonce = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let directory = std::env::temp_dir().join(format!(
            "flit-notification-policy-{label}-{}-{nonce}",
            process::id()
        ));
        fs::create_dir(&directory).expect("unique test directory");
        let project_path = directory.join("project");
        fs::create_dir(&project_path).expect("Project directory");
        let database_path = directory.join("flit.sqlite3");
        Self {
            directory,
            database_path,
            project_path,
        }
    }

    fn open(&self) -> Store {
        Store::open(&self.database_path, APPLIED_AT).expect("open Store")
    }

    fn register(&self, store: &mut Store) {
        assert!(matches!(
            store
                .register_project(ProjectRegistration {
                    id: "project-policy".to_owned(),
                    display_name: "Policy Project".to_owned(),
                    selected_path: self.project_path.clone(),
                    created_at: APPLIED_AT.to_owned(),
                })
                .expect("register Project"),
            ProjectRegistrationOutcome::Registered(_)
        ));
    }
}

impl Drop for TestWorkspace {
    fn drop(&mut self) {
        if let Err(error) = fs::remove_dir_all(&self.directory)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            eprintln!(
                "failed to remove test directory {}: {error}",
                self.directory.display()
            );
        }
    }
}

#[test]
fn defaults_are_quiet_and_projects_inherit_them() {
    let workspace = TestWorkspace::new("defaults");
    let mut store = workspace.open();
    self::TestWorkspace::register(&workspace, &mut store);

    let global = store.notification_policy(None).expect("global defaults");
    assert_eq!(global.global, GlobalNotificationPolicy::default());
    assert_eq!(global.project, None);
    assert_eq!(global.effective.kinds, NotificationKinds::default());
    assert_eq!(global.effective.quiet_hours, QuietHours::default());

    let project = store
        .notification_policy(Some("project-policy"))
        .expect("Project defaults");
    assert_eq!(project.project.expect("Project policy").version, 0);
    assert_eq!(project.effective.kinds, NotificationKinds::default());
    assert_eq!(project.effective.project_version, Some(0));
}

#[test]
fn global_and_project_updates_resolve_exact_precedence_and_persist() {
    let workspace = TestWorkspace::new("precedence");
    let mut store = workspace.open();
    workspace.register(&mut store);

    let global_kinds = NotificationKinds {
        permission: false,
        question: true,
        failure: false,
        completion: true,
        stuck: true,
    };
    let quiet_hours = QuietHours {
        enabled: true,
        start_minute: 21 * 60 + 30,
        end_minute: 7 * 60,
    };
    let global = store
        .update_global_notification_policy(0, global_kinds, quiet_hours, "2026-08-13T00:01:00.000Z")
        .expect("update global policy");
    assert_eq!(global.global.version, 1);
    assert_eq!(global.effective.kinds, global_kinds);

    let overrides = NotificationKindOverrides {
        permission: NotificationOverride::On,
        question: NotificationOverride::Off,
        failure: NotificationOverride::Inherit,
        completion: NotificationOverride::Off,
        stuck: NotificationOverride::Inherit,
    };
    let project = store
        .update_project_notification_policy(
            "project-policy",
            0,
            ProjectNotificationMaster::Inherit,
            overrides,
            "2026-08-13T00:02:00.000Z",
        )
        .expect("update Project policy");
    assert_eq!(project.global.version, 1);
    assert_eq!(project.project.as_ref().expect("Project policy").version, 1);
    assert_eq!(
        project.effective.kinds,
        NotificationKinds {
            permission: true,
            question: false,
            failure: false,
            completion: false,
            stuck: true,
        }
    );
    assert_eq!(project.effective.quiet_hours, quiet_hours);
    drop(store);

    let mut reopened = workspace.open();
    assert_eq!(
        reopened
            .notification_policy(Some("project-policy"))
            .expect("reopened policy"),
        project
    );
    let off = reopened
        .update_project_notification_policy(
            "project-policy",
            1,
            ProjectNotificationMaster::Off,
            overrides,
            "2026-08-13T00:03:00.000Z",
        )
        .expect("disable Project notifications");
    assert_eq!(
        off.effective.kinds,
        NotificationKinds {
            permission: false,
            question: false,
            failure: false,
            completion: false,
            stuck: false,
        }
    );
}

#[test]
fn stale_and_invalid_updates_leave_the_authoritative_policy_unchanged() {
    let workspace = TestWorkspace::new("atomic");
    let mut store = workspace.open();
    workspace.register(&mut store);
    let first = store
        .update_global_notification_policy(
            0,
            NotificationKinds::default(),
            QuietHours::default(),
            "2026-08-13T00:01:00.000Z",
        )
        .expect("first update");

    assert!(matches!(
        store.update_global_notification_policy(
            0,
            NotificationKinds {
                completion: true,
                ..NotificationKinds::default()
            },
            QuietHours::default(),
            "2026-08-13T00:02:00.000Z",
        ),
        Err(StoreError::NotificationPolicyVersionStale {
            scope: "global",
            expected: 0,
            current: 1,
        })
    ));
    assert!(matches!(
        store.update_global_notification_policy(
            1,
            NotificationKinds::default(),
            QuietHours {
                enabled: true,
                start_minute: 100,
                end_minute: 100,
            },
            "2026-08-13T00:03:00.000Z",
        ),
        Err(StoreError::InvalidNotificationPolicy {
            field: "quiet_hours.interval"
        })
    ));
    assert_eq!(store.notification_policy(None).expect("unchanged"), first);
}

#[test]
fn malformed_global_or_project_json_fails_closed_without_defaulting() {
    let workspace = TestWorkspace::new("malformed");
    let mut store = workspace.open();
    workspace.register(&mut store);
    drop(store);

    let connection = Connection::open(&workspace.database_path).expect("open raw database");
    connection
        .execute(
            "INSERT INTO app_settings(key, value_json, updated_at) VALUES('notification_policy', ?1, ?2)",
            params![r#"{"version":1,"unknown":true}"#, APPLIED_AT],
        )
        .expect("corrupt global policy");
    drop(connection);
    let store = workspace.open();
    assert!(matches!(
        store.notification_policy(None),
        Err(StoreError::StoredNotificationPolicyInvalid { scope: "global" })
    ));
    drop(store);

    let connection = Connection::open(&workspace.database_path).expect("open raw database");
    connection
        .execute(
            "DELETE FROM app_settings WHERE key = 'notification_policy'",
            [],
        )
        .expect("remove global policy");
    connection
        .execute(
            "UPDATE projects SET notification_policy_json = ?1 WHERE id = 'project-policy'",
            [r#"{"version":1,"master":"inherit"}"#],
        )
        .expect("corrupt Project policy");
    drop(connection);
    let store = workspace.open();
    assert!(matches!(
        store.notification_policy(Some("project-policy")),
        Err(StoreError::StoredNotificationPolicyInvalid { scope: "project" })
    ));
}
