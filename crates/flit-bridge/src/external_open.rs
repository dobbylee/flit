use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use flit_store::{
    ManagedGitFileStatus, ManagedGitProjectScope, ManagedGitRepositoryIdentity,
    ProjectDirectoryInspection,
};

#[cfg(unix)]
use std::{
    ffi::OsString,
    os::unix::{ffi::OsStringExt, fs::MetadataExt},
};

#[derive(Clone, Eq, PartialEq)]
pub(crate) struct ExternalOpenAuthority {
    pub project_path: PathBuf,
    pub project_filesystem_id: String,
    pub repository_identity: ManagedGitRepositoryIdentity,
    pub raw_path: Vec<u8>,
    pub status: ManagedGitFileStatus,
    pub project_scope: ManagedGitProjectScope,
}

impl std::fmt::Debug for ExternalOpenAuthority {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ExternalOpenAuthority")
            .field("project_path", &"<redacted>")
            .field("project_filesystem_id", &self.project_filesystem_id)
            .field("repository_identity", &self.repository_identity)
            .field("raw_path", &"<redacted>")
            .field("status", &self.status)
            .field("project_scope", &self.project_scope)
            .finish()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ExternalOpenGuardError {
    DeletedChange,
    OutsideProject,
    ProjectIdentityMismatch,
    RepositoryIdentityMismatch,
    TargetUnavailable,
    SymlinkEscape,
    TargetNotFile,
}

#[derive(Clone, Eq, PartialEq)]
pub(crate) struct ExternalOpenTarget {
    canonical_path: PathBuf,
    filesystem_id: String,
}

impl std::fmt::Debug for ExternalOpenTarget {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ExternalOpenTarget")
            .field("canonical_path", &"<redacted>")
            .field("filesystem_id", &self.filesystem_id)
            .finish()
    }
}

impl ExternalOpenTarget {
    pub(crate) fn canonical_path(&self) -> &Path {
        &self.canonical_path
    }
}

pub(crate) fn inspect_external_open_target(
    authority: &ExternalOpenAuthority,
) -> Result<ExternalOpenTarget, ExternalOpenGuardError> {
    if authority.status == ManagedGitFileStatus::Deleted {
        return Err(ExternalOpenGuardError::DeletedChange);
    }
    if authority.project_scope != ManagedGitProjectScope::InsideProject {
        return Err(ExternalOpenGuardError::OutsideProject);
    }

    let project = ProjectDirectoryInspection::inspect(&authority.project_path)
        .map_err(|_| ExternalOpenGuardError::ProjectIdentityMismatch)?;
    if project.selected_via_symlink
        || project.identity.canonical_path != authority.project_path
        || project.identity.filesystem_id != authority.project_filesystem_id
        || project.identity.filesystem_id != authority.repository_identity.project_filesystem_id
    {
        return Err(ExternalOpenGuardError::ProjectIdentityMismatch);
    }

    let repository_root = verified_repository_directory(
        &authority.repository_identity.repository_root,
        &authority.repository_identity.repository_root_filesystem_id,
    )?;
    verified_repository_directory(
        &authority.repository_identity.git_directory,
        &authority.repository_identity.git_directory_filesystem_id,
    )?;
    verified_repository_directory(
        &authority.repository_identity.common_directory,
        &authority.repository_identity.common_directory_filesystem_id,
    )?;

    let target = repository_root.join(path_from_bytes(&authority.raw_path)?);
    if !target.starts_with(&repository_root) {
        return Err(ExternalOpenGuardError::SymlinkEscape);
    }
    fs::symlink_metadata(&target).map_err(|_| ExternalOpenGuardError::TargetUnavailable)?;
    let canonical_path =
        fs::canonicalize(&target).map_err(|_| ExternalOpenGuardError::TargetUnavailable)?;
    if !canonical_path.starts_with(&repository_root)
        || !canonical_path.starts_with(&authority.project_path)
    {
        return Err(ExternalOpenGuardError::SymlinkEscape);
    }
    let metadata =
        fs::metadata(&canonical_path).map_err(|_| ExternalOpenGuardError::TargetUnavailable)?;
    if !metadata.is_file() {
        return Err(ExternalOpenGuardError::TargetNotFile);
    }
    Ok(ExternalOpenTarget {
        canonical_path,
        filesystem_id: filesystem_id(&metadata).ok_or(ExternalOpenGuardError::TargetUnavailable)?,
    })
}

fn verified_repository_directory(
    raw_path: &[u8],
    expected_filesystem_id: &str,
) -> Result<PathBuf, ExternalOpenGuardError> {
    let path = path_from_bytes(raw_path)?;
    let canonical =
        fs::canonicalize(&path).map_err(|_| ExternalOpenGuardError::RepositoryIdentityMismatch)?;
    if canonical != path {
        return Err(ExternalOpenGuardError::RepositoryIdentityMismatch);
    }
    let metadata =
        fs::metadata(&canonical).map_err(|_| ExternalOpenGuardError::RepositoryIdentityMismatch)?;
    if !metadata.is_dir() || filesystem_id(&metadata).as_deref() != Some(expected_filesystem_id) {
        return Err(ExternalOpenGuardError::RepositoryIdentityMismatch);
    }
    Ok(canonical)
}

