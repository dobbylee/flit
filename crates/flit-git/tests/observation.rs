use std::{
    ffi::OsStr,
    fs,
    os::unix::{fs::PermissionsExt, fs::symlink},
    path::{Path, PathBuf},
    process::{self, Command},
    sync::atomic::{AtomicU64, Ordering},
};

use flit_git::{
    DirtySummary, GitChangeObservationError, GitChangeSummary, GitCommandPhase, GitHead,
    GitObservation, GitObservationError, GitRunnerFailure, NotWorktreeReason, inspect_git_on_path,
    inspect_noexec_runner_at, observe_changes_since_clean_baseline, observe_clean_change_baseline,
    observe_repository,
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
fn observes_exact_tracked_changes_since_a_clean_baseline_without_paths() {
    let executable = installed_git();
    let workspace = TestWorkspace::new("changes");
    let root = workspace.canonical_directory();
    git(&executable, &root, &["init", "-b", "main"]);
    fs::write(root.join("modified.txt"), b"one\ntwo\n").expect("modified baseline file");
    fs::write(root.join("removed.txt"), b"removed\n").expect("removed baseline file");
    git(&executable, &root, &["add", "--", "."]);
    git_with_identity(&executable, &root, &["commit", "-m", "baseline"]);
    let baseline = clean_baseline(&executable, &root);
    assert_eq!(
        observe_changes_since_clean_baseline(&noexec_runner(), &executable, &root, &baseline,)
            .expect("exact unchanged terminal state"),
        GitChangeSummary::default()
    );

    fs::write(root.join("modified.txt"), b"one\nthree\n").expect("modified terminal file");
    fs::remove_file(root.join("removed.txt")).expect("removed terminal file");
    fs::write(root.join("added.txt"), b"added\nlines\n").expect("added terminal file");
    git(&executable, &root, &["add", "-A"]);

    assert_eq!(
        baseline.observe_changes().expect("exact terminal changes"),
        GitChangeSummary {
            files: 3,
            insertions: 3,
            deletions: 2,
        }
    );
}

#[test]
fn rejects_unborn_and_dirty_baselines() {
    let executable = installed_git();
    let workspace = TestWorkspace::new("baseline-contract");
    let root = workspace.canonical_directory();
    assert_eq!(
        observe_clean_change_baseline(&noexec_runner(), &executable, &root)
            .expect_err("non-worktree baseline"),
        GitChangeObservationError::BaselineNotWorktree(NotWorktreeReason::NotRepository)
    );
    git(&executable, &root, &["init", "-b", "main"]);
    assert_eq!(
        observe_clean_change_baseline(&noexec_runner(), &executable, &root)
            .expect_err("unborn baseline"),
        GitChangeObservationError::BaselineHeadUnavailable
    );

    fs::write(root.join("tracked.txt"), b"baseline\n").expect("tracked file");
    git(&executable, &root, &["add", "--", "."]);
    git_with_identity(&executable, &root, &["commit", "-m", "baseline"]);
    fs::write(root.join("tracked.txt"), b"dirty\n").expect("dirty tracked file");
    assert_eq!(
        observe_clean_change_baseline(&noexec_runner(), &executable, &root)
            .expect_err("dirty baseline"),
        GitChangeObservationError::BaselineNotClean
    );
}

#[test]
fn rejects_untracked_and_binary_terminal_states_without_zero_fallback() {
    let executable = installed_git();
    let workspace = TestWorkspace::new("terminal-unavailable");
    let root = workspace.canonical_directory();
    git(&executable, &root, &["init", "-b", "main"]);
    fs::write(root.join(".gitattributes"), b"binary.bin binary\n").expect("attributes");
    fs::write(root.join("binary.bin"), b"baseline\0bytes").expect("binary baseline");
    git(&executable, &root, &["add", "--", "."]);
    git_with_identity(&executable, &root, &["commit", "-m", "baseline"]);
    let baseline = clean_baseline(&executable, &root);

    fs::write(root.join("untracked.txt"), b"untracked\n").expect("untracked file");
    assert_eq!(
        observe_changes_since_clean_baseline(&noexec_runner(), &executable, &root, &baseline,)
            .expect_err("untracked terminal state"),
        GitChangeObservationError::TerminalUntracked
    );
    fs::remove_file(root.join("untracked.txt")).expect("remove untracked file");

    fs::write(root.join("binary.bin"), b"changed\0bytes").expect("binary terminal file");
    assert_eq!(
        observe_changes_since_clean_baseline(&noexec_runner(), &executable, &root, &baseline,)
            .expect_err("binary terminal diff"),
        GitChangeObservationError::BinaryNumstat
    );
}

#[test]
fn rejects_staged_content_cancelled_by_the_terminal_worktree() {
    let executable = installed_git();
    let workspace = TestWorkspace::new("mixed-index-worktree");
    let root = workspace.canonical_directory();
    git(&executable, &root, &["init", "-b", "main"]);
    fs::write(root.join("tracked.txt"), b"baseline\n").expect("baseline file");
    git(&executable, &root, &["add", "--", "."]);
    git_with_identity(&executable, &root, &["commit", "-m", "baseline"]);
    let baseline = clean_baseline(&executable, &root);

    fs::write(root.join("tracked.txt"), b"staged\n").expect("staged content");
    git(&executable, &root, &["add", "--", "tracked.txt"]);
    fs::write(root.join("tracked.txt"), b"baseline\n").expect("cancelled worktree content");

    assert_eq!(
        observe_changes_since_clean_baseline(&noexec_runner(), &executable, &root, &baseline,)
            .expect_err("mixed index and worktree changes"),
        GitChangeObservationError::MixedIndexWorktreeChangesUnsupported
    );
}

#[test]
fn rejects_index_flags_that_hide_tracked_worktree_changes() {
    let executable = installed_git();
    let workspace = TestWorkspace::new("hidden-index-flags");
    let root = workspace.canonical_directory();
    git(&executable, &root, &["init", "-b", "main"]);
    fs::write(root.join("tracked.txt"), b"baseline\n").expect("baseline file");
    git(&executable, &root, &["add", "--", "."]);
    git_with_identity(&executable, &root, &["commit", "-m", "baseline"]);
    let baseline = clean_baseline(&executable, &root);

    git(
        &executable,
        &root,
        &["update-index", "--assume-unchanged", "tracked.txt"],
    );
    fs::write(root.join("tracked.txt"), b"hidden assume-unchanged\n")
        .expect("assume-unchanged content");
    assert_eq!(
        observe_changes_since_clean_baseline(&noexec_runner(), &executable, &root, &baseline,)
            .expect_err("assume-unchanged flag"),
        GitChangeObservationError::IndexFlagsUnsupportedForChanges
    );

    fs::write(root.join("tracked.txt"), b"baseline\n").expect("restore baseline content");
    git(
        &executable,
        &root,
        &["update-index", "--no-assume-unchanged", "tracked.txt"],
    );
    git(
        &executable,
        &root,
        &["update-index", "--skip-worktree", "tracked.txt"],
    );
    fs::write(root.join("tracked.txt"), b"hidden skip-worktree\n").expect("skip-worktree content");
    assert_eq!(
        observe_changes_since_clean_baseline(&noexec_runner(), &executable, &root, &baseline,)
            .expect_err("skip-worktree flag"),
        GitChangeObservationError::IndexFlagsUnsupportedForChanges
    );
}

#[test]
fn ignores_replacement_refs_when_resolving_the_captured_baseline() {
    let executable = installed_git();
    let workspace = TestWorkspace::new("replacement-refs");
    let root = workspace.canonical_directory();
    git(&executable, &root, &["init", "-b", "main"]);
    fs::write(root.join("tracked.txt"), b"baseline\n").expect("baseline file");
    git(&executable, &root, &["add", "--", "."]);
    git_with_identity(&executable, &root, &["commit", "-m", "baseline"]);
    let baseline_oid = git_stdout(&executable, &root, &["rev-parse", "HEAD"]);
    let baseline = clean_baseline(&executable, &root);

    fs::write(root.join("tracked.txt"), b"terminal\n").expect("terminal file");
    git(&executable, &root, &["add", "--", "."]);
    git_with_identity(&executable, &root, &["commit", "-m", "terminal"]);
    let terminal_oid = git_stdout(&executable, &root, &["rev-parse", "HEAD"]);
    git(
        &executable,
        &root,
        &["replace", &baseline_oid, &terminal_oid],
    );

    assert_eq!(
        observe_changes_since_clean_baseline(&noexec_runner(), &executable, &root, &baseline,)
            .expect("replacement-independent aggregate"),
        GitChangeSummary {
            files: 1,
            insertions: 1,
            deletions: 1,
        }
    );
}

#[test]
fn overrides_weakened_stat_config_for_same_length_tracked_rewrites() {
    let executable = installed_git();
    let repository = TestWorkspace::new("stat-config-repository");
    let timestamp_workspace = TestWorkspace::new("stat-config-timestamp");
    let root = repository.canonical_directory();
    let timestamp_root = timestamp_workspace.canonical_directory();
    let tracked = root.join("tracked.txt");
    let reference = timestamp_root.join("reference");
    git(&executable, &root, &["init", "-b", "main"]);
    fs::write(&tracked, b"aaaa\n").expect("baseline file");
    git(&executable, &root, &["add", "--", "."]);
    git_with_identity(&executable, &root, &["commit", "-m", "baseline"]);
    let baseline = clean_baseline(&executable, &root);
    fs::write(&reference, b"timestamp reference\n").expect("timestamp reference");
    let status = Command::new("/usr/bin/touch")
        .args(["-r"])
        .arg(&tracked)
        .arg(&reference)
        .status()
        .expect("copy baseline timestamp");
    assert!(status.success(), "copy baseline timestamp failed");
    git(&executable, &root, &["config", "core.trustctime", "false"]);
    git(&executable, &root, &["config", "core.checkStat", "minimal"]);

    fs::write(&tracked, b"bbbb\n").expect("same-length terminal rewrite");
    let status = Command::new("/usr/bin/touch")
        .args(["-r"])
        .arg(&reference)
        .arg(&tracked)
        .status()
        .expect("restore baseline mtime");
    assert!(status.success(), "restore baseline mtime failed");

    assert_eq!(
        observe_changes_since_clean_baseline(&noexec_runner(), &executable, &root, &baseline,)
            .expect("stat-independent aggregate"),
        GitChangeSummary {
            files: 1,
            insertions: 1,
            deletions: 1,
        }
    );
}

#[test]
fn overrides_disabled_filemode_for_executable_bit_changes() {
    let executable = installed_git();
    let workspace = TestWorkspace::new("filemode-config");
    let root = workspace.canonical_directory();
    let tracked = root.join("tracked.sh");
    git(&executable, &root, &["init", "-b", "main"]);
    fs::write(&tracked, b"#!/bin/sh\n").expect("baseline file");
    git(&executable, &root, &["add", "--", "."]);
    git_with_identity(&executable, &root, &["commit", "-m", "baseline"]);
    let baseline = clean_baseline(&executable, &root);
    git(&executable, &root, &["config", "core.filemode", "false"]);

    let mut permissions = fs::metadata(&tracked)
        .expect("tracked metadata")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&tracked, permissions).expect("make tracked file executable");

    assert_eq!(
        observe_changes_since_clean_baseline(&noexec_runner(), &executable, &root, &baseline,)
            .expect("filemode-independent aggregate"),
        GitChangeSummary {
            files: 1,
            insertions: 0,
            deletions: 0,
        }
    );
}

