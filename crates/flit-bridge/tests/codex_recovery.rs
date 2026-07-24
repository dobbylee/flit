use std::{
    cell::RefCell,
    collections::{BTreeMap, VecDeque},
    fs,
    path::{Path, PathBuf},
    process,
    rc::Rc,
    sync::atomic::{AtomicU64, Ordering},
};

use flit_bridge::codex_recovery::{
    CodexRecoveryAttempt, CodexRecoveryConnector, CodexRecoveryError, CodexRecoveryProvider,
    CodexRecoveryProviderError, reconcile_live_codex_sessions,
};
use flit_providers::{
    CodexManagedScope, CodexManagedThreadConflict, CodexManagedThreadId, CodexManagedThreads,
    CodexThreadRead, CodexThreadState, MAX_CODEX_MANAGED_THREADS, ProviderFingerprint,
};
use flit_store::{
    InitialManagedSessionConnection, ManagedRunIntent, ProjectRegistration,
    ProjectTrustConfirmation, Store,
};
use serde_json::{Map, Value, json};

const CREATED_AT: &str = "2026-07-24T11:00:00Z";
const STARTED_AT: &str = "2026-07-24T11:00:01Z";
const OBSERVED_AT: &str = "2026-07-24T11:00:05Z";
const EXECUTABLE_VERSION: &str = "0.144.6";
static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(1);

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new(label: &str) -> Self {
        let nonce = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "flit-bridge-recovery-{label}-{}-{nonce}",
            process::id()
        ));
        fs::create_dir(&path).expect("test directory");
        Self(path)
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[derive(Default)]
struct FakeLog {
    connected: Vec<PathBuf>,
    listed: Vec<(PathBuf, Vec<String>)>,
    read: Vec<String>,
}

struct FakeProvider {
    profile: Option<ProviderFingerprint>,
    lists: VecDeque<Result<CodexManagedThreads, CodexRecoveryProviderError>>,
    reads: BTreeMap<String, Result<CodexThreadRead, CodexRecoveryProviderError>>,
    log: Rc<RefCell<FakeLog>>,
}

impl CodexRecoveryProvider for FakeProvider {
    fn validated_profile(&self) -> Option<&ProviderFingerprint> {
        self.profile.as_ref()
    }

    fn list_managed(
        &mut self,
        scope: &CodexManagedScope,
    ) -> Result<CodexManagedThreads, CodexRecoveryProviderError> {
        self.log.borrow_mut().listed.push((
            scope.canonical_cwd().to_owned(),
            scope
                .exact_thread_ids()
                .iter()
                .map(|thread_id| thread_id.as_str().to_owned())
                .collect(),
        ));
        self.lists.pop_front().expect("scripted list result")
    }

    fn read_managed(
        &mut self,
        thread_id: &CodexManagedThreadId,
    ) -> Result<CodexThreadRead, CodexRecoveryProviderError> {
        self.log
            .borrow_mut()
            .read
            .push(thread_id.as_str().to_owned());
        self.reads
            .remove(thread_id.as_str())
            .expect("scripted read result")
    }
}

struct FakeConnector {
    providers: BTreeMap<PathBuf, VecDeque<Result<FakeProvider, CodexRecoveryProviderError>>>,
    log: Rc<RefCell<FakeLog>>,
}

impl CodexRecoveryConnector for FakeConnector {
    type Provider = FakeProvider;

    fn connect(&mut self, executable: &Path) -> Result<Self::Provider, CodexRecoveryProviderError> {
        self.log.borrow_mut().connected.push(executable.to_owned());
        self.providers
            .get_mut(executable)
            .and_then(VecDeque::pop_front)
            .expect("scripted connector result")
    }
}

