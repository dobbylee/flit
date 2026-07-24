use std::{
    fmt, fs, io,
    path::{Component, Path, PathBuf},
};

#[cfg(unix)]
use std::os::unix::fs::MetadataExt;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectIdentity {
    pub canonical_path: PathBuf,
    pub filesystem_id: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectDirectoryInspection {
    pub identity: ProjectIdentity,
    pub selected_via_symlink: bool,
}

impl ProjectDirectoryInspection {
    pub fn inspect(selected_path: impl AsRef<Path>) -> Result<Self, ProjectInspectionError> {
        let selected_path = selected_path.as_ref();
        let selected_via_symlink = selected_path_traverses_symlink(selected_path)?;
        let canonical_path = fs::canonicalize(selected_path).map_err(|source| {
            ProjectInspectionError::Canonicalize {
                selected_path: selected_path.to_owned(),
                source,
            }
        })?;
        let metadata =
            fs::metadata(&canonical_path).map_err(|source| ProjectInspectionError::Metadata {
                canonical_path: canonical_path.clone(),
                source,
            })?;
        if !metadata.is_dir() {
            return Err(ProjectInspectionError::NotDirectory { canonical_path });
        }

        Ok(Self {
            selected_via_symlink,
            identity: ProjectIdentity {
                canonical_path,
                filesystem_id: filesystem_id(&metadata),
            },
        })
    }
}

pub(crate) fn is_valid_filesystem_id(filesystem_id: &str) -> bool {
    let Some((device, inode)) = filesystem_id
        .strip_prefix("unix:")
        .and_then(|value| value.split_once(':'))
    else {
        return false;
    };
    let (Ok(device), Ok(inode)) = (device.parse::<u64>(), inode.parse::<u64>()) else {
        return false;
    };
    filesystem_id == format!("unix:{device}:{inode}")
}

fn selected_path_traverses_symlink(path: &Path) -> Result<bool, ProjectInspectionError> {
    let absolute_path = absolute_path(path)?;
    let mut traversed_path = PathBuf::new();
    for component in absolute_path.components() {
        match component {
            Component::Prefix(prefix) => traversed_path.push(prefix.as_os_str()),
            Component::RootDir => traversed_path.push(Path::new("/")),
            Component::CurDir => {}
            Component::ParentDir => {
                traversed_path.pop();
            }
            Component::Normal(segment) => {
                traversed_path.push(segment);
                let metadata = fs::symlink_metadata(&traversed_path).map_err(|source| {
                    ProjectInspectionError::Canonicalize {
                        selected_path: path.to_owned(),
                        source,
                    }
                })?;
                if metadata.file_type().is_symlink() {
                    return Ok(true);
                }
            }
        }
    }
    Ok(false)
}

fn absolute_path(path: &Path) -> Result<PathBuf, ProjectInspectionError> {
    if path.is_absolute() {
        Ok(path.to_owned())
    } else {
        let current_directory =
            std::env::current_dir().map_err(ProjectInspectionError::CurrentDirectory)?;
        Ok(current_directory.join(path))
    }
}

#[cfg(unix)]
fn filesystem_id(metadata: &fs::Metadata) -> String {
    format!("unix:{}:{}", metadata.dev(), metadata.ino())
}

#[cfg(not(unix))]
fn filesystem_id(metadata: &fs::Metadata) -> String {
    let _ = metadata;
    "unsupported".to_owned()
}

#[derive(Debug)]
pub enum ProjectInspectionError {
    CurrentDirectory(io::Error),
    Canonicalize {
        selected_path: PathBuf,
        source: io::Error,
    },
    Metadata {
        canonical_path: PathBuf,
        source: io::Error,
    },
    NotDirectory {
        canonical_path: PathBuf,
    },
}

impl fmt::Display for ProjectInspectionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CurrentDirectory(error) => {
                write!(
                    formatter,
                    "could not resolve the current directory: {error}"
                )
            }
            Self::Canonicalize {
                selected_path,
                source,
            } => write!(
                formatter,
                "could not canonicalize Project directory {}: {source}",
                selected_path.display()
            ),
            Self::Metadata {
                canonical_path,
                source,
            } => write!(
                formatter,
                "could not inspect Project directory {}: {source}",
                canonical_path.display()
            ),
            Self::NotDirectory { canonical_path } => {
                write!(
                    formatter,
                    "Project path is not a directory: {}",
                    canonical_path.display()
                )
            }
        }
    }
}

impl std::error::Error for ProjectInspectionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::CurrentDirectory(error) => Some(error),
            Self::Canonicalize { source, .. } | Self::Metadata { source, .. } => Some(source),
            Self::NotDirectory { .. } => None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectRegistration {
    pub id: String,
    pub display_name: String,
    pub selected_path: PathBuf,
    pub created_at: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectTrustConfirmation {
    pub project_id: String,
    pub selected_path: PathBuf,
    pub confirmed_at: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Project {
    pub id: String,
    pub display_name: String,
    pub canonical_path: PathBuf,
    pub filesystem_id: Option<String>,
    pub trusted: bool,
    pub default_provider: Option<String>,
    pub notification_policy_json: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProjectRegistrationOutcome {
    Registered(Project),
    DuplicateCanonicalPath { existing_project_id: String },
    DuplicateFilesystemIdentity { existing_project_id: String },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProjectTrustOutcome {
    Trusted(Project),
    AlreadyTrusted(Project),
}