#[test]
fn rejects_missing_baseline_object_without_zero_fallback() {
    let executable = installed_git();
    let workspace = TestWorkspace::new("missing-baseline-object");
    let root = workspace.canonical_directory();
    git(&executable, &root, &["init", "-b", "main"]);
    fs::write(root.join("tracked.txt"), b"baseline\n").expect("baseline file");
    git(&executable, &root, &["add", "--", "."]);
    git_with_identity(&executable, &root, &["commit", "-m", "baseline"]);
    let baseline = clean_baseline(&executable, &root);
    let receipt = baseline.receipt();
    let GitHead::Available(baseline_oid) = &receipt.head else {
        panic!("expected an available baseline HEAD");
    };
    let baseline_object = root
        .join(".git/objects")
        .join(&baseline_oid[..2])
        .join(&baseline_oid[2..]);
    fs::write(root.join("tracked.txt"), b"terminal\n").expect("terminal file");
    git(&executable, &root, &["add", "--", "."]);
    git_with_identity(&executable, &root, &["commit", "-m", "terminal"]);
    fs::remove_file(&baseline_object).expect("remove baseline commit object");

    assert!(matches!(
        observe_changes_since_clean_baseline(&noexec_runner(), &executable, &root, &baseline,)
            .expect_err("missing baseline object"),
        GitChangeObservationError::Observation(GitObservationError::CommandFailed {
            phase: GitCommandPhase::ChangeSummary,
            ..
        })
    ));
}