#[test]
fn exact_partition_reconciles_terminal_and_unknown_states_and_retry_is_stable() {
    let directory = TestDirectory::new("partition");
    let executable = PathBuf::from("/private/tmp/flit-codex-recovery");
    let (mut store, project_path) = trusted_store(&directory);
    let cases = [
        ("completed", CodexThreadState::Completed),
        ("failed", CodexThreadState::Failed),
        ("interrupted", CodexThreadState::Interrupted),
        ("no-turns", CodexThreadState::NoTurns),
        ("unknown", CodexThreadState::Unknown),
        ("missing", CodexThreadState::Unknown),
        ("conflict", CodexThreadState::Unknown),
    ];
    for (label, _) in &cases {
        seed_live_session(
            &mut store,
            &project_path,
            &executable,
            label,
            Some(EXECUTABLE_VERSION),
        );
    }

    let matched = ["completed", "failed", "interrupted", "no-turns", "unknown"];
    let list = list_result(&matched, &["missing"], &["conflict"], &project_path);
    let reads = BTreeMap::from([
        (
            "thread-completed".to_owned(),
            Ok(thread_read(
                "completed",
                CodexThreadState::Completed,
                Some("turn-completed"),
            )),
        ),
        (
            "thread-failed".to_owned(),
            Ok(thread_read(
                "failed",
                CodexThreadState::Failed,
                Some("turn-failed"),
            )),
        ),
        (
            "thread-interrupted".to_owned(),
            Ok(thread_read(
                "interrupted",
                CodexThreadState::Interrupted,
                Some("turn-interrupted"),
            )),
        ),
        (
            "thread-no-turns".to_owned(),
            Ok(thread_read("no-turns", CodexThreadState::NoTurns, None)),
        ),
        (
            "thread-unknown".to_owned(),
            Ok(thread_read(
                "unknown",
                CodexThreadState::Unknown,
                Some("turn-unknown"),
            )),
        ),
    ]);
    let log = Rc::new(RefCell::new(FakeLog::default()));
    let provider = fake_provider(&executable, list, reads, Rc::clone(&log));
    let mut first_connector = connector(&executable, Ok(provider), Rc::clone(&log));
    let attempt = attempt("attempt-partition");

    let summary = reconcile_live_codex_sessions(&mut store, &mut first_connector, &attempt)
        .expect("reconciliation pass");
    assert_eq!(summary.examined, 7);
    assert_eq!(summary.completed, 1);
    assert_eq!(summary.failed, 1);
    assert_eq!(summary.interrupted, 1);
    assert_eq!(summary.no_turns, 1);
    assert_eq!(summary.unknown, 1);
    assert_eq!(summary.missing, 1);
    assert_eq!(summary.scope_conflicts, 1);
    assert_eq!(summary.duplicate_writes, 0);
    assert!(!summary.limit_reached);

    let log = log.borrow();
    assert_eq!(log.connected.as_slice(), std::slice::from_ref(&executable));
    assert_eq!(log.listed.len(), 1);
    assert_eq!(log.listed[0].0, project_path);
    assert_eq!(
        log.listed[0].1,
        [
            "thread-completed",
            "thread-conflict",
            "thread-failed",
            "thread-interrupted",
            "thread-missing",
            "thread-no-turns",
            "thread-unknown",
        ]
    );
    assert_eq!(
        log.read,
        [
            "thread-completed",
            "thread-failed",
            "thread-interrupted",
            "thread-no-turns",
            "thread-unknown",
        ]
    );
    drop(log);

    for (label, expected_end_reason) in [
        ("completed", Some("completed")),
        ("failed", Some("failed")),
        ("interrupted", Some("interrupted")),
        ("no-turns", None),
        ("unknown", None),
        ("missing", None),
        ("conflict", None),
    ] {
        assert_eq!(
            store
                .managed_session(&format!("session-{label}"))
                .expect("managed session")
                .expect("managed session")
                .end_reason
                .as_deref(),
            expected_end_reason
        );
    }
    assert_eq!(
        run_event_types(&store, "run-completed"),
        [
            "run.created",
            "run.start_requested",
            "session.connected",
            "diagnostic.sequence_gap",
            "run.completed",
        ]
    );
    assert_eq!(
        run_event_types(&store, "run-missing"),
        [
            "run.created",
            "run.start_requested",
            "session.connected",
            "diagnostic.sequence_gap",
        ]
    );

    let retry_list = list_result(
        &["no-turns", "unknown"],
        &["missing"],
        &["conflict"],
        &project_path,
    );
    let retry_reads = BTreeMap::from([
        (
            "thread-no-turns".to_owned(),
            Ok(thread_read("no-turns", CodexThreadState::NoTurns, None)),
        ),
        (
            "thread-unknown".to_owned(),
            Ok(thread_read(
                "unknown",
                CodexThreadState::Unknown,
                Some("turn-unknown"),
            )),
        ),
    ]);
    let retry_log = Rc::new(RefCell::new(FakeLog::default()));
    let retry_provider = fake_provider(&executable, retry_list, retry_reads, Rc::clone(&retry_log));
    let mut retry_connector = connector(&executable, Ok(retry_provider), Rc::clone(&retry_log));
    let cursor = store.latest_ingest_seq().expect("pre-retry cursor");
    let retry = reconcile_live_codex_sessions(&mut store, &mut retry_connector, &attempt)
        .expect("exact partial retry");
    assert_eq!(retry.examined, 4);
    assert_eq!(retry.duplicate_writes, 4);
    assert_eq!(store.latest_ingest_seq().expect("retry cursor"), cursor);
}

