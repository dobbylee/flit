use std::{
    ffi::OsString,
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process,
    sync::atomic::{AtomicU64, Ordering},
};

use flit_providers::{
    ExecutableInspectionError, ExecutableSelectionSource, inspect_codex_at, inspect_codex_on_path,
};
use sha2::{Digest, Sha256};

static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(1);

struct TestWorkspace {
    directory: PathBuf,
}

impl TestWorkspace {
    fn new(label: &str) -> Self {
        let nonce = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let directory = std::env::temp_dir().join(format!(
            "flit-provider-executable-{label}-{}-{nonce}",
            process::id()
        ));
        fs::create_dir(&directory).expect("unique test directory");
        Self { directory }
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
fn path_inspection_ignores_relative_entries_and_selects_the_first_valid_canonical_file() {
    let workspace = TestWorkspace::new("path");
    let invalid_directory = workspace.directory.join("invalid");
    let first_directory = workspace.directory.join("first");
    let second_directory = workspace.directory.join("second");
    let target_directory = workspace.directory.join("target");
    for directory in [
        &invalid_directory,
        &first_directory,
        &second_directory,
        &target_directory,
    ] {
        fs::create_dir(directory).expect("PATH directory");
    }

    let marker = workspace.directory.join("must-not-exist");
    let first_target = target_directory.join("codex-real");
    let first_content = format!("#!/bin/sh\ntouch '{}'\n", marker.display());
    write_executable(&first_target, first_content.as_bytes());
    std::os::unix::fs::symlink(&first_target, first_directory.join("codex"))
        .expect("Codex symlink");
    fs::write(invalid_directory.join("codex"), b"not executable").expect("invalid Codex");
    write_executable(&second_directory.join("codex"), b"#!/bin/sh\nexit 0\n");
    let path_environment = std::env::join_paths([
        Path::new("relative-entry"),
        &invalid_directory,
        &first_directory,
        &second_directory,
    ])
    .expect("PATH value");

    let inspection = inspect_codex_on_path(Some(&path_environment)).expect("inspect PATH Codex");
    assert_eq!(
        inspection.source,
        ExecutableSelectionSource::PathEnvironment
    );
    assert_eq!(inspection.selected_path, first_directory.join("codex"));
    assert_eq!(
        inspection.canonical_path,
        fs::canonicalize(&first_target).expect("canonical target")
    );
    assert!(inspection.filesystem_id.starts_with("unix:"));
    assert_eq!(inspection.sha256, sha256(first_content.as_bytes()));
    assert!(
        !marker.exists(),
        "inspection must not execute provider code"
    );
}

#[test]
fn explicit_inspection_rejects_unsafe_or_invalid_paths_without_execution() {
    let workspace = TestWorkspace::new("invalid");
    assert!(matches!(
        inspect_codex_at("relative/codex"),
        Err(ExecutableInspectionError::ExplicitPathNotAbsolute { .. })
    ));
    assert!(matches!(
        inspect_codex_at(workspace.directory.join("missing")),
        Err(ExecutableInspectionError::Canonicalize { .. })
    ));
    assert!(matches!(
        inspect_codex_at(&workspace.directory),
        Err(ExecutableInspectionError::NotRegularFile { .. })
    ));
    let regular_file = workspace.directory.join("regular");
    fs::write(&regular_file, b"not executable").expect("regular file");
    assert!(matches!(
        inspect_codex_at(&regular_file),
        Err(ExecutableInspectionError::NotExecutable { .. })
    ));

    for path_environment in [None, Some(OsString::from("relative:"))] {
        assert!(matches!(
            inspect_codex_on_path(path_environment.as_deref()),
            Err(ExecutableInspectionError::NotFoundOnPath {
                searched_directories
            }) if searched_directories.is_empty()
        ));
    }
}

#[test]
fn effective_execute_access_skips_an_owner_ineligible_path_candidate() {
    let workspace = TestWorkspace::new("access");
    let ineligible_directory = workspace.directory.join("ineligible");
    let eligible_directory = workspace.directory.join("eligible");
    fs::create_dir(&ineligible_directory).expect("ineligible directory");
    fs::create_dir(&eligible_directory).expect("eligible directory");
    let ineligible = ineligible_directory.join("codex");
    fs::write(&ineligible, b"ineligible").expect("ineligible file");
    let mut permissions = fs::metadata(&ineligible).expect("metadata").permissions();
    permissions.set_mode(0o601);
    fs::set_permissions(&ineligible, permissions).expect("ineligible permissions");
    let eligible = eligible_directory.join("codex");
    write_executable(&eligible, b"eligible");
    let path_environment =
        std::env::join_paths([ineligible_directory, eligible_directory]).expect("PATH value");

    assert!(matches!(
        inspect_codex_at(&ineligible),
        Err(ExecutableInspectionError::NotExecutable { .. })
    ));
    assert_eq!(
        inspect_codex_on_path(Some(&path_environment))
            .expect("eligible PATH executable")
            .canonical_path,
        fs::canonicalize(eligible).expect("canonical eligible executable")
    );
}

#[test]
fn reinspection_of_an_in_place_mutation_preserves_file_identity_but_changes_digest() {
    let workspace = TestWorkspace::new("mutation");
    let executable = workspace.directory.join("codex");
    write_executable(&executable, b"first");
    let first = inspect_codex_at(&executable).expect("first inspection");

    fs::write(&executable, b"second").expect("mutate executable");
    let second = inspect_codex_at(&executable).expect("second inspection");
    assert_eq!(second.canonical_path, first.canonical_path);
    assert_eq!(second.filesystem_id, first.filesystem_id);
    assert_ne!(second.sha256, first.sha256);
}

fn write_executable(path: &Path, content: &[u8]) {
    fs::write(path, content).expect("write executable");
    let mut permissions = fs::metadata(path).expect("metadata").permissions();
    permissions.set_mode(0o700);
    fs::set_permissions(path, permissions).expect("executable permissions");
}

fn sha256(content: &[u8]) -> String {
    format!("{:x}", Sha256::digest(content))
}
