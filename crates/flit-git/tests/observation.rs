use std::{
    ffi::OsStr,
    fs,
    os::unix::{fs::PermissionsExt, fs::symlink},
    path::{Path, PathBuf},
    process::{self, Command},
    sync::atomic::{AtomicU64, Ordering},
};

use flit_git::{
    DirtySummary, GitCommandPhase, GitHead, GitObservation, GitObservationError, GitRunnerFailure,
    NotWorktreeReason, inspect_git_on_path, inspect_noexec_runner_at, observe_repository,
};

static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(1);

struct TestWorkspace {
    directory: PathBuf,
}

impl TestWorkspace {
    fn new(label: &str) -> Self {
        let nonce = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let directory =
            std::env::temp_dir().join(format!("flit-git-{label}-{}-{nonce}", process::id()));
        fs::create_dir(&directory).expect("unique test directory");
        Self { directory }
    }

    fn canonical_directory(&self) -> PathBuf {
        fs::canonicalize(&self.directory).expect("canonical test directory")
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
fn observes_non_repository_unborn_clean_and_exact_dirty_categories() {
    let executable = installed_git();
    let workspace = TestWorkspace::new("actual");
    let root = workspace.canonical_directory();

    assert_eq!(
        observe(&executable, &root).expect("non-repository observation"),
        GitObservation::NotWorktree(NotWorktreeReason::NotRepository)
    );

    git(&executable, &root, &["init", "-b", "main"]);
    let GitObservation::Repository(unborn) =
        observe(&executable, &root).expect("unborn observation")
    else {
        panic!("expected a repository receipt");
    };
    assert_eq!(unborn.canonical_root, root);
    assert_eq!(unborn.head, GitHead::Unborn);
    assert!(unborn.dirty.is_clean());

    fs::write(root.join("tracked.txt"), b"baseline\n").expect("tracked file");
    fs::write(root.join("rename source.txt"), b"rename\n").expect("rename source");
    git(&executable, &root, &["add", "--", "."]);
    git_with_identity(&executable, &root, &["commit", "-m", "baseline"]);

    let GitObservation::Repository(clean) = observe(&executable, &root).expect("clean observation")
    else {
        panic!("expected a repository receipt");
    };
    assert!(
        matches!(clean.head, GitHead::Available(ref oid) if oid.len() == 40 || oid.len() == 64)
    );
    assert!(clean.dirty.is_clean());

    git(
        &executable,
        &root,
        &["mv", "--", "rename source.txt", "rename target.txt"],
    );
    fs::write(root.join("rename source.txt"), b"recreated\n").expect("recreated rename source");
    let GitObservation::Repository(move_and_recreate) =
        observe(&executable, &root).expect("move and recreate observation")
    else {
        panic!("expected a repository receipt");
    };
    assert_eq!(
        move_and_recreate.dirty,
        DirtySummary {
            staged: 1,
            unstaged: 0,
            untracked: 1,
            entries: 2,
        }
    );

    fs::write(root.join("tracked.txt"), b"unstaged\n").expect("unstaged file");

    let GitObservation::Repository(dirty) = observe(&executable, &root).expect("dirty observation")
    else {
        panic!("expected a repository receipt");
    };
    assert_eq!(
        dirty.dirty,
        DirtySummary {
            staged: 1,
            unstaged: 1,
            untracked: 1,
            entries: 3,
        }
    );
}

#[test]
fn configured_content_filter_is_blocked_and_status_fails_closed_on_stderr() {
    let executable = installed_git();
    let workspace = TestWorkspace::new("filter");
    let root = workspace.canonical_directory();
    let marker = root.join("filter-must-not-run");
    git(&executable, &root, &["init", "-b", "main"]);
    fs::write(root.join("tracked.txt"), b"baseline\n").expect("tracked file");
    fs::write(
        root.join(".gitattributes"),
        b"tracked.txt filter=sideeffect\n",
    )
    .expect("attributes");
    git(&executable, &root, &["add", "--", "."]);
    git_with_identity(&executable, &root, &["commit", "-m", "baseline"]);
    let filter_command = format!("/usr/bin/touch {}; /bin/cat", marker.display());
    git(
        &executable,
        &root,
        &["config", "filter.sideeffect.clean", &filter_command],
    );
    fs::write(root.join("tracked.txt"), b"changed?\n").expect("same-length edit");

    assert_eq!(
        observe(&executable, &root).expect_err("configured filter"),
        GitObservationError::UnexpectedCommandStderr {
            phase: GitCommandPhase::Status
        }
    );
    assert!(!marker.exists(), "configured filter must not execute");
}

#[test]
fn missing_promisor_blob_fails_without_lazy_fetch_or_remote_helper_execution() {
    let executable = installed_git();
    let workspace = TestWorkspace::new("promisor");
    let root = workspace.canonical_directory();
    let marker = root.join("upload-pack-must-not-run");
    let upload_pack = root.join("marker-upload-pack");
    git(&executable, &root, &["init", "-b", "main"]);
    fs::write(root.join("old.txt"), b"original content\n").expect("baseline file");
    git(&executable, &root, &["add", "--", "."]);
    git_with_identity(&executable, &root, &["commit", "-m", "baseline"]);
    let source_blob = git_stdout(&executable, &root, &["rev-parse", "HEAD:old.txt"]);

    fs::remove_file(root.join("old.txt")).expect("remove old worktree file");
    fs::write(root.join("new.txt"), b"original changed\n").expect("near-rename file");
    git(&executable, &root, &["add", "-A"]);
    write_executable(
        &upload_pack,
        format!("#!/bin/sh\n/usr/bin/touch '{}'\nexit 1\n", marker.display()).as_bytes(),
    );
    git(
        &executable,
        &root,
        &["config", "core.repositoryformatversion", "1"],
    );
    git(
        &executable,
        &root,
        &["config", "extensions.partialClone", "origin"],
    );
    git(
        &executable,
        &root,
        &["config", "remote.origin.promisor", "true"],
    );
    git(
        &executable,
        &root,
        &["config", "remote.origin.partialclonefilter", "blob:none"],
    );
    git(
        &executable,
        &root,
        &[
            "config",
            "remote.origin.url",
            root.to_str().expect("UTF-8 root"),
        ],
    );
    git(
        &executable,
        &root,
        &[
            "config",
            "remote.origin.uploadpack",
            upload_pack.to_str().expect("UTF-8 upload-pack path"),
        ],
    );
    let object_path = root
        .join(".git/objects")
        .join(&source_blob[..2])
        .join(&source_blob[2..]);
    fs::remove_file(&object_path).expect("remove promised source blob");

    assert!(matches!(
        observe(&executable, &root).expect_err("missing promised blob"),
        GitObservationError::CommandFailed {
            phase: GitCommandPhase::Status,
            ..
        }
    ));
    assert!(
        !marker.exists(),
        "remote upload-pack helper must not execute"
    );
    assert!(!object_path.exists(), "missing object must not be fetched");
}

#[test]
fn rejects_missing_changed_and_noncanonical_boundaries_before_execution() {
    let workspace = TestWorkspace::new("boundaries");
    let empty_path = std::env::join_paths([&workspace.directory]).expect("empty PATH");
    assert_eq!(
        inspect_git_on_path(Some(&empty_path)).expect_err("missing Git"),
        GitObservationError::GitNotFound
    );
    assert_eq!(
        inspect_git_on_path(Some(OsStr::new("relative-only"))).expect_err("relative PATH"),
        GitObservationError::GitNotFound
    );
    assert_eq!(
        inspect_noexec_runner_at("relative-runner").expect_err("relative runner"),
        GitObservationError::RunnerPathNotAbsolute
    );
    assert_eq!(
        inspect_noexec_runner_at(workspace.directory.join("missing-runner"))
            .expect_err("missing runner"),
        GitObservationError::RunnerUnavailable
    );

    let executable_path = workspace.directory.join("git");
    fs::write(&executable_path, b"#!/bin/sh\nexit 0\n").expect("fake Git");
    let mut permissions = fs::metadata(&executable_path)
        .expect("metadata")
        .permissions();
    permissions.set_mode(0o700);
    fs::set_permissions(&executable_path, permissions).expect("make executable");
    let path = std::env::join_paths([&workspace.directory]).expect("fake PATH");
    let executable = inspect_git_on_path(Some(&path)).expect("inspect fake Git");

    let mut changed_permissions = fs::metadata(&executable_path)
        .expect("metadata")
        .permissions();
    changed_permissions.set_mode(0o600);
    fs::set_permissions(&executable_path, changed_permissions).expect("change executable");
    assert_eq!(
        observe(&executable, &workspace.canonical_directory()).expect_err("changed Git"),
        GitObservationError::GitExecutableChanged
    );

    let installed = installed_git();
    let noncanonical = workspace.directory.join("alias");
    symlink(&workspace.directory, &noncanonical).expect("Project symlink");
    assert_eq!(
        observe(&installed, &noncanonical).expect_err("noncanonical Project"),
        GitObservationError::ProjectDirectoryNotCanonical
    );

    let copied_runner = workspace.directory.join("copied-runner");
    fs::copy(noexec_runner_path(), &copied_runner).expect("copy runner");
    let mut runner_permissions = fs::metadata(&copied_runner)
        .expect("runner metadata")
        .permissions();
    runner_permissions.set_mode(0o700);
    fs::set_permissions(&copied_runner, runner_permissions).expect("runner permissions");
    let changed_runner = inspect_noexec_runner_at(&copied_runner).expect("inspect copied runner");
    let mut changed_runner_permissions = fs::metadata(&copied_runner)
        .expect("runner metadata")
        .permissions();
    changed_runner_permissions.set_mode(0o600);
    fs::set_permissions(&copied_runner, changed_runner_permissions).expect("change runner");
    assert_eq!(
        observe_repository(
            &changed_runner,
            &installed,
            &workspace.canonical_directory()
        )
        .expect_err("changed runner"),
        GitObservationError::RunnerChanged
    );
}

#[test]
fn runner_sets_a_hard_process_limit_before_execing_the_selected_image() {
    let workspace = TestWorkspace::new("runner-limit");
    let root = workspace.canonical_directory();
    let marker = root.join("descendant-must-not-run");
    let fake_git = root.join("git");
    write_executable(
        &fake_git,
        format!(
            "#!/bin/sh\nulimit -u\nulimit -u 1\n/usr/bin/touch '{}'\nprintf '%s\\n' '{}'\n",
            marker.display(),
            root.display()
        )
        .as_bytes(),
    );
    let path = std::env::join_paths([&root]).expect("fake Git PATH");
    let executable = inspect_git_on_path(Some(&path)).expect("inspect fake Git");

    assert!(matches!(
        observe(&executable, &root).expect_err("descendant attempt"),
        GitObservationError::CommandFailed {
            phase: GitCommandPhase::RepositoryRoot,
            ..
        }
    ));
    assert!(!marker.exists(), "runner must prevent descendant creation");
}

#[test]
fn runner_boundary_failure_is_typed_without_retaining_diagnostics() {
    let workspace = TestWorkspace::new("runner-failure");
    let root = workspace.canonical_directory();
    let fake_runner = root.join("fake-runner");
    write_executable(
        &fake_runner,
        b"#!/bin/sh\nprintf 'flit-git-noexec:v1:limit-set-failed\\n' >&2\nexit 122\n",
    );
    let runner = inspect_noexec_runner_at(&fake_runner).expect("inspect fake runner");
    let executable = installed_git();

    assert_eq!(
        observe_repository(&runner, &executable, &root).expect_err("runner limit failure"),
        GitObservationError::RunnerBoundaryFailed {
            phase: GitCommandPhase::RepositoryRoot,
            failure: GitRunnerFailure::LimitSetFailed,
        }
    );
}

#[test]
fn bounded_mount_boundary_diagnostic_is_an_exact_non_repository_observation() {
    let workspace = TestWorkspace::new("mount-boundary");
    let root = workspace.canonical_directory();
    let fake_git = root.join("git");
    write_executable(
        &fake_git,
        b"#!/bin/sh\nprintf 'fatal: not a git repository (or any parent up to mount point /Volumes/External)\\nStopping at filesystem boundary (GIT_DISCOVERY_ACROSS_FILESYSTEM not set).\\n' >&2\nexit 128\n",
    );
    let path = std::env::join_paths([&root]).expect("fake Git PATH");
    let executable = inspect_git_on_path(Some(&path)).expect("inspect fake Git");

    assert_eq!(
        observe(&executable, &root).expect("mount-boundary observation"),
        GitObservation::NotWorktree(NotWorktreeReason::NotRepository)
    );
}

fn installed_git() -> flit_git::GitExecutable {
    inspect_git_on_path(std::env::var_os("PATH").as_deref()).expect("installed Git")
}

fn noexec_runner() -> flit_git::GitNoExecRunner {
    inspect_noexec_runner_at(noexec_runner_path()).expect("no-child-exec runner")
}

fn noexec_runner_path() -> &'static Path {
    Path::new(env!("CARGO_BIN_EXE_flit-git-noexec"))
}

fn observe(
    executable: &flit_git::GitExecutable,
    root: &Path,
) -> Result<GitObservation, GitObservationError> {
    observe_repository(&noexec_runner(), executable, root)
}

fn git(executable: &flit_git::GitExecutable, root: &Path, arguments: &[&str]) {
    let status = Command::new(executable.canonical_path())
        .env_clear()
        .env("LC_ALL", "C")
        .arg("--no-optional-locks")
        .arg("-C")
        .arg(root)
        .args(arguments)
        .status()
        .expect("run Git");
    assert!(status.success(), "Git command failed: {arguments:?}");
}

fn git_with_identity(executable: &flit_git::GitExecutable, root: &Path, arguments: &[&str]) {
    let status = Command::new(executable.canonical_path())
        .env_clear()
        .env("LC_ALL", "C")
        .arg("--no-optional-locks")
        .args([
            "-c",
            "user.name=Flit Test",
            "-c",
            "user.email=flit@example.invalid",
        ])
        .arg("-C")
        .arg(root)
        .args(arguments)
        .status()
        .expect("run Git with test identity");
    assert!(status.success(), "Git command failed: {arguments:?}");
}

fn git_stdout(executable: &flit_git::GitExecutable, root: &Path, arguments: &[&str]) -> String {
    let output = Command::new(executable.canonical_path())
        .env_clear()
        .env("LC_ALL", "C")
        .arg("--no-optional-locks")
        .arg("-C")
        .arg(root)
        .args(arguments)
        .output()
        .expect("run Git for stdout");
    assert!(output.status.success(), "Git command failed: {arguments:?}");
    String::from_utf8(output.stdout)
        .expect("UTF-8 Git stdout")
        .trim_end()
        .to_owned()
}

fn write_executable(path: &Path, content: &[u8]) {
    fs::write(path, content).expect("write executable");
    let mut permissions = fs::metadata(path).expect("metadata").permissions();
    permissions.set_mode(0o700);
    fs::set_permissions(path, permissions).expect("executable permissions");
}