#[test]
fn rejects_replaced_git_directory_even_when_the_baseline_object_is_shared() {
    let executable = installed_git();
    let repository = TestWorkspace::new("repository-identity");
    let alternate_workspace = TestWorkspace::new("alternate-repository");
    let root = repository.canonical_directory();
    let alternate_root = alternate_workspace.canonical_directory();
    git(&executable, &root, &["init", "-b", "main"]);
    fs::write(root.join("tracked.txt"), b"baseline\n").expect("baseline file");
    git(&executable, &root, &["add", "--", "."]);
    git_with_identity(&executable, &root, &["commit", "-m", "baseline"]);
    let baseline = clean_baseline(&executable, &root);

    git(
        &executable,
        &alternate_root,
        &[
            "clone",
            "--no-hardlinks",
            root.to_str().expect("UTF-8 source repository"),
            "replacement",
        ],
    );
    let replacement_git = alternate_root.join("replacement/.git");
    fs::rename(root.join(".git"), alternate_root.join("original-git"))
        .expect("preserve original Git directory");
    fs::write(
        root.join(".git"),
        format!("gitdir: {}\n", replacement_git.display()),
    )
    .expect("redirect Git directory");

    assert_eq!(
        observe_changes_since_clean_baseline(&noexec_runner(), &executable, &root, &baseline,)
            .expect_err("replaced repository identity"),
        GitChangeObservationError::RepositoryIdentityChanged
    );
}

