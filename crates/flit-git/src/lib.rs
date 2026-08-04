use std::{
    error::Error,
    ffi::{OsStr, OsString},
    fmt, fs,
    os::unix::{ffi::OsStringExt, fs::MetadataExt},
    path::{Path, PathBuf},
    time::Duration,
};

use rustix::fs::{Access, AtFlags, CWD, accessat};

mod index;
mod numstat;
mod porcelain;
mod process;
mod runner;

use process::{ProcessError, ProcessPolicy, run_bounded};

pub const GIT_OBSERVATION_TIMEOUT: Duration = Duration::from_secs(3);
pub const MAX_GIT_OUTPUT_BYTES: usize = 1024 * 1024;
pub const MAX_GIT_STATUS_ENTRIES: usize = 10_000;
pub const MAX_GIT_PATH_BYTES: usize = 16 * 1024;
pub const MAX_GIT_CHANGE_COUNT: u64 = 9_007_199_254_740_991;

#[doc(hidden)]
pub fn run_noexec_boundary() -> ! {
    runner::main()
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GitExecutable {
    canonical_path: PathBuf,
    identity: FileIdentity,
}

impl GitExecutable {
    pub fn canonical_path(&self) -> &Path {
        &self.canonical_path
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GitNoExecRunner {
    canonical_path: PathBuf,
    identity: FileIdentity,
}

impl GitNoExecRunner {
    pub fn canonical_path(&self) -> &Path {
        &self.canonical_path
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GitObservation {
    NotWorktree(NotWorktreeReason),
    Repository(RepositoryReceipt),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NotWorktreeReason {
    NotRepository,
    BareRepository,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepositoryReceipt {
    pub canonical_root: PathBuf,
    pub head: GitHead,
    pub dirty: DirtySummary,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GitChangeBaseline {
    receipt: RepositoryReceipt,
    repository_identity: GitRepositoryIdentity,
    project_directory: CanonicalDirectoryIdentity,
    repository_root: CanonicalDirectoryIdentity,
    contains_submodules: bool,
    runner: GitNoExecRunner,
    executable: GitExecutable,
}

impl GitChangeBaseline {
    pub fn receipt(&self) -> &RepositoryReceipt {
        &self.receipt
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GitHead {
    Available(String),
    Unborn,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DirtySummary {
    pub staged: u32,
    pub unstaged: u32,
    pub untracked: u32,
    pub entries: u32,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct GitChangeSummary {
    pub files: u64,
    pub insertions: u64,
    pub deletions: u64,
}

impl DirtySummary {
    pub const fn is_clean(self) -> bool {
        self.entries == 0 && self.staged == 0 && self.unstaged == 0 && self.untracked == 0
    }
}

pub fn inspect_git_on_path(
    path_environment: Option<&OsStr>,
) -> Result<GitExecutable, GitObservationError> {
    let directories = path_environment
        .map(std::env::split_paths)
        .into_iter()
        .flatten()
        .filter(|directory| directory.is_absolute());

    for directory in directories {
        let selected = directory.join("git");
        let Ok(selected_metadata) = fs::metadata(&selected) else {
            continue;
        };
        if !selected_metadata.is_file() || !path_is_executable(&selected) {
            continue;
        }
        let Ok(canonical_path) = fs::canonicalize(&selected) else {
            continue;
        };
        let Ok(metadata) = fs::metadata(&canonical_path) else {
            continue;
        };
        if !metadata.is_file() || !path_is_executable(&canonical_path) {
            continue;
        }
        return Ok(GitExecutable {
            canonical_path,
            identity: FileIdentity::from_metadata(&metadata),
        });
    }

    Err(GitObservationError::GitNotFound)
}

pub fn inspect_noexec_runner_at(
    selected_path: impl AsRef<Path>,
) -> Result<GitNoExecRunner, GitObservationError> {
    let selected_path = selected_path.as_ref();
    if !selected_path.is_absolute() {
        return Err(GitObservationError::RunnerPathNotAbsolute);
    }
    let canonical_path =
        fs::canonicalize(selected_path).map_err(|_| GitObservationError::RunnerUnavailable)?;
    let metadata =
        fs::metadata(&canonical_path).map_err(|_| GitObservationError::RunnerUnavailable)?;
    if !metadata.is_file() || !path_is_executable(&canonical_path) {
        return Err(GitObservationError::RunnerUnavailable);
    }
    Ok(GitNoExecRunner {
        canonical_path,
        identity: FileIdentity::from_metadata(&metadata),
    })
}

pub fn observe_repository(
    runner: &GitNoExecRunner,
    executable: &GitExecutable,
    canonical_project_directory: &Path,
) -> Result<GitObservation, GitObservationError> {
    verify_runner(runner)?;
    verify_executable(executable)?;
    let project_identity = verify_canonical_directory(canonical_project_directory)?;

    let root_output = run_git(
        runner,
        executable,
        root_arguments(canonical_project_directory),
        GitCommandPhase::RepositoryRoot,
    )?;
    if !root_output.status.success() {
        if root_output.status.code() == Some(128)
            && let Some(reason) = classify_not_worktree(&root_output.stderr)
        {
            verify_observation_identities(
                runner,
                executable,
                canonical_project_directory,
                &project_identity,
                None,
            )?;
            return Ok(GitObservation::NotWorktree(reason));
        }
        return Err(GitObservationError::CommandFailed {
            phase: GitCommandPhase::RepositoryRoot,
            exit_code: root_output.status.code(),
        });
    }
    if !root_output.stderr.is_empty() {
        return Err(GitObservationError::UnexpectedCommandStderr {
            phase: GitCommandPhase::RepositoryRoot,
        });
    }
    let canonical_root = parse_repository_root(&root_output.stdout)?;
    if !canonical_project_directory.starts_with(&canonical_root) {
        return Err(GitObservationError::RepositoryRootMismatch);
    }
    let root_identity = verify_canonical_directory(&canonical_root)
        .map_err(|_| GitObservationError::RepositoryRootChanged)?;

    let status_output = run_git(
        runner,
        executable,
        status_arguments(&canonical_root),
        GitCommandPhase::Status,
    )?;
    if !status_output.status.success() {
        return Err(GitObservationError::CommandFailed {
            phase: GitCommandPhase::Status,
            exit_code: status_output.status.code(),
        });
    }
    if !status_output.stderr.is_empty() {
        return Err(GitObservationError::UnexpectedCommandStderr {
            phase: GitCommandPhase::Status,
        });
    }
    let (head, dirty) = porcelain::parse_status(&status_output.stdout)?;
    verify_observation_identities(
        runner,
        executable,
        canonical_project_directory,
        &project_identity,
        Some((&canonical_root, &root_identity)),
    )?;

    Ok(GitObservation::Repository(RepositoryReceipt {
        canonical_root,
        head,
        dirty,
    }))
}

pub fn observe_clean_change_baseline(
    runner: &GitNoExecRunner,
    executable: &GitExecutable,
    canonical_project_directory: &Path,
) -> Result<GitChangeBaseline, GitChangeObservationError> {
    verify_runner(runner)?;
    verify_executable(executable)?;
    let project_identity = verify_canonical_directory(canonical_project_directory)?;
    let initial = match observe_repository(runner, executable, canonical_project_directory)? {
        GitObservation::NotWorktree(reason) => {
            return Err(GitChangeObservationError::BaselineNotWorktree(reason));
        }
        GitObservation::Repository(receipt) => receipt,
    };
    match &initial.head {
        GitHead::Available(oid) if porcelain::valid_object_id(oid.as_bytes()) => oid,
        GitHead::Available(_) => return Err(GitChangeObservationError::BaselineHeadInvalid),
        GitHead::Unborn => return Err(GitChangeObservationError::BaselineHeadUnavailable),
    };
    if !initial.dirty.is_clean() {
        return Err(GitChangeObservationError::BaselineNotClean);
    }
    let root_identity = verify_canonical_directory(&initial.canonical_root)
        .map_err(|_| GitChangeObservationError::BaselineRepositoryMismatch)?;
    let repository_identity_before = observe_repository_identity(
        runner,
        executable,
        &initial.canonical_root,
        GitCommandPhase::RepositoryIdentity,
    )?;
    let baseline_index = observe_index(runner, executable, &initial.canonical_root)?;
    let receipt = match observe_repository(runner, executable, canonical_project_directory)? {
        GitObservation::NotWorktree(reason) => {
            return Err(GitChangeObservationError::BaselineNotWorktree(reason));
        }
        GitObservation::Repository(receipt) => receipt,
    };
    if receipt != initial {
        return Err(GitChangeObservationError::RepositoryChangedDuringObservation);
    }
    let repository_identity_after = observe_repository_identity(
        runner,
        executable,
        &receipt.canonical_root,
        GitCommandPhase::RepositoryIdentity,
    )?;
    if repository_identity_after != repository_identity_before {
        return Err(GitChangeObservationError::RepositoryIdentityChanged);
    }
    verify_observation_identities(
        runner,
        executable,
        canonical_project_directory,
        &project_identity,
        Some((&receipt.canonical_root, &root_identity)),
    )?;

    Ok(GitChangeBaseline {
        receipt,
        repository_identity: repository_identity_before,
        project_directory: CanonicalDirectoryIdentity {
            canonical_path: canonical_project_directory.to_owned(),
            identity: StableDirectoryIdentity::from_file_identity(&project_identity),
        },
        repository_root: CanonicalDirectoryIdentity {
            canonical_path: initial.canonical_root,
            identity: StableDirectoryIdentity::from_file_identity(&root_identity),
        },
        contains_submodules: baseline_index.contains_submodules,
        runner: runner.clone(),
        executable: executable.clone(),
    })
}

pub fn observe_changes_since_clean_baseline(
    runner: &GitNoExecRunner,
    executable: &GitExecutable,
    canonical_project_directory: &Path,
    baseline: &GitChangeBaseline,
) -> Result<GitChangeSummary, GitChangeObservationError> {
    if runner != &baseline.runner {
        return Err(GitChangeObservationError::BaselineRunnerMismatch);
    }
    if executable != &baseline.executable {
        return Err(GitChangeObservationError::BaselineGitExecutableMismatch);
    }
    verify_baseline_worktree_identity(canonical_project_directory, baseline)?;
    let receipt = &baseline.receipt;
    let GitHead::Available(baseline_oid) = &receipt.head else {
        unreachable!("GitChangeBaseline always contains an available HEAD")
    };
    verify_runner(runner)?;
    verify_executable(executable)?;
    let project_identity = verify_canonical_directory(canonical_project_directory)?;
    let root_identity = verify_canonical_directory(&receipt.canonical_root)
        .map_err(|_| GitChangeObservationError::RepositoryIdentityChanged)?;
    if !canonical_project_directory.starts_with(&receipt.canonical_root) {
        return Err(GitChangeObservationError::TerminalRepositoryMismatch);
    }

    let terminal_before = match observe_repository(runner, executable, canonical_project_directory)?
    {
        GitObservation::NotWorktree(reason) => {
            return Err(GitChangeObservationError::TerminalNotWorktree(reason));
        }
        GitObservation::Repository(receipt) => receipt,
    };
    verify_observation_identities(
        runner,
        executable,
        canonical_project_directory,
        &project_identity,
        Some((&receipt.canonical_root, &root_identity)),
    )?;
    verify_repository_identity(
        runner,
        executable,
        &receipt.canonical_root,
        &baseline.repository_identity,
    )?;
    verify_baseline_worktree_identity(canonical_project_directory, baseline)?;
    if baseline.contains_submodules {
        return Err(GitChangeObservationError::SubmodulesUnsupportedForChanges);
    }
    if observe_index(runner, executable, &receipt.canonical_root)?.contains_submodules {
        return Err(GitChangeObservationError::SubmodulesUnsupportedForChanges);
    }
    if terminal_before.canonical_root != receipt.canonical_root {
        return Err(GitChangeObservationError::TerminalRepositoryMismatch);
    }
    if terminal_before.dirty.untracked != 0 {
        return Err(GitChangeObservationError::TerminalUntracked);
    }
    if terminal_before.dirty.staged != 0 && terminal_before.dirty.unstaged != 0 {
        return Err(GitChangeObservationError::MixedIndexWorktreeChangesUnsupported);
    }

    let first = observe_numstat(runner, executable, &receipt.canonical_root, baseline_oid)?;
    let terminal_middle = observe_status_at_root(
        runner,
        executable,
        canonical_project_directory,
        &project_identity,
        &receipt.canonical_root,
        &root_identity,
    )?;
    if terminal_middle != (terminal_before.head.clone(), terminal_before.dirty)
        || terminal_middle.1.untracked != 0
    {
        return Err(GitChangeObservationError::RepositoryChangedDuringObservation);
    }

    verify_repository_identity(
        runner,
        executable,
        &receipt.canonical_root,
        &baseline.repository_identity,
    )?;
    verify_baseline_worktree_identity(canonical_project_directory, baseline)?;
    if observe_index(runner, executable, &receipt.canonical_root)?.contains_submodules {
        return Err(GitChangeObservationError::SubmodulesUnsupportedForChanges);
    }
    let second = observe_numstat(runner, executable, &receipt.canonical_root, baseline_oid)?;
    if second != first {
        return Err(GitChangeObservationError::RepositoryChangedDuringObservation);
    }
    let terminal_after = observe_status_at_root(
        runner,
        executable,
        canonical_project_directory,
        &project_identity,
        &receipt.canonical_root,
        &root_identity,
    )?;
    if terminal_after != (terminal_before.head, terminal_before.dirty)
        || terminal_after.1.untracked != 0
    {
        return Err(GitChangeObservationError::RepositoryChangedDuringObservation);
    }

    verify_repository_identity(
        runner,
        executable,
        &receipt.canonical_root,
        &baseline.repository_identity,
    )?;
    verify_baseline_worktree_identity(canonical_project_directory, baseline)?;
    if observe_index(runner, executable, &receipt.canonical_root)?.contains_submodules {
        return Err(GitChangeObservationError::SubmodulesUnsupportedForChanges);
    }
    numstat::parse(&first)
}

fn observe_index(
    runner: &GitNoExecRunner,
    executable: &GitExecutable,
    canonical_root: &Path,
) -> Result<index::IndexSummary, GitChangeObservationError> {
    let output = run_git(
        runner,
        executable,
        index_arguments(canonical_root),
        GitCommandPhase::Index,
    )?;
    let stdout = require_successful_output(output, GitCommandPhase::Index)?;
    index::parse(&stdout)
}

fn verify_baseline_worktree_identity(
    canonical_project_directory: &Path,
    baseline: &GitChangeBaseline,
) -> Result<(), GitChangeObservationError> {
    let project = stable_directory_identity(canonical_project_directory)
        .map_err(|_| GitChangeObservationError::BaselineProjectIdentityChanged)?;
    if project != baseline.project_directory {
        return Err(GitChangeObservationError::BaselineProjectIdentityChanged);
    }
    let root = stable_directory_identity(&baseline.receipt.canonical_root)
        .map_err(|_| GitChangeObservationError::BaselineRepositoryRootIdentityChanged)?;
    if root != baseline.repository_root {
        return Err(GitChangeObservationError::BaselineRepositoryRootIdentityChanged);
    }
    Ok(())
}

fn stable_directory_identity(
    path: &Path,
) -> Result<CanonicalDirectoryIdentity, GitChangeObservationError> {
    if !path.is_absolute() {
        return Err(GitObservationError::ProjectDirectoryNotCanonical.into());
    }
    let canonical =
        fs::canonicalize(path).map_err(|_| GitObservationError::ProjectDirectoryUnavailable)?;
    let metadata =
        fs::metadata(&canonical).map_err(|_| GitObservationError::ProjectDirectoryUnavailable)?;
    if canonical != path || !metadata.is_dir() {
        return Err(GitObservationError::ProjectDirectoryNotCanonical.into());
    }
    Ok(CanonicalDirectoryIdentity {
        canonical_path: canonical,
        identity: StableDirectoryIdentity::from_metadata(&metadata),
    })
}

fn verify_repository_identity(
    runner: &GitNoExecRunner,
    executable: &GitExecutable,
    canonical_root: &Path,
    expected: &GitRepositoryIdentity,
) -> Result<(), GitChangeObservationError> {
    let current = observe_repository_identity(
        runner,
        executable,
        canonical_root,
        GitCommandPhase::RepositoryIdentity,
    )?;
    if &current == expected {
        Ok(())
    } else {
        Err(GitChangeObservationError::RepositoryIdentityChanged)
    }
}

fn observe_repository_identity(
    runner: &GitNoExecRunner,
    executable: &GitExecutable,
    canonical_root: &Path,
    phase: GitCommandPhase,
) -> Result<GitRepositoryIdentity, GitChangeObservationError> {
    let output = run_git(
        runner,
        executable,
        repository_identity_arguments(canonical_root),
        phase,
    )?;
    let stdout = require_successful_output(output, phase)?;
    parse_repository_identity(&stdout)
}

fn parse_repository_identity(
    output: &[u8],
) -> Result<GitRepositoryIdentity, GitChangeObservationError> {
    let Some(output) = output.strip_suffix(b"\n") else {
        return Err(GitChangeObservationError::MalformedRepositoryIdentity);
    };
    let paths = output.split(|byte| *byte == b'\n').collect::<Vec<_>>();
    if paths.len() != 2 {
        return Err(GitChangeObservationError::MalformedRepositoryIdentity);
    }
    Ok(GitRepositoryIdentity {
        git_directory: canonical_directory_identity(paths[0])?,
        common_directory: canonical_directory_identity(paths[1])?,
    })
}

fn canonical_directory_identity(
    value: &[u8],
) -> Result<CanonicalDirectoryIdentity, GitChangeObservationError> {
    if value.is_empty() || value.contains(&0) {
        return Err(GitChangeObservationError::MalformedRepositoryIdentity);
    }
    if value.len() > MAX_GIT_PATH_BYTES {
        return Err(GitObservationError::GitPathTooLong.into());
    }
    let path = PathBuf::from(OsString::from_vec(value.to_vec()));
    if !path.is_absolute() {
        return Err(GitChangeObservationError::MalformedRepositoryIdentity);
    }
    let canonical = fs::canonicalize(&path)
        .map_err(|_| GitChangeObservationError::RepositoryIdentityChanged)?;
    let metadata = fs::metadata(&canonical)
        .map_err(|_| GitChangeObservationError::RepositoryIdentityChanged)?;
    if canonical != path || !metadata.is_dir() {
        return Err(GitChangeObservationError::MalformedRepositoryIdentity);
    }
    Ok(CanonicalDirectoryIdentity {
        canonical_path: canonical,
        identity: StableDirectoryIdentity::from_metadata(&metadata),
    })
}

fn observe_status_at_root(
    runner: &GitNoExecRunner,
    executable: &GitExecutable,
    canonical_project_directory: &Path,
    project_identity: &FileIdentity,
    canonical_root: &Path,
    root_identity: &FileIdentity,
) -> Result<(GitHead, DirtySummary), GitChangeObservationError> {
    let output = run_git(
        runner,
        executable,
        status_arguments(canonical_root),
        GitCommandPhase::Status,
    )?;
    let stdout = require_successful_output(output, GitCommandPhase::Status)?;
    let receipt = porcelain::parse_status(&stdout)?;
    verify_observation_identities(
        runner,
        executable,
        canonical_project_directory,
        project_identity,
        Some((canonical_root, root_identity)),
    )?;
    Ok(receipt)
}

fn observe_numstat(
    runner: &GitNoExecRunner,
    executable: &GitExecutable,
    canonical_root: &Path,
    baseline_oid: &str,
) -> Result<Vec<u8>, GitChangeObservationError> {
    let output = run_git(
        runner,
        executable,
        numstat_arguments(canonical_root, baseline_oid),
        GitCommandPhase::ChangeSummary,
    )?;
    Ok(require_successful_output(
        output,
        GitCommandPhase::ChangeSummary,
    )?)
}

fn require_successful_output(
    output: process::ProcessOutput,
    phase: GitCommandPhase,
) -> Result<Vec<u8>, GitObservationError> {
    if !output.status.success() {
        return Err(GitObservationError::CommandFailed {
            phase,
            exit_code: output.status.code(),
        });
    }
    if !output.stderr.is_empty() {
        return Err(GitObservationError::UnexpectedCommandStderr { phase });
    }
    Ok(output.stdout)
}

fn classify_not_worktree(stderr: &[u8]) -> Option<NotWorktreeReason> {
    if stderr == b"fatal: not a git repository (or any of the parent directories): .git\n" {
        return Some(NotWorktreeReason::NotRepository);
    }
    if stderr == b"fatal: this operation must be run in a work tree\n" {
        return Some(NotWorktreeReason::BareRepository);
    }

    const MOUNT_PREFIX: &[u8] = b"fatal: not a git repository (or any parent up to mount point ";
    const MOUNT_SUFFIX: &[u8] =
        b")\nStopping at filesystem boundary (GIT_DISCOVERY_ACROSS_FILESYSTEM not set).\n";
    let mount_path = stderr
        .strip_prefix(MOUNT_PREFIX)?
        .strip_suffix(MOUNT_SUFFIX)?;
    if mount_path.is_empty() || mount_path.len() > MAX_GIT_PATH_BYTES || mount_path.contains(&0) {
        return None;
    }
    let mount_path = PathBuf::from(OsString::from_vec(mount_path.to_vec()));
    mount_path
        .is_absolute()
        .then_some(NotWorktreeReason::NotRepository)
}

fn root_arguments(project: &Path) -> Vec<OsString> {
    base_arguments(project)
        .into_iter()
        .chain([
            OsString::from("rev-parse"),
            OsString::from("--path-format=absolute"),
            OsString::from("--show-toplevel"),
        ])
        .collect()
}

fn status_arguments(root: &Path) -> Vec<OsString> {
    base_arguments(root)
        .into_iter()
        .chain([
            OsString::from("status"),
            OsString::from("--porcelain=v2"),
            OsString::from("-z"),
            OsString::from("--branch"),
            OsString::from("--untracked-files=all"),
            OsString::from("--ignore-submodules=all"),
        ])
        .collect()
}

fn numstat_arguments(root: &Path, baseline_oid: &str) -> Vec<OsString> {
    base_arguments(root)
        .into_iter()
        .chain([
            OsString::from("diff"),
            OsString::from("--numstat"),
            OsString::from("-z"),
            OsString::from("--no-renames"),
            OsString::from("--no-ext-diff"),
            OsString::from("--no-textconv"),
            OsString::from("--ignore-submodules=all"),
            OsString::from(baseline_oid),
            OsString::from("--"),
        ])
        .collect()
}

fn repository_identity_arguments(root: &Path) -> Vec<OsString> {
    base_arguments(root)
        .into_iter()
        .chain([
            OsString::from("rev-parse"),
            OsString::from("--path-format=absolute"),
            OsString::from("--git-dir"),
            OsString::from("--git-common-dir"),
        ])
        .collect()
}

fn index_arguments(root: &Path) -> Vec<OsString> {
    base_arguments(root)
        .into_iter()
        .chain([
            OsString::from("ls-files"),
            OsString::from("--stage"),
            OsString::from("-v"),
            OsString::from("-z"),
            OsString::from("--"),
        ])
        .collect()
}

fn base_arguments(directory: &Path) -> Vec<OsString> {
    vec![
        OsString::from("--no-optional-locks"),
        OsString::from("--no-replace-objects"),
        OsString::from("-c"),
        OsString::from("core.fsmonitor=false"),
        OsString::from("-c"),
        OsString::from("core.trustctime=true"),
        OsString::from("-c"),
        OsString::from("core.checkStat=default"),
        OsString::from("-c"),
        OsString::from("core.filemode=true"),
        OsString::from("-c"),
        OsString::from("core.untrackedCache=false"),
        OsString::from("-c"),
        OsString::from("status.relativePaths=false"),
        OsString::from("-c"),
        OsString::from("status.renames=true"),
        OsString::from("-C"),
        directory.as_os_str().to_owned(),
    ]
}

fn run_git(
    runner: &GitNoExecRunner,
    executable: &GitExecutable,
    arguments: Vec<OsString>,
    phase: GitCommandPhase,
) -> Result<process::ProcessOutput, GitObservationError> {
    verify_runner(runner)?;
    verify_executable(executable)?;
    let output = run_bounded(
        &runner.canonical_path,
        &runner::arguments(executable, arguments),
        ProcessPolicy {
            timeout: GIT_OBSERVATION_TIMEOUT,
            max_output_bytes: MAX_GIT_OUTPUT_BYTES,
        },
    )
    .map_err(|error| map_process_error(phase, error))?;
    if let Some(failure) = runner::decode_failure(&output) {
        return Err(GitObservationError::RunnerBoundaryFailed { phase, failure });
    }
    Ok(output)
}

fn map_process_error(phase: GitCommandPhase, error: ProcessError) -> GitObservationError {
    match error {
        ProcessError::TimedOut => GitObservationError::CommandTimedOut { phase },
        ProcessError::OutputTooLarge => GitObservationError::CommandOutputTooLarge { phase },
        ProcessError::OutputDrainTimedOut => {
            GitObservationError::CommandOutputDrainTimedOut { phase }
        }
        ProcessError::Spawn => GitObservationError::CommandSpawnFailed { phase },
        ProcessError::MissingOutputPipe
        | ProcessError::Wait
        | ProcessError::ReadOutput
        | ProcessError::ConfigureOutput
        | ProcessError::TerminateProcessGroup => GitObservationError::CommandIoFailed { phase },
    }
}

fn parse_repository_root(stdout: &[u8]) -> Result<PathBuf, GitObservationError> {
    let Some(without_newline) = stdout.strip_suffix(b"\n") else {
        return Err(GitObservationError::MalformedRepositoryRoot);
    };
    if without_newline.is_empty() || without_newline.contains(&0) {
        return Err(GitObservationError::MalformedRepositoryRoot);
    }
    if without_newline.len() > MAX_GIT_PATH_BYTES {
        return Err(GitObservationError::GitPathTooLong);
    }
    let path = PathBuf::from(OsString::from_vec(without_newline.to_vec()));
    if !path.is_absolute() {
        return Err(GitObservationError::MalformedRepositoryRoot);
    }
    let canonical =
        fs::canonicalize(&path).map_err(|_| GitObservationError::RepositoryRootChanged)?;
    if canonical != path {
        return Err(GitObservationError::RepositoryRootMismatch);
    }
    Ok(path)
}

fn verify_executable(executable: &GitExecutable) -> Result<(), GitObservationError> {
    if executable_matches(&executable.canonical_path, &executable.identity) {
        Ok(())
    } else {
        Err(GitObservationError::GitExecutableChanged)
    }
}

fn verify_runner(runner: &GitNoExecRunner) -> Result<(), GitObservationError> {
    if executable_matches(&runner.canonical_path, &runner.identity) {
        Ok(())
    } else {
        Err(GitObservationError::RunnerChanged)
    }
}

fn verify_canonical_directory(path: &Path) -> Result<FileIdentity, GitObservationError> {
    if !path.is_absolute() {
        return Err(GitObservationError::ProjectDirectoryNotCanonical);
    }
    let canonical =
        fs::canonicalize(path).map_err(|_| GitObservationError::ProjectDirectoryUnavailable)?;
    let metadata =
        fs::metadata(&canonical).map_err(|_| GitObservationError::ProjectDirectoryUnavailable)?;
    if canonical != path || !metadata.is_dir() {
        return Err(GitObservationError::ProjectDirectoryNotCanonical);
    }
    Ok(FileIdentity::from_metadata(&metadata))
}

fn verify_observation_identities(
    runner: &GitNoExecRunner,
    executable: &GitExecutable,
    project: &Path,
    project_identity: &FileIdentity,
    root: Option<(&Path, &FileIdentity)>,
) -> Result<(), GitObservationError> {
    verify_runner(runner)?;
    verify_executable(executable)?;
    let current_project = verify_canonical_directory(project)
        .map_err(|_| GitObservationError::ProjectDirectoryChanged)?;
    if &current_project != project_identity {
        return Err(GitObservationError::ProjectDirectoryChanged);
    }
    if let Some((root, root_identity)) = root {
        let current_root = verify_canonical_directory(root)
            .map_err(|_| GitObservationError::RepositoryRootChanged)?;
        if &current_root != root_identity {
            return Err(GitObservationError::RepositoryRootChanged);
        }
    }
    Ok(())
}

fn path_is_executable(path: &Path) -> bool {
    accessat(CWD, path, Access::EXEC_OK, AtFlags::EACCESS).is_ok()
}

fn executable_matches(path: &Path, identity: &FileIdentity) -> bool {
    let Ok(canonical) = fs::canonicalize(path) else {
        return false;
    };
    let Ok(metadata) = fs::metadata(&canonical) else {
        return false;
    };
    canonical == path
        && metadata.is_file()
        && path_is_executable(&canonical)
        && FileIdentity::from_metadata(&metadata) == *identity
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct FileIdentity {
    device: u64,
    inode: u64,
    length: u64,
    modified_seconds: i64,
    modified_nanoseconds: i64,
    changed_seconds: i64,
    changed_nanoseconds: i64,
    mode: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct GitRepositoryIdentity {
    git_directory: CanonicalDirectoryIdentity,
    common_directory: CanonicalDirectoryIdentity,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct CanonicalDirectoryIdentity {
    canonical_path: PathBuf,
    identity: StableDirectoryIdentity,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct StableDirectoryIdentity {
    device: u64,
    inode: u64,
    mode: u32,
}

impl StableDirectoryIdentity {
    fn from_metadata(metadata: &fs::Metadata) -> Self {
        Self {
            device: metadata.dev(),
            inode: metadata.ino(),
            mode: metadata.mode(),
        }
    }

    fn from_file_identity(identity: &FileIdentity) -> Self {
        Self {
            device: identity.device,
            inode: identity.inode,
            mode: identity.mode,
        }
    }
}

impl FileIdentity {
    fn from_metadata(metadata: &fs::Metadata) -> Self {
        Self {
            device: metadata.dev(),
            inode: metadata.ino(),
            length: metadata.len(),
            modified_seconds: metadata.mtime(),
            modified_nanoseconds: metadata.mtime_nsec(),
            changed_seconds: metadata.ctime(),
            changed_nanoseconds: metadata.ctime_nsec(),
            mode: metadata.mode(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GitCommandPhase {
    RepositoryRoot,
    RepositoryIdentity,
    Index,
    Status,
    ChangeSummary,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GitRunnerFailure {
    InvalidArguments,
    RootUnsupported,
    LimitSetFailed,
    LimitVerificationFailed,
    GitIdentityChanged,
    ExecFailed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GitObservationError {
    GitNotFound,
    GitExecutableChanged,
    RunnerPathNotAbsolute,
    RunnerUnavailable,
    RunnerChanged,
    ProjectDirectoryUnavailable,
    ProjectDirectoryNotCanonical,
    ProjectDirectoryChanged,
    RepositoryRootChanged,
    RepositoryRootMismatch,
    MalformedRepositoryRoot,
    RunnerBoundaryFailed {
        phase: GitCommandPhase,
        failure: GitRunnerFailure,
    },
    CommandSpawnFailed {
        phase: GitCommandPhase,
    },
    CommandIoFailed {
        phase: GitCommandPhase,
    },
    CommandTimedOut {
        phase: GitCommandPhase,
    },
    CommandOutputDrainTimedOut {
        phase: GitCommandPhase,
    },
    CommandOutputTooLarge {
        phase: GitCommandPhase,
    },
    CommandFailed {
        phase: GitCommandPhase,
        exit_code: Option<i32>,
    },
    UnexpectedCommandStderr {
        phase: GitCommandPhase,
    },
    MalformedPorcelain,
    DuplicatePorcelainRecord,
    TooManyPorcelainEntries,
    GitPathTooLong,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GitChangeObservationError {
    Observation(GitObservationError),
    BaselineHeadUnavailable,
    BaselineHeadInvalid,
    BaselineNotClean,
    BaselineRepositoryMismatch,
    BaselineNotWorktree(NotWorktreeReason),
    TerminalNotWorktree(NotWorktreeReason),
    TerminalRepositoryMismatch,
    TerminalUntracked,
    RepositoryChangedDuringObservation,
    MalformedNumstat,
    DuplicateNumstatRecord,
    BinaryNumstat,
    TooManyNumstatEntries,
    GitChangeCountTooLarge,
    MalformedRepositoryIdentity,
    RepositoryIdentityChanged,
    BaselineRunnerMismatch,
    BaselineGitExecutableMismatch,
    BaselineProjectIdentityChanged,
    BaselineRepositoryRootIdentityChanged,
    MalformedIndex,
    DuplicateIndexRecord,
    TooManyIndexEntries,
    UnmergedIndex,
    SubmodulesUnsupportedForChanges,
    MixedIndexWorktreeChangesUnsupported,
    IndexFlagsUnsupportedForChanges,
}

impl From<GitObservationError> for GitChangeObservationError {
    fn from(error: GitObservationError) -> Self {
        Self::Observation(error)
    }
}

impl fmt::Display for GitObservationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::GitNotFound => "installed Git was not found",
            Self::GitExecutableChanged => "the selected Git executable changed",
            Self::RunnerPathNotAbsolute => "the no-child-exec runner path is not absolute",
            Self::RunnerUnavailable => "the no-child-exec runner is unavailable",
            Self::RunnerChanged => "the selected no-child-exec runner changed",
            Self::ProjectDirectoryUnavailable => "the Project directory is unavailable",
            Self::ProjectDirectoryNotCanonical => "the Project directory is not canonical",
            Self::ProjectDirectoryChanged => "the Project directory changed during observation",
            Self::RepositoryRootChanged => "the repository root changed during observation",
            Self::RepositoryRootMismatch => {
                "the repository root identity does not match the Project"
            }
            Self::MalformedRepositoryRoot => "Git returned a malformed repository root",
            Self::RunnerBoundaryFailed { .. } => "the no-child-exec runner boundary failed",
            Self::CommandSpawnFailed { .. } => "the Git command could not be started",
            Self::CommandIoFailed { .. } => "the Git command could not be observed safely",
            Self::CommandTimedOut { .. } => "the Git command timed out",
            Self::CommandOutputDrainTimedOut { .. } => {
                "the Git command output did not close in time"
            }
            Self::CommandOutputTooLarge { .. } => "the Git command output exceeded its bound",
            Self::CommandFailed { .. } => "the Git command failed",
            Self::UnexpectedCommandStderr { .. } => {
                "the Git command returned unexpected diagnostic output"
            }
            Self::MalformedPorcelain => "Git returned malformed porcelain output",
            Self::DuplicatePorcelainRecord => "Git returned a duplicate porcelain record",
            Self::TooManyPorcelainEntries => "Git returned too many porcelain entries",
            Self::GitPathTooLong => "Git returned a path that exceeded its bound",
        })
    }
}

impl Error for GitObservationError {}

impl fmt::Display for GitChangeObservationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Self::Observation(error) = self {
            return error.fmt(formatter);
        }
        formatter.write_str(match self {
            Self::Observation(_) => unreachable!("handled above"),
            Self::BaselineHeadUnavailable => "the Git baseline has no available HEAD",
            Self::BaselineHeadInvalid => "the Git baseline HEAD is invalid",
            Self::BaselineNotClean => "the Git baseline is not clean",
            Self::BaselineRepositoryMismatch => {
                "the Git baseline repository does not match the Project"
            }
            Self::BaselineNotWorktree(_) => "the baseline Project is not a Git worktree",
            Self::TerminalNotWorktree(_) => "the terminal Project is not a Git worktree",
            Self::TerminalRepositoryMismatch => {
                "the terminal Git repository does not match the baseline"
            }
            Self::TerminalUntracked => "the terminal Git observation contains untracked files",
            Self::RepositoryChangedDuringObservation => {
                "the repository changed during terminal Git observation"
            }
            Self::MalformedNumstat => "Git returned malformed numstat output",
            Self::DuplicateNumstatRecord => "Git returned a duplicate numstat record",
            Self::BinaryNumstat => "Git returned a binary numstat record",
            Self::TooManyNumstatEntries => "Git returned too many numstat entries",
            Self::GitChangeCountTooLarge => "Git returned a change count that exceeded its bound",
            Self::MalformedRepositoryIdentity => "Git returned a malformed repository identity",
            Self::RepositoryIdentityChanged => {
                "the Git repository identity changed during observation"
            }
            Self::BaselineRunnerMismatch => {
                "the terminal no-child-exec runner differs from the baseline"
            }
            Self::BaselineGitExecutableMismatch => {
                "the terminal Git executable differs from the baseline"
            }
            Self::BaselineProjectIdentityChanged => {
                "the baseline Project directory identity changed"
            }
            Self::BaselineRepositoryRootIdentityChanged => {
                "the baseline repository root identity changed"
            }
            Self::MalformedIndex => "Git returned malformed index output",
            Self::DuplicateIndexRecord => "Git returned a duplicate index record",
            Self::TooManyIndexEntries => "Git returned too many index entries",
            Self::UnmergedIndex => "the terminal Git index contains unmerged entries",
            Self::SubmodulesUnsupportedForChanges => {
                "Git submodules are unavailable for exact change aggregation"
            }
            Self::MixedIndexWorktreeChangesUnsupported => {
                "mixed staged and unstaged changes are unavailable for exact aggregation"
            }
            Self::IndexFlagsUnsupportedForChanges => {
                "Git index flags are unavailable for exact change aggregation"
            }
        })
    }
}

impl Error for GitChangeObservationError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Observation(error) => Some(error),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{GitChangeObservationError, parse_repository_identity};

    #[test]
    fn rejects_malformed_repository_identity_output() {
        for malformed in [
            b"".as_slice(),
            b"/only-one-path\n",
            b"relative\nrelative\n",
            b"/git\0dir\n/common\n",
            b"/git\n/common\nextra\n",
        ] {
            assert_eq!(
                parse_repository_identity(malformed).expect_err("malformed repository identity"),
                GitChangeObservationError::MalformedRepositoryIdentity
            );
        }
    }
}
