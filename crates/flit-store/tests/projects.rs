use std::{
    fs,
    path::{Path, PathBuf},
    process,
    sync::{
        Arc, Barrier,
        atomic::{AtomicU64, Ordering},
    },
    thread,
};

use flit_store::{
    ProjectDirectoryInspection, ProjectInspectionError, ProjectRegistration,
    ProjectRegistrationOutcome, Store, StoreError, initial_migration_checksum,
};
use rusqlite::{Connection, ErrorCode, params};

const APPLIED_AT: &str = "2026-07-24T00:00:00.000Z";
static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(1);

struct TestWorkspace {
    directory: PathBuf,
    database_path: PathBuf,
    project_path: PathBuf,
}

impl TestWorkspace {
    fn new(label: &str) -> Self {
        let nonce = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let directory =
            std::env::temp_dir().join(format!("flit-projects-{label}-{}-{nonce}", process::id()));
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
fn inspection_returns_a_canonical_directory_identity_and_detects_a_selected_symlink() {
    let workspace = TestWorkspace::new("inspection");
    let direct = ProjectDirectoryInspection::inspect(&workspace.project_path).expect("direct");
    assert!(direct.identity.canonical_path.is_absolute());
    assert!(direct.identity.filesystem_id.starts_with("unix:"));
    let canonical =
        ProjectDirectoryInspection::inspect(&direct.identity.canonical_path).expect("canonical");
    assert!(!canonical.selected_via_symlink);
    assert_eq!(canonical.identity, direct.identity);

    let selected_link = workspace.directory.join("selected-link");
    std::os::unix::fs::symlink(&workspace.project_path, &selected_link).expect("Project symlink");
    let via_link = ProjectDirectoryInspection::inspect(&selected_link).expect("symlink");
    assert!(via_link.selected_via_symlink);
    assert_eq!(via_link.identity, direct.identity);

    let symlink_target = workspace.project_path.join("symlink-target");
    fs::create_dir(&symlink_target).expect("symlink target directory");
    let nested_link = workspace.project_path.join("nested-link");
    std::os::unix::fs::symlink(&symlink_target, &nested_link).expect("nested symlink");
    let via_symlink_parent =
        ProjectDirectoryInspection::inspect(nested_link.join("..")).expect("symlink parent");
    assert!(via_symlink_parent.selected_via_symlink);
    assert_eq!(via_symlink_parent.identity, direct.identity);
}

#[test]
fn inspection_rejects_missing_paths_and_regular_files() {
    let workspace = TestWorkspace::new("invalid");
    let missing = workspace.directory.join("missing");
    assert!(matches!(
        ProjectDirectoryInspection::inspect(&missing),
        Err(ProjectInspectionError::Canonicalize { .. })
    ));

    let file = workspace.directory.join("not-a-directory");
    fs::write(&file, "not a Project").expect("regular file");
    assert!(matches!(
        ProjectDirectoryInspection::inspect(&file),
        Err(ProjectInspectionError::NotDirectory { .. })
    ));

    let mut store = workspace.open();
    assert!(matches!(
        store.register_project(registration("missing", "Missing", &missing)),
        Err(StoreError::ProjectInspection(
            ProjectInspectionError::Canonicalize { .. }
        ))
    ));
    assert!(matches!(
        store.register_project(registration("file", "File", &file)),
        Err(StoreError::ProjectInspection(
            ProjectInspectionError::NotDirectory { .. }
        ))
    ));
    assert_eq!(project_count(&workspace.database_path), 0);
}

#[test]
fn registration_persists_an_untrusted_project_and_reopens_it() {
    let workspace = TestWorkspace::new("register");
    let registration = registration("project-one", "Project One", &workspace.project_path);
    let mut store = workspace.open();
    let registered = match store.register_project(registration).expect("register") {
        ProjectRegistrationOutcome::Registered(project) => project,
        outcome => panic!("unexpected registration outcome: {outcome:?}"),
    };
    assert!(!registered.trusted);
    assert_eq!(registered.default_provider, None);
    assert_eq!(registered.notification_policy_json, "{}");
    assert!(registered.filesystem_id.is_some());
    drop(store);

    let reopened = workspace.open();
    assert_eq!(
        reopened.project("project-one").expect("read Project"),
        Some(registered)
    );
}

#[test]
fn registration_rejects_canonical_and_filesystem_identity_duplicates_without_writing_rows() {
    let workspace = TestWorkspace::new("duplicates");
    let mut store = workspace.open();
    assert!(matches!(
        store
            .register_project(registration(
                "project-one",
                "Project One",
                &workspace.project_path
            ))
            .expect("first registration"),
        ProjectRegistrationOutcome::Registered(_)
    ));

    let selected_link = workspace.directory.join("selected-link");
    std::os::unix::fs::symlink(&workspace.project_path, &selected_link).expect("Project symlink");
    assert!(matches!(
        store
            .register_project(registration("project-two", "Project Two", &selected_link))
            .expect("canonical duplicate"),
        ProjectRegistrationOutcome::DuplicateCanonicalPath { existing_project_id }
            if existing_project_id == "project-one"
    ));

    let alternate_path = workspace.directory.join("legacy-path");
    store.project("project-one").expect("read first Project");
    drop(store);
    let connection = Connection::open(&workspace.database_path).expect("legacy connection");
    connection
        .execute(
            "UPDATE projects SET canonical_path = ?1 WHERE id = 'project-one'",
            [alternate_path.to_string_lossy().as_ref()],
        )
        .expect("simulate renamed legacy path");
    drop(connection);

    let mut reopened = workspace.open();
    assert!(matches!(
        reopened
            .register_project(registration("project-three", "Project Three", &workspace.project_path))
            .expect("filesystem duplicate"),
        ProjectRegistrationOutcome::DuplicateFilesystemIdentity { existing_project_id }
            if existing_project_id == "project-one"
    ));
    assert_eq!(project_count(&workspace.database_path), 1);
}

#[test]
fn concurrent_registrations_return_a_typed_duplicate_after_the_immediate_transaction_recheck() {
    let workspace = TestWorkspace::new("concurrent");
    drop(workspace.open());
    let start = Arc::new(Barrier::new(2));
    let first = register_concurrently(
        workspace.database_path.clone(),
        workspace.project_path.clone(),
        "project-one",
        "Project One",
        Arc::clone(&start),
    );
    let second = register_concurrently(
        workspace.database_path.clone(),
        workspace.project_path.clone(),
        "project-two",
        "Project Two",
        start,
    );
    let first = first
        .join()
        .expect("first registration thread")
        .expect("first result");
    let second = second
        .join()
        .expect("second registration thread")
        .expect("second result");

    let outcomes = [first, second];
    let registered = outcomes
        .iter()
        .find_map(|outcome| match outcome {
            ProjectRegistrationOutcome::Registered(project) => Some(project),
            ProjectRegistrationOutcome::DuplicateCanonicalPath { .. }
            | ProjectRegistrationOutcome::DuplicateFilesystemIdentity { .. } => None,
        })
        .expect("one Project is registered");
    let duplicate = outcomes
        .iter()
        .find_map(|outcome| match outcome {
            ProjectRegistrationOutcome::DuplicateCanonicalPath {
                existing_project_id,
            }
            | ProjectRegistrationOutcome::DuplicateFilesystemIdentity {
                existing_project_id,
            } => Some(existing_project_id),
            ProjectRegistrationOutcome::Registered(_) => None,
        })
        .expect("one typed duplicate");
    assert_eq!(duplicate, &registered.id);
    assert_eq!(project_count(&workspace.database_path), 1);
}

#[test]
fn partial_unique_index_rejects_concurrent_legacy_filesystem_identity_rows() {
    let workspace = TestWorkspace::new("unique-index");
    let inspection = ProjectDirectoryInspection::inspect(&workspace.project_path).expect("inspect");
    let store = workspace.open();
    drop(store);
    let connection = Connection::open(&workspace.database_path).expect("raw connection");
    insert_project(
        &connection,
        "first",
        "/private/tmp/flit-project-first",
        &inspection.identity.filesystem_id,
    );
    let error = connection
        .execute(
            "INSERT INTO projects(id, display_name, canonical_path, filesystem_id, trusted, notification_policy_json, created_at, updated_at) VALUES('second', 'Second', '/private/tmp/flit-project-second', ?1, 0, '{}', ?2, ?2)",
            params![inspection.identity.filesystem_id, APPLIED_AT],
        )
        .expect_err("duplicate filesystem identity must be rejected");
    assert_eq!(
        error.sqlite_error_code(),
        Some(ErrorCode::ConstraintViolation)
    );
}

#[test]
fn invalid_registration_fields_are_rejected_before_a_row_is_written() {
    let workspace = TestWorkspace::new("invalid-registration");
    let mut store = workspace.open();
    assert!(matches!(
        store.register_project(registration(" ", "Project", &workspace.project_path)),
        Err(StoreError::InvalidProjectRegistration { field: "id" })
    ));
    assert_eq!(project_count(&workspace.database_path), 0);
}

#[test]
fn version_one_database_migrates_while_preserving_projects_and_conflicting_legacy_ids_fail_closed()
{
    let workspace = TestWorkspace::new("migration");
    let connection = Connection::open(&workspace.database_path).expect("version one database");
    connection
        .execute_batch(include_str!("../migrations/0001_initial.sql"))
        .expect("version one schema");
    connection
        .execute(
            "INSERT INTO schema_migrations(version, name, checksum, applied_at) VALUES(1, 'initial', ?1, ?2)",
            params![initial_migration_checksum(), APPLIED_AT],
        )
        .expect("version one registry");
    insert_project(
        &connection,
        "legacy",
        "/private/tmp/flit-legacy",
        "unix:1:1",
    );
    drop(connection);

    let store = workspace.open();
    assert_eq!(store.schema_version().expect("schema version"), 2);
    assert!(
        !store
            .project("legacy")
            .expect("legacy Project")
            .unwrap()
            .trusted
    );
    drop(store);

    let conflict = TestWorkspace::new("migration-conflict");
    let connection =
        Connection::open(&conflict.database_path).expect("version one conflict database");
    connection
        .execute_batch(include_str!("../migrations/0001_initial.sql"))
        .expect("version one schema");
    connection
        .execute(
            "INSERT INTO schema_migrations(version, name, checksum, applied_at) VALUES(1, 'initial', ?1, ?2)",
            params![initial_migration_checksum(), APPLIED_AT],
        )
        .expect("version one registry");
    insert_project(
        &connection,
        "legacy-a",
        "/private/tmp/flit-legacy-a",
        "unix:2:2",
    );
    insert_project(
        &connection,
        "legacy-b",
        "/private/tmp/flit-legacy-b",
        "unix:2:2",
    );
    drop(connection);

    assert!(matches!(
        Store::open(&conflict.database_path, APPLIED_AT),
        Err(StoreError::Sqlite(_))
    ));
    let connection = Connection::open(&conflict.database_path).expect("inspect rejected migration");
    let versions = connection
        .prepare("SELECT version FROM schema_migrations ORDER BY version")
        .expect("version query")
        .query_map([], |row| row.get::<_, i64>(0))
        .expect("version rows")
        .collect::<Result<Vec<_>, _>>()
        .expect("versions");
    assert_eq!(versions, [1]);
}

fn registration(
    id: &str,
    display_name: &str,
    selected_path: impl AsRef<Path>,
) -> ProjectRegistration {
    ProjectRegistration {
        id: id.to_owned(),
        display_name: display_name.to_owned(),
        selected_path: selected_path.as_ref().to_owned(),
        created_at: APPLIED_AT.to_owned(),
    }
}

fn register_concurrently(
    database_path: PathBuf,
    selected_path: PathBuf,
    id: &'static str,
    display_name: &'static str,
    start: Arc<Barrier>,
) -> thread::JoinHandle<Result<ProjectRegistrationOutcome, String>> {
    thread::spawn(move || {
        let mut store =
            Store::open(database_path, APPLIED_AT).map_err(|error| error.to_string())?;
        start.wait();
        store
            .register_project(registration(id, display_name, selected_path))
            .map_err(|error| error.to_string())
    })
}

fn insert_project(connection: &Connection, id: &str, canonical_path: &str, filesystem_id: &str) {
    connection
        .execute(
            "INSERT INTO projects(id, display_name, canonical_path, filesystem_id, trusted, notification_policy_json, created_at, updated_at) VALUES(?1, ?1, ?2, ?3, 0, '{}', ?4, ?4)",
            params![id, canonical_path, filesystem_id, APPLIED_AT],
        )
        .expect("insert Project");
}

fn project_count(path: &Path) -> i64 {
    Connection::open(path)
        .expect("count connection")
        .query_row("SELECT COUNT(*) FROM projects", [], |row| row.get(0))
        .expect("Project count")
}