#[test]
fn rejects_replaced_worktree_with_the_same_external_git_directory() {
    let executable = installed_git();
    let workspace = TestWorkspace::new("worktree-identity");
    let parent = workspace.canonical_directory();
    let project = parent.join("project");
    let external_git = parent.join("external.git");
    git(
        &executable,
        &parent,
        &[
            "init",
            "-b",
            "main",
            "--separate-git-dir",
            external_git.to_str().expect("UTF-8 external Git directory"),
            "project",
        ],
    );
    let root = fs::canonicalize(&project).expect("canonical Project");
    fs::write(root.join("tracked.txt"), b"baseline\n").expect("baseline file");
    git(&executable, &root, &["add", "--", "."]);
    git_with_identity(&executable, &root, &["commit", "-m", "baseline"]);
    let baseline = clean_baseline(&executable, &root);

    fs::rename(&root, parent.join("original-project")).expect("preserve original worktree");
    fs::create_dir(&root).expect("replacement worktree root");
    fs::write(
        root.join(".git"),
        format!("gitdir: {}\n", external_git.display()),
    )
    .expect("reuse external Git directory");
    fs::write(root.join("tracked.txt"), b"baseline\n").expect("replacement tracked file");

    assert_eq!(
        observe_changes_since_clean_baseline(&noexec_runner(), &executable, &root, &baseline,)
            .expect_err("replaced worktree identity"),
        GitChangeObservationError::BaselineProjectIdentityChanged
    );
}