#[test]
fn provider_and_contract_ambiguity_persists_only_unknown_gaps() {
    let directory = TestDirectory::new("ambiguity");
    let (mut store, project_path) = trusted_store(&directory);
    let missing_path = PathBuf::from("/private/tmp/flit-codex-missing-path");
    let profile_mismatch = PathBuf::from("/private/tmp/flit-codex-profile-mismatch");
    let list_failure = PathBuf::from("/private/tmp/flit-codex-list-failure");
    let malformed_list = PathBuf::from("/private/tmp/flit-codex-malformed-list");
    let malformed_read = PathBuf::from("/private/tmp/flit-codex-malformed-read");
    seed_live_session(
        &mut store,
        &project_path,
        &missing_path,
        "missing-path",
        None,
    );
    for (path, label) in [
        (&profile_mismatch, "profile-mismatch"),
        (&list_failure, "list-failure"),
        (&malformed_list, "malformed-list"),
        (&malformed_read, "malformed-read"),
    ] {
        seed_live_session(
            &mut store,
            &project_path,
            path,
            label,
            Some(EXECUTABLE_VERSION),
        );
    }

    let log = Rc::new(RefCell::new(FakeLog::default()));
    let mut mismatch_provider = fake_provider(
        &profile_mismatch,
        list_result(&["profile-mismatch"], &[], &[], &project_path),
        BTreeMap::new(),
        Rc::clone(&log),
    );
    mismatch_provider
        .profile
        .as_mut()
        .expect("profile")
        .executable_version = "0.145.0".to_owned();
    let mut list_failure_provider = fake_provider(
        &list_failure,
        list_result(&["list-failure"], &[], &[], &project_path),
        BTreeMap::new(),
        Rc::clone(&log),
    );
    list_failure_provider.lists = VecDeque::from([Err(CodexRecoveryProviderError)]);
    let malformed_provider = fake_provider(
        &malformed_list,
        CodexManagedThreads {
            matched_thread_ids: vec![thread_id("unexpected")],
            conflicting_threads: Vec::new(),
            missing_thread_ids: Vec::new(),
            unrelated_thread_count: 0,
            page_count: 1,
        },
        BTreeMap::new(),
        Rc::clone(&log),
    );
    let malformed_read_provider = fake_provider(
        &malformed_read,
        list_result(&["malformed-read"], &[], &[], &project_path),
        BTreeMap::from([(
            "thread-malformed-read".to_owned(),
            Ok(thread_read(
                "malformed-read",
                CodexThreadState::Completed,
                None,
            )),
        )]),
        Rc::clone(&log),
    );
    let mut providers = BTreeMap::new();
    providers.insert(
        profile_mismatch.clone(),
        VecDeque::from([Ok(mismatch_provider)]),
    );
    providers.insert(
        list_failure.clone(),
        VecDeque::from([Ok(list_failure_provider)]),
    );
    providers.insert(
        malformed_list.clone(),
        VecDeque::from([Ok(malformed_provider)]),
    );
    providers.insert(
        malformed_read.clone(),
        VecDeque::from([Ok(malformed_read_provider)]),
    );
    let mut connector = FakeConnector {
        providers,
        log: Rc::clone(&log),
    };

    let summary =
        reconcile_live_codex_sessions(&mut store, &mut connector, &attempt("attempt-ambiguity"))
            .expect("ambiguity reconciliation");
    assert_eq!(summary.examined, 5);
    assert_eq!(summary.unknown, 5);
    assert_eq!(summary.completed, 0);
    for label in [
        "missing-path",
        "profile-mismatch",
        "list-failure",
        "malformed-list",
        "malformed-read",
    ] {
        assert_eq!(
            run_event_types(&store, &format!("run-{label}"))
                .last()
                .map(String::as_str),
            Some("diagnostic.sequence_gap")
        );
        assert_eq!(
            store
                .managed_session(&format!("session-{label}"))
                .expect("session")
                .expect("session")
                .ended_at,
            None
        );
    }
    let log = log.borrow();
    assert!(!log.connected.contains(&missing_path));
    assert!(
        !log.read
            .iter()
            .any(|thread| thread == "thread-malformed-list")
    );
    assert_eq!(log.read, ["thread-malformed-read"]);
}

