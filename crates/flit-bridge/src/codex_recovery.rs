use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsString,
    path::{Path, PathBuf},
};

use flit_providers::{
    CodexAppServer, CodexManagedScope, CodexManagedThreadId, CodexManagedThreads,
    CodexManagedTurnId, CodexThreadRead, CodexThreadState, MAX_CODEX_LIST_PAGES,
    MAX_CODEX_MANAGED_THREADS, ProviderFingerprint,
};
use flit_store::{
    MAX_LIVE_MANAGED_SESSIONS, ManagedReconciliationState, ManagedSession,
    ManagedSessionReconciliation, ManagedSessionReconciliationOutcome, Store, StoreError,
};
use sha2::{Digest, Sha256};

const MAX_RECOVERY_ATTEMPT_ID_BYTES: usize = 96;
const MAX_RECOVERY_TIMESTAMP_BYTES: usize = 128;
const MAX_MANAGED_CONTRACT_VERSION_BYTES: usize = 256;
const UNKNOWN_CODEX_CONTRACT_VERSION: &str = "codex-app-server/unknown";
const CODEX_CONTRACT_VERSION_PREFIX: &str = "codex-app-server/";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CodexRecoveryAttempt {
    pub id: String,
    pub observed_at: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CodexRecoverySummary {
    pub examined: usize,
    pub no_turns: usize,
    pub completed: usize,
    pub failed: usize,
    pub interrupted: usize,
    pub unknown: usize,
    pub missing: usize,
    pub scope_conflicts: usize,
    pub duplicate_writes: usize,
    pub limit_reached: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum CodexRecoveryError {
    #[error("invalid Codex recovery attempt field: {0}")]
    InvalidAttempt(&'static str),
    #[error("Codex recovery persistence failed")]
    Store(#[from] StoreError),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CodexRecoveryProviderError;

pub trait CodexRecoveryProvider {
    fn validated_profile(&self) -> Option<&ProviderFingerprint>;

    fn list_managed(
        &mut self,
        scope: &CodexManagedScope,
    ) -> Result<CodexManagedThreads, CodexRecoveryProviderError>;

    fn read_managed(
        &mut self,
        thread_id: &CodexManagedThreadId,
    ) -> Result<CodexThreadRead, CodexRecoveryProviderError>;
}

pub trait CodexRecoveryConnector {
    type Provider: CodexRecoveryProvider;

    fn connect(&mut self, executable: &Path) -> Result<Self::Provider, CodexRecoveryProviderError>;
}

#[derive(Default)]
pub struct ExactCodexRecoveryConnector;

impl CodexRecoveryProvider for CodexAppServer {
    fn validated_profile(&self) -> Option<&ProviderFingerprint> {
        CodexAppServer::validated_profile(self)
    }

    fn list_managed(
        &mut self,
        scope: &CodexManagedScope,
    ) -> Result<CodexManagedThreads, CodexRecoveryProviderError> {
        CodexAppServer::list_managed(self, scope).map_err(|_| CodexRecoveryProviderError)
    }

    fn read_managed(
        &mut self,
        thread_id: &CodexManagedThreadId,
    ) -> Result<CodexThreadRead, CodexRecoveryProviderError> {
        CodexAppServer::read_managed(self, thread_id).map_err(|_| CodexRecoveryProviderError)
    }
}

impl CodexRecoveryConnector for ExactCodexRecoveryConnector {
    type Provider = CodexAppServer;

    fn connect(&mut self, executable: &Path) -> Result<Self::Provider, CodexRecoveryProviderError> {
        CodexAppServer::connect_at(executable).map_err(|_| CodexRecoveryProviderError)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
struct ExecutableGroup {
    path_spelling: OsString,
    version: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ListDisposition {
    Matched,
    Missing,
    ScopeConflict,
}

pub fn reconcile_live_codex_sessions<C: CodexRecoveryConnector>(
    store: &mut Store,
    connector: &mut C,
    attempt: &CodexRecoveryAttempt,
) -> Result<CodexRecoverySummary, CodexRecoveryError> {
    validate_attempt(attempt)?;
    let sessions = store.live_managed_sessions(MAX_LIVE_MANAGED_SESSIONS)?;
    let mut summary = CodexRecoverySummary {
        examined: sessions.len(),
        limit_reached: sessions.len() == MAX_LIVE_MANAGED_SESSIONS,
        ..CodexRecoverySummary::default()
    };
    let mut executable_groups = BTreeMap::<ExecutableGroup, Vec<ManagedSession>>::new();
    for session in sessions {
        let Some(path) = session.executable_path.clone() else {
            persist_reconciliation(
                store,
                attempt,
                &session,
                ManagedReconciliationState::Unknown,
                None,
                &mut summary,
            )?;
            continue;
        };
        let Some(version) = session.executable_version.clone() else {
            persist_reconciliation(
                store,
                attempt,
                &session,
                ManagedReconciliationState::Unknown,
                None,
                &mut summary,
            )?;
            continue;
        };
        executable_groups
            .entry(ExecutableGroup {
                path_spelling: path.into_os_string(),
                version,
            })
            .or_default()
            .push(session);
    }

    for (executable, sessions) in executable_groups {
        let executable_path = Path::new(&executable.path_spelling);
        let Ok(mut provider) = connector.connect(executable_path) else {
            persist_unknown_group(store, attempt, sessions, &mut summary)?;
            continue;
        };
        let profile_matches = provider.validated_profile().is_some_and(|profile| {
            same_path_spelling(&profile.canonical_executable, executable_path)
                && profile.executable_version == executable.version
        });
        if !profile_matches {
            persist_unknown_group(store, attempt, sessions, &mut summary)?;
            continue;
        }

        let mut cwd_groups = BTreeMap::<PathBuf, Vec<ManagedSession>>::new();
        for session in sessions {
            cwd_groups
                .entry(session.cwd.clone())
                .or_default()
                .push(session);
        }
        for (cwd, sessions) in cwd_groups {
            reconcile_cwd_group(store, &mut provider, attempt, &cwd, sessions, &mut summary)?;
        }
    }
    Ok(summary)
}

fn reconcile_cwd_group<P: CodexRecoveryProvider>(
    store: &mut Store,
    provider: &mut P,
    attempt: &CodexRecoveryAttempt,
    cwd: &Path,
    sessions: Vec<ManagedSession>,
    summary: &mut CodexRecoverySummary,
) -> Result<(), CodexRecoveryError> {
    let mut exact_sessions = BTreeMap::<CodexManagedThreadId, ManagedSession>::new();
    let mut invalid_sessions = Vec::new();
    for session in sessions {
        match CodexManagedThreadId::new(session.external_session_key.clone()) {
            Ok(thread_id) if !exact_sessions.contains_key(&thread_id) => {
                exact_sessions.insert(thread_id, session);
            }
            Ok(_) | Err(_) => invalid_sessions.push(session),
        }
    }
    persist_unknown_group(store, attempt, invalid_sessions, summary)?;
    if exact_sessions.is_empty() {
        return Ok(());
    }
    let scope = match CodexManagedScope::new(cwd.to_owned(), exact_sessions.keys().cloned()) {
        Ok(scope) => scope,
        Err(_) => {
            persist_unknown_group(
                store,
                attempt,
                exact_sessions.into_values().collect(),
                summary,
            )?;
            return Ok(());
        }
    };
    let listed = match provider.list_managed(&scope) {
        Ok(listed) => listed,
        Err(_) => {
            persist_unknown_group(
                store,
                attempt,
                exact_sessions.into_values().collect(),
                summary,
            )?;
            return Ok(());
        }
    };
    let Some(partition) = exact_partition(exact_sessions.keys(), cwd, &listed) else {
        persist_unknown_group(
            store,
            attempt,
            exact_sessions.into_values().collect(),
            summary,
        )?;
        return Ok(());
    };

    for (thread_id, session) in exact_sessions {
        match partition
            .get(&thread_id)
            .copied()
            .expect("validated exact partition")
        {
            ListDisposition::Missing => {
                persist_reconciliation(
                    store,
                    attempt,
                    &session,
                    ManagedReconciliationState::Missing,
                    None,
                    summary,
                )?;
            }
            ListDisposition::ScopeConflict => {
                persist_reconciliation(
                    store,
                    attempt,
                    &session,
                    ManagedReconciliationState::ScopeConflict,
                    None,
                    summary,
                )?;
            }
            ListDisposition::Matched => {
                let (state, latest_turn_id) = match provider.read_managed(&thread_id) {
                    Ok(read) if read.thread_id == thread_id => map_thread_read(read),
                    Ok(_) | Err(_) => (ManagedReconciliationState::Unknown, None),
                };
                persist_reconciliation(
                    store,
                    attempt,
                    &session,
                    state,
                    latest_turn_id.as_deref(),
                    summary,
                )?;
            }
        }
    }
    Ok(())
}

fn exact_partition<'a>(
    expected: impl Iterator<Item = &'a CodexManagedThreadId>,
    expected_cwd: &Path,
    listed: &CodexManagedThreads,
) -> Option<BTreeMap<CodexManagedThreadId, ListDisposition>> {
    if listed.page_count == 0 || listed.page_count > MAX_CODEX_LIST_PAGES {
        return None;
    }
    let observed_limit = listed.page_count.checked_mul(MAX_CODEX_MANAGED_THREADS)?;
    let observed_count = listed
        .matched_thread_ids
        .len()
        .checked_add(listed.conflicting_threads.len())?
        .checked_add(listed.unrelated_thread_count)?;
    if observed_count > observed_limit {
        return None;
    }
    let expected = expected.cloned().collect::<BTreeSet<_>>();
    let mut partition = BTreeMap::new();
    for thread_id in &listed.matched_thread_ids {
        if !expected.contains(thread_id)
            || partition
                .insert(thread_id.clone(), ListDisposition::Matched)
                .is_some()
        {
            return None;
        }
    }
    for conflict in &listed.conflicting_threads {
        if conflict.observed_cwd == expected_cwd
            || !is_lexically_canonical_spelling(&conflict.observed_cwd)
            || CodexManagedScope::new(
                conflict.observed_cwd.clone(),
                std::iter::once(conflict.thread_id.clone()),
            )
            .is_err()
            || !expected.contains(&conflict.thread_id)
            || partition
                .insert(conflict.thread_id.clone(), ListDisposition::ScopeConflict)
                .is_some()
        {
            return None;
        }
    }
    for thread_id in &listed.missing_thread_ids {
        if !expected.contains(thread_id)
            || partition
                .insert(thread_id.clone(), ListDisposition::Missing)
                .is_some()
        {
            return None;
        }
    }
    (partition.len() == expected.len()).then_some(partition)
}

fn same_path_spelling(left: &Path, right: &Path) -> bool {
    left.as_os_str() == right.as_os_str()
}

fn is_lexically_canonical_spelling(path: &Path) -> bool {
    let normalized = path.components().collect::<PathBuf>();
    same_path_spelling(path, &normalized)
}

fn map_thread_read(read: CodexThreadRead) -> (ManagedReconciliationState, Option<String>) {
    let latest_turn_id = match read.latest_turn_id {
        Some(turn_id) => match CodexManagedTurnId::new(turn_id) {
            Ok(turn_id) => Some(turn_id.as_str().to_owned()),
            Err(_) => return (ManagedReconciliationState::Unknown, None),
        },
        None => None,
    };
    match (read.state, latest_turn_id) {
        (CodexThreadState::NoTurns, None) => (ManagedReconciliationState::NoTurns, None),
        (CodexThreadState::Completed, Some(turn_id)) => {
            (ManagedReconciliationState::Completed, Some(turn_id))
        }
        (CodexThreadState::Failed, Some(turn_id)) => {
            (ManagedReconciliationState::Failed, Some(turn_id))
        }
        (CodexThreadState::Interrupted, Some(turn_id)) => {
            (ManagedReconciliationState::Interrupted, Some(turn_id))
        }
        (CodexThreadState::Unknown, latest_turn_id) => {
            (ManagedReconciliationState::Unknown, latest_turn_id)
        }
        (CodexThreadState::NoTurns, Some(_))
        | (CodexThreadState::Completed, None)
        | (CodexThreadState::Failed, None)
        | (CodexThreadState::Interrupted, None) => (ManagedReconciliationState::Unknown, None),
    }
}

fn persist_unknown_group(
    store: &mut Store,
    attempt: &CodexRecoveryAttempt,
    sessions: Vec<ManagedSession>,
    summary: &mut CodexRecoverySummary,
) -> Result<(), CodexRecoveryError> {
    for session in sessions {
        persist_reconciliation(
            store,
            attempt,
            &session,
            ManagedReconciliationState::Unknown,
            None,
            summary,
        )?;
    }
    Ok(())
}

fn persist_reconciliation(
    store: &mut Store,
    attempt: &CodexRecoveryAttempt,
    session: &ManagedSession,
    state: ManagedReconciliationState,
    latest_turn_id: Option<&str>,
    summary: &mut CodexRecoverySummary,
) -> Result<(), CodexRecoveryError> {
    let digest = format!("{:x}", Sha256::digest(session.id.as_bytes()));
    let gap_event_id = format!("recovery-{}-{digest}-gap", attempt.id);
    let terminal_event_id = matches!(
        state,
        ManagedReconciliationState::Completed
            | ManagedReconciliationState::Failed
            | ManagedReconciliationState::Interrupted
    )
    .then(|| format!("recovery-{}-{digest}-terminal", attempt.id));
    let contract_version = recovery_contract_version(session.executable_version.as_deref());
    let outcome = store.reconcile_managed_session(ManagedSessionReconciliation {
        run_id: session.run_id.clone(),
        session_id: session.id.clone(),
        external_session_key: session.external_session_key.clone(),
        state,
        latest_turn_id: latest_turn_id.map(str::to_owned),
        contract_version,
        observed_at: attempt.observed_at.clone(),
        gap_event_id,
        terminal_event_id,
    })?;
    if matches!(
        outcome,
        ManagedSessionReconciliationOutcome::Duplicate { .. }
    ) {
        summary.duplicate_writes += 1;
    }
    match state {
        ManagedReconciliationState::NoTurns => summary.no_turns += 1,
        ManagedReconciliationState::Completed => summary.completed += 1,
        ManagedReconciliationState::Failed => summary.failed += 1,
        ManagedReconciliationState::Interrupted => summary.interrupted += 1,
        ManagedReconciliationState::Unknown => summary.unknown += 1,
        ManagedReconciliationState::Missing => summary.missing += 1,
        ManagedReconciliationState::ScopeConflict => summary.scope_conflicts += 1,
    }
    Ok(())
}

fn recovery_contract_version(executable_version: Option<&str>) -> String {
    let Some(version) = executable_version else {
        return UNKNOWN_CODEX_CONTRACT_VERSION.to_owned();
    };
    let Some(max_version_bytes) =
        MAX_MANAGED_CONTRACT_VERSION_BYTES.checked_sub(CODEX_CONTRACT_VERSION_PREFIX.len())
    else {
        return UNKNOWN_CODEX_CONTRACT_VERSION.to_owned();
    };
    if version.len() > max_version_bytes {
        return UNKNOWN_CODEX_CONTRACT_VERSION.to_owned();
    }
    format!("{CODEX_CONTRACT_VERSION_PREFIX}{version}")
}

fn validate_attempt(attempt: &CodexRecoveryAttempt) -> Result<(), CodexRecoveryError> {
    if attempt.id.is_empty()
        || attempt.id.len() > MAX_RECOVERY_ATTEMPT_ID_BYTES
        || !attempt
            .id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(CodexRecoveryError::InvalidAttempt("id"));
    }
    if attempt.observed_at.trim().is_empty()
        || attempt.observed_at.len() > MAX_RECOVERY_TIMESTAMP_BYTES
        || attempt.observed_at.chars().any(char::is_control)
    {
        return Err(CodexRecoveryError::InvalidAttempt("observed_at"));
    }
    Ok(())
}