#[test]
fn rejects_replaced_repository_root_even_when_the_nested_project_is_preserved() {
    let executable = installed_git();
    let workspace = TestWorkspace::new("repository-root-identity");
    let parent = workspace.canonical_directory();
    let repository = parent.join("repository");
    let external_git = parent.join("external.git");
    git(
        &executable,
        &parent,
        &[
            "init",
            "-b",
            "main",
            "--separate-git-dir",
            external_git.to_str().expect("UTF-8 external Git directory"),
            "repository",
        ],
    );
    let root = fs::canonicalize(&repository).expect("canonical repository root");
    fs::create_dir(root.join("nested")).expect("nested Project");
    fs::write(root.join("nested/tracked.txt"), b"baseline\n").expect("baseline file");
    git(&executable, &root, &["add", "--", "."]);
    git_with_identity(&executable, &root, &["commit", "-m", "baseline"]);
    let project = fs::canonicalize(root.join("nested")).expect("canonical nested Project");
    let baseline = clean_baseline(&executable, &project);

    let original_root = parent.join("original-repository");
    fs::rename(&root, &original_root).expect("preserve original repository root");
    fs::create_dir(&root).expect("replacement repository root");
    fs::rename(original_root.join("nested"), root.join("nested"))
        .expect("preserve nested Project identity");
    fs::write(
        root.join(".git"),
        format!("gitdir: {}\n", external_git.display()),
    )
    .expect("reuse external Git directory");

    assert_eq!(
        observe_changes_since_clean_baseline(&noexec_runner(), &executable, &project, &baseline,)
            .expect_err("replaced repository root identity"),
        GitChangeObservationError::BaselineRepositoryRootIdentityChanged
    );
}

#[test]
fn rejects_exact_counts_when_a_superproject_gitlink_advances() {
    let executable = installed_git();
    let source_workspace = TestWorkspace::new("submodule-source");
    let super_workspace = TestWorkspace::new("submodule-superproject");
    let source = source_workspace.canonical_directory();
    let root = super_workspace.canonical_directory();
    git(&executable, &source, &["init", "-b", "main"]);
    fs::write(source.join("source.txt"), b"a\n").expect("source A");
    git(&executable, &source, &["add", "--", "."]);
    git_with_identity(&executable, &source, &["commit", "-m", "source A"]);
    let source_a = git_stdout(&executable, &source, &["rev-parse", "HEAD"]);
    fs::write(source.join("source.txt"), b"b\n").expect("source B");
    git(&executable, &source, &["add", "--", "."]);
    git_with_identity(&executable, &source, &["commit", "-m", "source B"]);
    let source_b = git_stdout(&executable, &source, &["rev-parse", "HEAD"]);

    git(&executable, &root, &["init", "-b", "main"]);
    fs::write(
        root.join(".gitmodules"),
        format!(
            "[submodule \"module\"]\n\tpath = module\n\turl = {}\n",
            source.display()
        ),
    )
    .expect("submodule metadata");
    git(&executable, &root, &["add", "--", ".gitmodules"]);
    git(
        &executable,
        &root,
        &[
            "update-index",
            "--add",
            "--cacheinfo",
            "160000",
            &source_a,
            "module",
        ],
    );
    git_with_identity(&executable, &root, &["commit", "-m", "baseline gitlink"]);
    let baseline = clean_baseline(&executable, &root);

    git(
        &executable,
        &root,
        &["update-index", "--cacheinfo", "160000", &source_b, "module"],
    );
    git_with_identity(&executable, &root, &["commit", "-m", "terminal gitlink"]);

    assert_eq!(
        observe_changes_since_clean_baseline(&noexec_runner(), &executable, &root, &baseline,)
            .expect_err("submodule gitlink change"),
        GitChangeObservationError::SubmodulesUnsupportedForChanges
    );
}