#[test]
fn invalid_turn_ids_degrade_to_unknown_without_stopping_later_groups() {
    let directory = TestDirectory::new("invalid-turn-ids");
    let (mut store, project_path) = trusted_store(&directory);
    let completed_path = PathBuf::from("/private/tmp/flit-codex-a-invalid-completed-turn");
    let unknown_path = PathBuf::from("/private/tmp/flit-codex-b-invalid-unknown-turn");
    let later_path = PathBuf::from("/private/tmp/flit-codex-z-later-valid-turn");
    for (path, label) in [
        (&completed_path, "invalid-completed-turn"),
        (&unknown_path, "invalid-unknown-turn"),
        (&later_path, "later-valid-turn"),
    ] {
        seed_live_session(
            &mut store,
            &project_path,
            path,
            label,
            Some(EXECUTABLE_VERSION),
        );
    }

    let log = Rc::new(RefCell::new(FakeLog::default()));
    let completed_provider = fake_provider(
        &completed_path,
        list_result(&["invalid-completed-turn"], &[], &[], &project_path),
        BTreeMap::from([(
            "thread-invalid-completed-turn".to_owned(),
            Ok(thread_read(
                "invalid-completed-turn",
                CodexThreadState::Completed,
                Some("   "),
            )),
        )]),
        Rc::clone(&log),
    );
    let unknown_provider = fake_provider(
        &unknown_path,
        list_result(&["invalid-unknown-turn"], &[], &[], &project_path),
        BTreeMap::from([(
            "thread-invalid-unknown-turn".to_owned(),
            Ok(thread_read(
                "invalid-unknown-turn",
                CodexThreadState::Unknown,
                Some("\u{0001}"),
            )),
        )]),
        Rc::clone(&log),
    );
    let later_provider = fake_provider(
        &later_path,
        list_result(&["later-valid-turn"], &[], &[], &project_path),
        BTreeMap::from([(
            "thread-later-valid-turn".to_owned(),
            Ok(thread_read(
                "later-valid-turn",
                CodexThreadState::Completed,
                Some("turn-later-valid"),
            )),
        )]),
        Rc::clone(&log),
    );
    let mut connector = FakeConnector {
        providers: BTreeMap::from([
            (completed_path, VecDeque::from([Ok(completed_provider)])),
            (unknown_path, VecDeque::from([Ok(unknown_provider)])),
            (later_path, VecDeque::from([Ok(later_provider)])),
        ]),
        log: Rc::clone(&log),
    };

    let summary = reconcile_live_codex_sessions(
        &mut store,
        &mut connector,
        &attempt("attempt-invalid-turn-ids"),
    )
    .expect("invalid turn ID reconciliation");
    assert_eq!(summary.examined, 3);
    assert_eq!(summary.unknown, 2);
    assert_eq!(summary.completed, 1);
    for label in ["invalid-completed-turn", "invalid-unknown-turn"] {
        assert_eq!(
            store
                .managed_session(&format!("session-{label}"))
                .expect("session")
                .expect("session")
                .ended_at,
            None
        );
        assert_eq!(
            run_event_types(&store, &format!("run-{label}"))
                .last()
                .map(String::as_str),
            Some("diagnostic.sequence_gap")
        );
    }
    assert_eq!(
        store
            .managed_session("session-later-valid-turn")
            .expect("session")
            .expect("session")
            .end_reason
            .as_deref(),
        Some("completed")
    );
    assert_eq!(
        log.borrow().read,
        [
            "thread-invalid-completed-turn",
            "thread-invalid-unknown-turn",
            "thread-later-valid-turn",
        ]
    );
}