#[cfg(unix)]
fn path_from_bytes(raw_path: &[u8]) -> Result<PathBuf, ExternalOpenGuardError> {
    Ok(PathBuf::from(OsString::from_vec(raw_path.to_vec())))
}

#[cfg(not(unix))]
fn path_from_bytes(_raw_path: &[u8]) -> Result<PathBuf, ExternalOpenGuardError> {
    Err(ExternalOpenGuardError::RepositoryIdentityMismatch)
}

#[cfg(unix)]
fn filesystem_id(metadata: &fs::Metadata) -> Option<String> {
    Some(format!("unix:{}:{}", metadata.dev(), metadata.ino()))
}

#[cfg(not(unix))]
fn filesystem_id(_metadata: &fs::Metadata) -> Option<String> {
    None
}

pub(crate) fn open_with_default_application(path: &Path) -> Result<(), ()> {
    let status = Command::new("/usr/bin/open")
        .arg(path)
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|_| ())?;
    status.success().then_some(()).ok_or(())
}

#[cfg(all(test, unix))]
mod tests {
    use std::{
        os::unix::{ffi::OsStrExt, fs::symlink},
        process,
        sync::atomic::{AtomicU64, Ordering},
    };

    use super::*;

    static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(1);

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "flit-external-open-{}-{}",
                process::id(),
                NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).expect("external-open test directory");
            Self(path)
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn path_bytes(path: &Path) -> Vec<u8> {
        path.as_os_str().as_bytes().to_vec()
    }

    fn directory_id(path: &Path) -> String {
        let metadata = fs::metadata(path).expect("directory metadata");
        format!("unix:{}:{}", metadata.dev(), metadata.ino())
    }

    fn authority(project: &Path, raw_path: &[u8]) -> ExternalOpenAuthority {
        let git_directory = project.join(".git");
        fs::create_dir_all(&git_directory).expect("Git directory");
        let project_id = directory_id(project);
        let git_id = directory_id(&git_directory);
        ExternalOpenAuthority {
            project_path: project.to_owned(),
            project_filesystem_id: project_id.clone(),
            repository_identity: ManagedGitRepositoryIdentity {
                project_filesystem_id: project_id.clone(),
                repository_root: path_bytes(project),
                repository_root_filesystem_id: project_id,
                git_directory: path_bytes(&git_directory),
                git_directory_filesystem_id: git_id.clone(),
                common_directory: path_bytes(&git_directory),
                common_directory_filesystem_id: git_id,
            },
            raw_path: raw_path.to_vec(),
            status: ManagedGitFileStatus::Modified,
            project_scope: ManagedGitProjectScope::InsideProject,
        }
    }

    #[test]
    fn guard_rejects_non_openable_targets_without_weakening_path_authority() {
        let directory = TestDirectory::new();
        let project = directory.0.join("project");
        fs::create_dir(&project).expect("Project directory");
        let project = fs::canonicalize(project).expect("canonical Project");
        fs::write(project.join("inside.txt"), b"inside").expect("inside file");
        let valid = authority(&project, b"inside.txt");
        let target = inspect_external_open_target(&valid).expect("valid target");
        assert_eq!(
            target.canonical_path(),
            fs::canonicalize(project.join("inside.txt")).expect("canonical target")
        );

        let mut deleted = valid.clone();
        deleted.status = ManagedGitFileStatus::Deleted;
        assert_eq!(
            inspect_external_open_target(&deleted),
            Err(ExternalOpenGuardError::DeletedChange)
        );
        let mut outside = valid.clone();
        outside.project_scope = ManagedGitProjectScope::OutsideProject;
        assert_eq!(
            inspect_external_open_target(&outside),
            Err(ExternalOpenGuardError::OutsideProject)
        );
        let mut project_drift = valid.clone();
        project_drift.project_filesystem_id = "unix:1:1".to_owned();
        assert_eq!(
            inspect_external_open_target(&project_drift),
            Err(ExternalOpenGuardError::ProjectIdentityMismatch)
        );
        let mut repository_drift = valid.clone();
        repository_drift
            .repository_identity
            .repository_root_filesystem_id = "unix:1:1".to_owned();
        assert_eq!(
            inspect_external_open_target(&repository_drift),
            Err(ExternalOpenGuardError::RepositoryIdentityMismatch)
        );

        let missing = authority(&project, b"missing.txt");
        assert_eq!(
            inspect_external_open_target(&missing),
            Err(ExternalOpenGuardError::TargetUnavailable)
        );
        fs::create_dir(project.join("directory-target")).expect("directory target");
        let non_file = authority(&project, b"directory-target");
        assert_eq!(
            inspect_external_open_target(&non_file),
            Err(ExternalOpenGuardError::TargetNotFile)
        );

        let outside_root = directory.0.join("outside.txt");
        fs::write(&outside_root, b"outside").expect("outside file");
        symlink(&outside_root, project.join("escape.txt")).expect("escape symlink");
        let escape = authority(&project, b"escape.txt");
        assert_eq!(
            inspect_external_open_target(&escape),
            Err(ExternalOpenGuardError::SymlinkEscape)
        );
    }
}