#[test]
fn rejects_runner_and_git_executable_mismatches_from_the_bound_baseline() {
    let executable = installed_git();
    let repository = TestWorkspace::new("baseline-tools-repository");
    let tool_workspace = TestWorkspace::new("baseline-tools-alternate");
    let root = repository.canonical_directory();
    let tool_root = tool_workspace.canonical_directory();
    git(&executable, &root, &["init", "-b", "main"]);
    fs::write(root.join("tracked.txt"), b"baseline\n").expect("baseline file");
    git(&executable, &root, &["add", "--", "."]);
    git_with_identity(&executable, &root, &["commit", "-m", "baseline"]);
    let baseline = clean_baseline(&executable, &root);

    let copied_runner = tool_root.join("copied-runner");
    fs::copy(noexec_runner_path(), &copied_runner).expect("copy runner");
    let mut runner_permissions = fs::metadata(&copied_runner)
        .expect("runner metadata")
        .permissions();
    runner_permissions.set_mode(0o700);
    fs::set_permissions(&copied_runner, runner_permissions).expect("runner permissions");
    let alternate_runner = inspect_noexec_runner_at(&copied_runner).expect("alternate runner");
    assert_eq!(
        observe_changes_since_clean_baseline(&alternate_runner, &executable, &root, &baseline,)
            .expect_err("runner mismatch"),
        GitChangeObservationError::BaselineRunnerMismatch
    );

    let fake_git = tool_root.join("git");
    write_executable(&fake_git, b"#!/bin/sh\nexit 99\n");
    let path = std::env::join_paths([&tool_root]).expect("alternate Git PATH");
    let alternate_git = inspect_git_on_path(Some(&path)).expect("alternate Git");
    assert_eq!(
        observe_changes_since_clean_baseline(&noexec_runner(), &alternate_git, &root, &baseline,)
            .expect_err("Git executable mismatch"),
        GitChangeObservationError::BaselineGitExecutableMismatch
    );
}

#[test]
fn rejects_a_terminal_project_that_is_no_longer_a_worktree() {
    let executable = installed_git();
    let workspace = TestWorkspace::new("terminal-non-worktree");
    let root = workspace.canonical_directory();
    git(&executable, &root, &["init", "-b", "main"]);
    fs::write(root.join("tracked.txt"), b"baseline\n").expect("tracked file");
    git(&executable, &root, &["add", "--", "."]);
    git_with_identity(&executable, &root, &["commit", "-m", "baseline"]);
    let baseline = clean_baseline(&executable, &root);
    fs::remove_dir_all(root.join(".git")).expect("remove temporary repository metadata");

    assert_eq!(
        observe_changes_since_clean_baseline(&noexec_runner(), &executable, &root, &baseline,)
            .expect_err("terminal non-worktree"),
        GitChangeObservationError::TerminalNotWorktree(NotWorktreeReason::NotRepository)
    );
}

#[test]
fn disables_configured_diff_helpers_for_terminal_aggregation() {
    let executable = installed_git();
    let repository = TestWorkspace::new("diff-helper-repository");
    let helper_workspace = TestWorkspace::new("diff-helper-tool");
    let root = repository.canonical_directory();
    let helper_root = helper_workspace.canonical_directory();
    let marker = helper_root.join("helper-must-not-run");
    let helper = helper_root.join("diff-helper");
    write_executable(
        &helper,
        format!(
            "#!/bin/sh\n/usr/bin/touch '{}'\n/bin/cat \"$1\"\n",
            marker.display()
        )
        .as_bytes(),
    );
    git(&executable, &root, &["init", "-b", "main"]);
    fs::write(
        root.join(".gitattributes"),
        b"tracked.txt diff=sideeffect\n",
    )
    .expect("attributes");
    fs::write(root.join("tracked.txt"), b"baseline\n").expect("tracked file");
    git(&executable, &root, &["add", "--", "."]);
    git_with_identity(&executable, &root, &["commit", "-m", "baseline"]);
    let baseline = clean_baseline(&executable, &root);
    let helper_path = helper.to_str().expect("UTF-8 helper path");
    git(
        &executable,
        &root,
        &["config", "diff.sideeffect.command", helper_path],
    );
    git(
        &executable,
        &root,
        &["config", "diff.sideeffect.textconv", helper_path],
    );
    fs::write(root.join("tracked.txt"), b"changed\n").expect("terminal file");

    assert_eq!(
        observe_changes_since_clean_baseline(&noexec_runner(), &executable, &root, &baseline,)
            .expect("helper-free aggregate"),
        GitChangeSummary {
            files: 1,
            insertions: 1,
            deletions: 1,
        }
    );
    assert!(!marker.exists(), "configured diff helper must not execute");
}

