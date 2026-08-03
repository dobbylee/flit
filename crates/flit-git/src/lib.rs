use std::{
    error::Error,
    ffi::{OsStr, OsString},
    fmt, fs,
    os::unix::{ffi::OsStringExt, fs::MetadataExt},
    path::{Path, PathBuf},
    time::Duration,
};

use rustix::fs::{Access, AtFlags, CWD, accessat};

mod porcelain;
mod process;
mod runner;

use process::{ProcessError, ProcessPolicy, run_bounded};

pub const GIT_OBSERVATION_TIMEOUT: Duration = Duration::from_secs(3);
pub const MAX_GIT_OUTPUT_BYTES: usize = 1024 * 1024;
pub const MAX_GIT_STATUS_ENTRIES: usize = 10_000;
pub const MAX_GIT_PATH_BYTES: usize = 16 * 1024;

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

fn base_arguments(directory: &Path) -> Vec<OsString> {
    vec![
        OsString::from("--no-optional-locks"),
        OsString::from("-c"),
        OsString::from("core.fsmonitor=false"),
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
    Status,
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