#[test]
fn noncanonical_executable_spellings_do_not_match_validated_profiles() {
    let directory = TestDirectory::new("executable-spelling");
    let (mut store, project_path) = trusted_store(&directory);
    let cases = [
        (
            "executable-internal-dot",
            PathBuf::from("/private/tmp/flit-codex-executable-dot/./codex"),
            PathBuf::from("/private/tmp/flit-codex-executable-dot/codex"),
        ),
        (
            "executable-repeated-separator",
            PathBuf::from("/private/tmp/flit-codex-executable-repeat//codex"),
            PathBuf::from("/private/tmp/flit-codex-executable-repeat/codex"),
        ),
        (
            "executable-trailing-separator",
            PathBuf::from("/private/tmp/flit-codex-executable-trailing/codex/"),
            PathBuf::from("/private/tmp/flit-codex-executable-trailing/codex"),
        ),
    ];
    for (label, stored_path, _) in &cases {
        seed_live_session(
            &mut store,
            &project_path,
            stored_path,
            label,
            Some(EXECUTABLE_VERSION),
        );
    }

    let log = Rc::new(RefCell::new(FakeLog::default()));
    let mut providers = BTreeMap::new();
    for (label, stored_path, canonical_path) in &cases {
        providers.insert(
            stored_path.clone(),
            VecDeque::from([Ok(fake_provider(
                canonical_path,
                list_result(&[label], &[], &[], &project_path),
                BTreeMap::new(),
                Rc::clone(&log),
            ))]),
        );
    }
    let mut connector = FakeConnector {
        providers,
        log: Rc::clone(&log),
    };

    let summary = reconcile_live_codex_sessions(
        &mut store,
        &mut connector,
        &attempt("attempt-executable-spelling"),
    )
    .expect("noncanonical executable reconciliation");
    assert_eq!(summary.examined, 3);
    assert_eq!(summary.unknown, 3);
    let log = log.borrow();
    assert_eq!(log.connected.len(), 3);
    assert!(log.listed.is_empty());
    assert!(log.read.is_empty());
}