#[test]
fn missing_terminal_blob_fails_without_lazy_fetch_or_remote_helper_execution() {
    let executable = installed_git();
    let repository = TestWorkspace::new("terminal-promisor-repository");
    let helper_workspace = TestWorkspace::new("terminal-promisor-helper");
    let root = repository.canonical_directory();
    let helper_root = helper_workspace.canonical_directory();
    let marker = helper_root.join("upload-pack-must-not-run");
    let upload_pack = helper_root.join("marker-upload-pack");
    git(&executable, &root, &["init", "-b", "main"]);
    fs::write(root.join("tracked.txt"), b"baseline content\n").expect("baseline file");
    git(&executable, &root, &["add", "--", "."]);
    git_with_identity(&executable, &root, &["commit", "-m", "baseline"]);
    let baseline = clean_baseline(&executable, &root);
    let source_blob = git_stdout(&executable, &root, &["rev-parse", "HEAD:tracked.txt"]);
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
    fs::remove_file(root.join("tracked.txt")).expect("remove worktree file");
    fs::remove_file(&object_path).expect("remove promised source blob");

    assert!(matches!(
        observe_changes_since_clean_baseline(&noexec_runner(), &executable, &root, &baseline,)
            .expect_err("missing terminal blob"),
        GitChangeObservationError::Observation(GitObservationError::CommandFailed {
            phase: GitCommandPhase::ChangeSummary,
            ..
        })
    ));
    assert!(
        !marker.exists(),
        "remote upload-pack helper must not execute"
    );
    assert!(!object_path.exists(), "missing object must not be fetched");
}

#[test]
fn rejects_a_terminal_receipt_that_changes_between_bounded_diff_reads() {
    const OID: &str = "0123456789abcdef0123456789abcdef01234567";
    let repository = TestWorkspace::new("unstable-repository");
    let tool_workspace = TestWorkspace::new("unstable-tool");
    let root = repository.canonical_directory();
    let tool_root = tool_workspace.canonical_directory();
    let fake_git = tool_root.join("git");
    let changed = tool_root.join("changed");
    write_executable(
        &fake_git,
        format!(
            "#!/bin/sh\ncase \" $* \" in\n  *\" --git-dir \"*) printf '%s\\n%s\\n' '{}' '{}' ;;\n  *\" rev-parse \"*) printf '%s\\n' '{}' ;;\n  *\" status \"*) printf '# branch.oid {OID}\\0# branch.head main\\0' ;;\n  *\" ls-files \"*) exit 0 ;;\n  *\" diff \"*) if [ -e '{}' ]; then printf '2\\t0\\tpath\\0'; else : > '{}'; printf '1\\t0\\tpath\\0'; fi ;;\n  *) exit 2 ;;\nesac\n",
            root.display(),
            root.display(),
            root.display(),
            changed.display(),
            changed.display(),
        )
        .as_bytes(),
    );
    let path = std::env::join_paths([&tool_root]).expect("fake Git PATH");
    let executable = inspect_git_on_path(Some(&path)).expect("inspect fake Git");
    let baseline = clean_baseline(&executable, &root);

    assert_eq!(
        observe_changes_since_clean_baseline(&noexec_runner(), &executable, &root, &baseline,)
            .expect_err("unstable diff"),
        GitChangeObservationError::RepositoryChangedDuringObservation
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

fn clean_baseline(
    executable: &flit_git::GitExecutable,
    root: &Path,
) -> flit_git::GitChangeBaseline {
    observe_clean_change_baseline(&noexec_runner(), executable, root)
        .expect("observe clean Git change baseline")
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