#[test]
fn impossible_list_metadata_degrades_each_cwd_group_without_reads() {
    let directory = TestDirectory::new("impossible-list");
    let (mut store, project_path) = trusted_store(&directory);
    let count_path = PathBuf::from("/private/tmp/flit-codex-impossible-count");
    let relative_path = PathBuf::from("/private/tmp/flit-codex-relative-conflict");
    let empty_path = PathBuf::from("/private/tmp/flit-codex-empty-conflict");
    let overlong_path = PathBuf::from("/private/tmp/flit-codex-overlong-conflict");
    let parent_path = PathBuf::from("/private/tmp/flit-codex-parent-conflict");
    let dot_path = PathBuf::from("/private/tmp/flit-codex-dot-conflict");
    let repeated_path = PathBuf::from("/private/tmp/flit-codex-repeated-conflict");
    let trailing_path = PathBuf::from("/private/tmp/flit-codex-trailing-conflict");
    for (path, label) in [
        (&count_path, "impossible-count"),
        (&relative_path, "relative-conflict"),
        (&empty_path, "empty-conflict"),
        (&overlong_path, "overlong-conflict"),
        (&parent_path, "parent-conflict"),
        (&dot_path, "dot-conflict"),
        (&repeated_path, "repeated-conflict"),
        (&trailing_path, "trailing-conflict"),
    ] {
        seed_live_session(
            &mut store,
            &project_path,
            path,
            label,
            Some(EXECUTABLE_VERSION),
        );
    }

    let malformed_conflict = |label: &str, observed_cwd: PathBuf| CodexManagedThreads {
        matched_thread_ids: Vec::new(),
        conflicting_threads: vec![CodexManagedThreadConflict {
            thread_id: thread_id(label),
            observed_cwd,
        }],
        missing_thread_ids: Vec::new(),
        unrelated_thread_count: 0,
        page_count: 1,
    };
    let impossible_count = CodexManagedThreads {
        matched_thread_ids: vec![thread_id("impossible-count")],
        conflicting_threads: Vec::new(),
        missing_thread_ids: Vec::new(),
        unrelated_thread_count: MAX_CODEX_MANAGED_THREADS,
        page_count: 1,
    };
    let scripted = [
        (&count_path, impossible_count),
        (
            &relative_path,
            malformed_conflict("relative-conflict", PathBuf::from("relative")),
        ),
        (
            &empty_path,
            malformed_conflict("empty-conflict", PathBuf::new()),
        ),
        (
            &overlong_path,
            malformed_conflict(
                "overlong-conflict",
                PathBuf::from(format!("/{}", "x".repeat(16 * 1024 + 1))),
            ),
        ),
        (
            &parent_path,
            malformed_conflict(
                "parent-conflict",
                project_path.join("nested").join("..").join("elsewhere"),
            ),
        ),
        (
            &dot_path,
            malformed_conflict(
                "dot-conflict",
                PathBuf::from(format!("{}/scope/./other", project_path.display())),
            ),
        ),
        (
            &repeated_path,
            malformed_conflict(
                "repeated-conflict",
                PathBuf::from(format!("{}/scope//other", project_path.display())),
            ),
        ),
        (
            &trailing_path,
            malformed_conflict(
                "trailing-conflict",
                PathBuf::from(format!("{}/scope/other/", project_path.display())),
            ),
        ),
    ];
    let log = Rc::new(RefCell::new(FakeLog::default()));
    let mut providers = BTreeMap::new();
    for (path, list) in scripted {
        providers.insert(
            path.clone(),
            VecDeque::from([Ok(fake_provider(
                path,
                list,
                BTreeMap::new(),
                Rc::clone(&log),
            ))]),
        );
    }
    let mut connector = FakeConnector {
        providers,
        log: Rc::clone(&log),
    };

    let summary = reconcile_live_codex_sessions(
        &mut store,
        &mut connector,
        &attempt("attempt-impossible-list"),
    )
    .expect("malformed list reconciliation");
    assert_eq!(summary.examined, 8);
    assert_eq!(summary.unknown, 8);
    assert!(log.borrow().read.is_empty());
    for label in [
        "impossible-count",
        "relative-conflict",
        "empty-conflict",
        "overlong-conflict",
        "parent-conflict",
        "dot-conflict",
        "repeated-conflict",
        "trailing-conflict",
    ] {
        assert_eq!(
            run_event_types(&store, &format!("run-{label}"))
                .last()
                .map(String::as_str),
            Some("diagnostic.sequence_gap")
        );
    }
}

#[test]
fn long_stored_versions_use_unknown_contract_and_do_not_stop_later_groups() {
    let directory = TestDirectory::new("long-versions");
    let (mut store, project_path) = trusted_store(&directory);
    let version_240 = "v".repeat(240);
    let version_256 = "v".repeat(256);
    let path_240 = PathBuf::from("/private/tmp/flit-codex-a-version-240");
    let path_256 = PathBuf::from("/private/tmp/flit-codex-b-version-256");
    let later_path = PathBuf::from("/private/tmp/flit-codex-z-version-valid");
    seed_live_session(
        &mut store,
        &project_path,
        &path_240,
        "version-240",
        Some(&version_240),
    );
    seed_live_session(
        &mut store,
        &project_path,
        &path_256,
        "version-256",
        Some(&version_256),
    );
    seed_live_session(
        &mut store,
        &project_path,
        &later_path,
        "version-valid",
        Some(EXECUTABLE_VERSION),
    );

    let log = Rc::new(RefCell::new(FakeLog::default()));
    let later_provider = fake_provider(
        &later_path,
        list_result(&["version-valid"], &[], &[], &project_path),
        BTreeMap::from([(
            "thread-version-valid".to_owned(),
            Ok(thread_read(
                "version-valid",
                CodexThreadState::Completed,
                Some("turn-version-valid"),
            )),
        )]),
        Rc::clone(&log),
    );
    let mut connector = FakeConnector {
        providers: BTreeMap::from([
            (path_240, VecDeque::from([Err(CodexRecoveryProviderError)])),
            (path_256, VecDeque::from([Err(CodexRecoveryProviderError)])),
            (later_path, VecDeque::from([Ok(later_provider)])),
        ]),
        log,
    };

    let summary = reconcile_live_codex_sessions(
        &mut store,
        &mut connector,
        &attempt("attempt-long-versions"),
    )
    .expect("long version reconciliation");
    assert_eq!(summary.examined, 3);
    assert_eq!(summary.unknown, 2);
    assert_eq!(summary.completed, 1);
    for label in ["version-240", "version-256"] {
        assert_eq!(
            run_event_types(&store, &format!("run-{label}"))
                .last()
                .map(String::as_str),
            Some("diagnostic.sequence_gap")
        );
    }
    assert_eq!(
        store
            .managed_session("session-version-valid")
            .expect("session")
            .expect("session")
            .end_reason
            .as_deref(),
        Some("completed")
    );
}

#[test]
fn store_conflict_stops_retry_instead_of_becoming_provider_unknown() {
    let directory = TestDirectory::new("store-conflict");
    let executable = PathBuf::from("/private/tmp/flit-codex-store-conflict");
    let (mut store, project_path) = trusted_store(&directory);
    seed_live_session(
        &mut store,
        &project_path,
        &executable,
        "store-conflict",
        Some(EXECUTABLE_VERSION),
    );
    let first_log = Rc::new(RefCell::new(FakeLog::default()));
    let mut first_connector = connector(
        &executable,
        Err(CodexRecoveryProviderError),
        Rc::clone(&first_log),
    );
    let attempt = attempt("attempt-store-conflict");
    let first = reconcile_live_codex_sessions(&mut store, &mut first_connector, &attempt)
        .expect("initial unknown gap");
    assert_eq!(first.unknown, 1);
    let cursor = store.latest_ingest_seq().expect("initial gap cursor");

    let second_log = Rc::new(RefCell::new(FakeLog::default()));
    let provider = fake_provider(
        &executable,
        list_result(&["store-conflict"], &[], &[], &project_path),
        BTreeMap::from([(
            "thread-store-conflict".to_owned(),
            Ok(thread_read(
                "store-conflict",
                CodexThreadState::Completed,
                Some("turn-store-conflict"),
            )),
        )]),
        Rc::clone(&second_log),
    );
    let mut second_connector = connector(&executable, Ok(provider), Rc::clone(&second_log));
    assert!(matches!(
        reconcile_live_codex_sessions(&mut store, &mut second_connector, &attempt),
        Err(CodexRecoveryError::Store(_))
    ));
    assert_eq!(store.latest_ingest_seq().expect("conflict cursor"), cursor);
    assert_eq!(
        store
            .managed_session("session-store-conflict")
            .expect("session")
            .expect("session")
            .ended_at,
        None
    );
}

fn trusted_store(directory: &TestDirectory) -> (Store, PathBuf) {
    let database = directory.0.join("flit.sqlite3");
    let project_path = directory.0.join("project");
    fs::create_dir(&project_path).expect("Project directory");
    let mut store = Store::open(&database, CREATED_AT).expect("open Store");
    store
        .register_project(ProjectRegistration {
            id: "project-1".to_owned(),
            display_name: "Recovery Project".to_owned(),
            selected_path: project_path.clone(),
            created_at: CREATED_AT.to_owned(),
        })
        .expect("register Project");
    store
        .confirm_project_trust(ProjectTrustConfirmation {
            project_id: "project-1".to_owned(),
            selected_path: project_path,
            confirmed_at: CREATED_AT.to_owned(),
        })
        .expect("trust Project");
    let canonical_path = store
        .project("project-1")
        .expect("Project")
        .expect("Project")
        .canonical_path;
    (store, canonical_path)
}

fn seed_live_session(
    store: &mut Store,
    project_path: &Path,
    executable: &Path,
    label: &str,
    executable_version: Option<&str>,
) {
    let run_id = format!("run-{label}");
    store
        .create_managed_run_intent(ManagedRunIntent {
            id: run_id.clone(),
            project_id: "project-1".to_owned(),
            title: format!("Run {label}"),
            goal: Some("Reconcile this managed Run.".to_owned()),
            start_request: object(json!({"prompt_sha256": format!("digest-{label}")})),
            baseline_head: None,
            created_at: CREATED_AT.to_owned(),
            run_created_event_id: format!("event-{label}-created"),
            start_requested_event_id: format!("event-{label}-requested"),
        })
        .expect("managed Run");
    store
        .connect_initial_managed_session(InitialManagedSessionConnection {
            id: format!("session-{label}"),
            run_id,
            external_session_key: format!("thread-{label}"),
            session_fingerprint: "validated-profile".to_owned(),
            executable_path: executable_version.map(|_| executable.to_owned()),
            executable_version: executable_version.map(str::to_owned),
            cwd: project_path.to_owned(),
            capabilities: object(json!({"completion_detect": "supported"})),
            contract_version: format!("codex-app-server/{EXECUTABLE_VERSION}"),
            started_at: STARTED_AT.to_owned(),
            connected_event_id: format!("event-{label}-connected"),
        })
        .expect("managed session");
}

fn fake_provider(
    executable: &Path,
    list: CodexManagedThreads,
    reads: BTreeMap<String, Result<CodexThreadRead, CodexRecoveryProviderError>>,
    log: Rc<RefCell<FakeLog>>,
) -> FakeProvider {
    FakeProvider {
        profile: Some(profile(executable, EXECUTABLE_VERSION)),
        lists: VecDeque::from([Ok(list)]),
        reads,
        log,
    }
}

fn connector(
    executable: &Path,
    provider: Result<FakeProvider, CodexRecoveryProviderError>,
    log: Rc<RefCell<FakeLog>>,
) -> FakeConnector {
    FakeConnector {
        providers: BTreeMap::from([(executable.to_owned(), VecDeque::from([provider]))]),
        log,
    }
}

fn profile(executable: &Path, version: &str) -> ProviderFingerprint {
    ProviderFingerprint {
        canonical_executable: executable.to_owned(),
        executable_version: version.to_owned(),
        executable_sha256: "executable-sha".to_owned(),
        combined_schema_sha256: "combined-schema-sha".to_owned(),
        v2_schema_sha256: "v2-schema-sha".to_owned(),
        method_allowlist_sha256: "allowlist-sha".to_owned(),
        fixture_sha256: "fixture-sha".to_owned(),
        smoke_run_id: "smoke-run".to_owned(),
    }
}

fn list_result(
    matched: &[&str],
    missing: &[&str],
    conflicts: &[&str],
    observed_cwd: &Path,
) -> CodexManagedThreads {
    CodexManagedThreads {
        matched_thread_ids: matched.iter().map(|label| thread_id(label)).collect(),
        conflicting_threads: conflicts
            .iter()
            .map(|label| CodexManagedThreadConflict {
                thread_id: thread_id(label),
                observed_cwd: observed_cwd.join("conflicting"),
            })
            .collect(),
        missing_thread_ids: missing.iter().map(|label| thread_id(label)).collect(),
        unrelated_thread_count: 0,
        page_count: 1,
    }
}

fn thread_read(label: &str, state: CodexThreadState, turn: Option<&str>) -> CodexThreadRead {
    CodexThreadRead {
        thread_id: thread_id(label),
        latest_turn_id: turn.map(str::to_owned),
        state,
    }
}

fn thread_id(label: &str) -> CodexManagedThreadId {
    CodexManagedThreadId::new(format!("thread-{label}")).expect("thread ID")
}

fn attempt(id: &str) -> CodexRecoveryAttempt {
    CodexRecoveryAttempt {
        id: id.to_owned(),
        observed_at: OBSERVED_AT.to_owned(),
    }
}

fn run_event_types(store: &Store, run_id: &str) -> Vec<String> {
    let upper_bound = store.latest_ingest_seq().expect("event cursor");
    let events = store
        .run_events_through(run_id, 0, upper_bound, 100)
        .expect("Run events")
        .events;
    events.into_iter().map(|event| event.event_type).collect()
}

fn object(value: Value) -> Map<String, Value> {
    value.as_object().expect("object fixture").clone()
}
