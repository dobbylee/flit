use std::{
    error::Error,
    ffi::OsStr,
    fmt, fs,
    fs::File,
    io::{self, Read},
    path::{Path, PathBuf},
};

#[cfg(unix)]
use std::os::unix::fs::MetadataExt;

#[cfg(unix)]
use rustix::fs::{Access, AtFlags, CWD, accessat};
use sha2::{Digest, Sha256};

pub const MAX_EXECUTABLE_BYTES: u64 = 512 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExecutableSelectionSource {
    PathEnvironment,
    Explicit,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExecutableInspection {
    pub source: ExecutableSelectionSource,
    pub selected_path: PathBuf,
    pub canonical_path: PathBuf,
    pub filesystem_id: String,
    pub sha256: String,
}

pub fn inspect_codex_on_path(
    path_environment: Option<&OsStr>,
) -> Result<ExecutableInspection, ExecutableInspectionError> {
    let searched_directories = path_environment
        .map(std::env::split_paths)
        .into_iter()
        .flatten()
        .filter(|directory| directory.is_absolute())
        .collect::<Vec<_>>();

    for directory in &searched_directories {
        let candidate = directory.join("codex");
        let Ok(metadata) = fs::metadata(&candidate) else {
            continue;
        };
        if metadata.is_file() && path_is_executable(&candidate) {
            return inspect_executable(candidate, ExecutableSelectionSource::PathEnvironment);
        }
    }

    Err(ExecutableInspectionError::NotFoundOnPath {
        searched_directories,
    })
}

pub fn inspect_codex_at(
    selected_path: impl AsRef<Path>,
) -> Result<ExecutableInspection, ExecutableInspectionError> {
    let selected_path = selected_path.as_ref();
    if !selected_path.is_absolute() {
        return Err(ExecutableInspectionError::ExplicitPathNotAbsolute {
            selected_path: selected_path.to_owned(),
        });
    }
    inspect_executable(
        selected_path.to_owned(),
        ExecutableSelectionSource::Explicit,
    )
}

fn inspect_executable(
    selected_path: PathBuf,
    source: ExecutableSelectionSource,
) -> Result<ExecutableInspection, ExecutableInspectionError> {
    inspect_executable_with_observer(selected_path, source, |_| {})
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum InspectionPhase {
    BeforeOpen,
    Hashing,
    AfterHashMetadata,
}

fn inspect_executable_with_observer(
    selected_path: PathBuf,
    source: ExecutableSelectionSource,
    mut observer: impl FnMut(InspectionPhase),
) -> Result<ExecutableInspection, ExecutableInspectionError> {
    let canonical_path = fs::canonicalize(&selected_path).map_err(|error| {
        ExecutableInspectionError::Canonicalize {
            selected_path: selected_path.clone(),
            source: error,
        }
    })?;
    let path_metadata_before =
        fs::metadata(&canonical_path).map_err(|error| ExecutableInspectionError::ReadMetadata {
            canonical_path: canonical_path.clone(),
            source: error,
        })?;
    observer(InspectionPhase::BeforeOpen);
    let mut file =
        File::open(&canonical_path).map_err(|error| ExecutableInspectionError::OpenExecutable {
            canonical_path: canonical_path.clone(),
            source: error,
        })?;
    let handle_metadata_before =
        file.metadata()
            .map_err(|error| ExecutableInspectionError::ReadMetadata {
                canonical_path: canonical_path.clone(),
                source: error,
            })?;
    if !same_file(&path_metadata_before, &handle_metadata_before) {
        return Err(ExecutableInspectionError::ChangedDuringInspection { selected_path });
    }
    if !handle_metadata_before.is_file() {
        return Err(ExecutableInspectionError::NotRegularFile { canonical_path });
    }
    if !path_is_executable(&canonical_path) {
        return Err(ExecutableInspectionError::NotExecutable { canonical_path });
    }
    let initial_length = handle_metadata_before.len();
    if initial_length > MAX_EXECUTABLE_BYTES {
        return Err(ExecutableInspectionError::ExecutableTooLarge {
            canonical_path,
            size_bytes: initial_length,
            max_bytes: MAX_EXECUTABLE_BYTES,
        });
    }
    let filesystem_id = metadata_filesystem_id(&handle_metadata_before)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    let mut observed_hashing = false;
    let mut remaining = initial_length;
    while remaining > 0 {
        let read_limit = usize::try_from(remaining.min(buffer.len() as u64))
            .expect("bounded buffer length fits usize");
        let read = file.read(&mut buffer[..read_limit]).map_err(|error| {
            ExecutableInspectionError::ReadExecutable {
                canonical_path: canonical_path.clone(),
                source: error,
            }
        })?;
        if read == 0 {
            return Err(ExecutableInspectionError::ChangedDuringInspection { selected_path });
        }
        hasher.update(&buffer[..read]);
        remaining -= read as u64;
        if !observed_hashing {
            observer(InspectionPhase::Hashing);
            observed_hashing = true;
        }
    }
    let handle_metadata_after =
        file.metadata()
            .map_err(|error| ExecutableInspectionError::ReadMetadata {
                canonical_path: canonical_path.clone(),
                source: error,
            })?;
    observer(InspectionPhase::AfterHashMetadata);
    let final_canonical_path = fs::canonicalize(&selected_path).map_err(|error| {
        ExecutableInspectionError::Canonicalize {
            selected_path: selected_path.clone(),
            source: error,
        }
    })?;
    let path_metadata_after =
        fs::metadata(&canonical_path).map_err(|error| ExecutableInspectionError::ReadMetadata {
            canonical_path: canonical_path.clone(),
            source: error,
        })?;
    if handle_signature(&handle_metadata_before) != handle_signature(&handle_metadata_after)
        || handle_signature(&handle_metadata_after) != handle_signature(&path_metadata_after)
        || final_canonical_path != canonical_path
        || !path_is_executable(&canonical_path)
    {
        return Err(ExecutableInspectionError::ChangedDuringInspection { selected_path });
    }

    Ok(ExecutableInspection {
        source,
        selected_path,
        canonical_path,
        filesystem_id,
        sha256: format!("{:x}", hasher.finalize()),
    })
}

#[cfg(unix)]
fn path_is_executable(path: &Path) -> bool {
    accessat(CWD, path, Access::EXEC_OK, AtFlags::EACCESS).is_ok()
}

#[cfg(not(unix))]
fn path_is_executable(path: &Path) -> bool {
    let _ = path;
    false
}

#[cfg(unix)]
fn same_file(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    left.dev() == right.dev() && left.ino() == right.ino()
}

#[cfg(not(unix))]
fn same_file(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    let _ = (left, right);
    false
}

#[cfg(unix)]
fn handle_signature(metadata: &fs::Metadata) -> (u64, u64, u64, i64, i64, i64, i64, u32) {
    (
        metadata.dev(),
        metadata.ino(),
        metadata.len(),
        metadata.mtime(),
        metadata.mtime_nsec(),
        metadata.ctime(),
        metadata.ctime_nsec(),
        metadata.mode(),
    )
}

#[cfg(not(unix))]
fn handle_signature(metadata: &fs::Metadata) -> (u64, u64, u64, i64, i64, i64, i64, u32) {
    let _ = metadata;
    (0, 0, 0, 0, 0, 0, 0, 0)
}

#[cfg(unix)]
fn metadata_filesystem_id(metadata: &fs::Metadata) -> Result<String, ExecutableInspectionError> {
    Ok(format!("unix:{}:{}", metadata.dev(), metadata.ino()))
}

#[cfg(not(unix))]
fn metadata_filesystem_id(metadata: &fs::Metadata) -> Result<String, ExecutableInspectionError> {
    let _ = metadata;
    Err(ExecutableInspectionError::UnsupportedPlatform)
}

#[derive(Debug)]
pub enum ExecutableInspectionError {
    NotFoundOnPath {
        searched_directories: Vec<PathBuf>,
    },
    ExplicitPathNotAbsolute {
        selected_path: PathBuf,
    },
    Canonicalize {
        selected_path: PathBuf,
        source: io::Error,
    },
    OpenExecutable {
        canonical_path: PathBuf,
        source: io::Error,
    },
    ReadMetadata {
        canonical_path: PathBuf,
        source: io::Error,
    },
    NotRegularFile {
        canonical_path: PathBuf,
    },
    NotExecutable {
        canonical_path: PathBuf,
    },
    ExecutableTooLarge {
        canonical_path: PathBuf,
        size_bytes: u64,
        max_bytes: u64,
    },
    ReadExecutable {
        canonical_path: PathBuf,
        source: io::Error,
    },
    ChangedDuringInspection {
        selected_path: PathBuf,
    },
    UnsupportedPlatform,
}

impl fmt::Display for ExecutableInspectionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFoundOnPath {
                searched_directories,
            } => write!(
                formatter,
                "Codex executable was not found in {} absolute PATH directories",
                searched_directories.len()
            ),
            Self::ExplicitPathNotAbsolute { selected_path } => write!(
                formatter,
                "selected Codex executable path is not absolute: {}",
                selected_path.display()
            ),
            Self::Canonicalize {
                selected_path,
                source,
            } => write!(
                formatter,
                "could not canonicalize Codex executable {}: {source}",
                selected_path.display()
            ),
            Self::OpenExecutable {
                canonical_path,
                source,
            } => write!(
                formatter,
                "could not open Codex executable {}: {source}",
                canonical_path.display()
            ),
            Self::ReadMetadata {
                canonical_path,
                source,
            } => write!(
                formatter,
                "could not read Codex executable metadata {}: {source}",
                canonical_path.display()
            ),
            Self::NotRegularFile { canonical_path } => write!(
                formatter,
                "Codex executable is not a regular file: {}",
                canonical_path.display()
            ),
            Self::NotExecutable { canonical_path } => write!(
                formatter,
                "Codex file does not have an executable bit: {}",
                canonical_path.display()
            ),
            Self::ExecutableTooLarge {
                canonical_path,
                size_bytes,
                max_bytes,
            } => write!(
                formatter,
                "Codex executable {} is {size_bytes} bytes, above the {max_bytes}-byte inspection limit",
                canonical_path.display()
            ),
            Self::ReadExecutable {
                canonical_path,
                source,
            } => write!(
                formatter,
                "could not hash Codex executable {}: {source}",
                canonical_path.display()
            ),
            Self::ChangedDuringInspection { selected_path } => write!(
                formatter,
                "Codex executable changed during inspection: {}",
                selected_path.display()
            ),
            Self::UnsupportedPlatform => {
                formatter.write_str("Codex executable identity is unsupported on this platform")
            }
        }
    }
}

impl Error for ExecutableInspectionError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Canonicalize { source, .. }
            | Self::OpenExecutable { source, .. }
            | Self::ReadMetadata { source, .. }
            | Self::ReadExecutable { source, .. } => Some(source),
            Self::NotFoundOnPath { .. }
            | Self::ExplicitPathNotAbsolute { .. }
            | Self::NotRegularFile { .. }
            | Self::NotExecutable { .. }
            | Self::ExecutableTooLarge { .. }
            | Self::ChangedDuringInspection { .. }
            | Self::UnsupportedPlatform => None,
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::{
        fs,
        os::unix::fs::PermissionsExt,
        path::{Path, PathBuf},
        process,
        sync::atomic::{AtomicU64, Ordering},
    };

    use super::{
        ExecutableInspectionError, ExecutableSelectionSource, InspectionPhase,
        MAX_EXECUTABLE_BYTES, inspect_executable_with_observer,
    };

    static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(1);

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new(label: &str) -> Self {
            let nonce = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "flit-provider-executable-unit-{label}-{}-{nonce}",
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

    #[test]
    fn replacement_between_path_metadata_and_open_fails_closed() {
        let directory = TestDirectory::new("replacement");
        let executable = directory.0.join("codex");
        write_executable(&executable, b"original");
        let moved = directory.0.join("original");

        let result = inspect_executable_with_observer(
            executable.clone(),
            ExecutableSelectionSource::Explicit,
            |phase| {
                if phase == InspectionPhase::BeforeOpen {
                    fs::rename(&executable, &moved).expect("move original");
                    write_executable(&executable, b"replacement");
                }
            },
        );
        assert!(matches!(
            result,
            Err(ExecutableInspectionError::ChangedDuringInspection { .. })
        ));
    }

    #[test]
    fn in_place_mutation_during_hashing_fails_closed() {
        let directory = TestDirectory::new("hash-mutation");
        let executable = directory.0.join("codex");
        write_executable(&executable, &vec![b'a'; 256 * 1024]);

        let result = inspect_executable_with_observer(
            executable.clone(),
            ExecutableSelectionSource::Explicit,
            |phase| {
                if phase == InspectionPhase::Hashing {
                    fs::write(&executable, vec![b'b'; 256 * 1024]).expect("mutate executable");
                }
            },
        );
        assert!(matches!(
            result,
            Err(ExecutableInspectionError::ChangedDuringInspection { .. })
        ));
        assert!(
            inspect_executable_with_observer(
                executable,
                ExecutableSelectionSource::Explicit,
                |_| {}
            )
            .is_ok()
        );
    }

    #[test]
    fn in_place_mutation_after_hash_metadata_fails_closed() {
        let directory = TestDirectory::new("post-hash-mutation");
        let executable = directory.0.join("codex");
        write_executable(&executable, b"original");

        let result = inspect_executable_with_observer(
            executable.clone(),
            ExecutableSelectionSource::Explicit,
            |phase| {
                if phase == InspectionPhase::AfterHashMetadata {
                    fs::write(&executable, b"changed!").expect("mutate executable");
                }
            },
        );
        assert!(matches!(
            result,
            Err(ExecutableInspectionError::ChangedDuringInspection { .. })
        ));
    }

    #[test]
    fn growing_file_is_bounded_by_its_initial_length_and_fails_closed() {
        let directory = TestDirectory::new("growth");
        let executable = directory.0.join("codex");
        write_executable(&executable, &vec![b'a'; 128 * 1024]);

        let result = inspect_executable_with_observer(
            executable.clone(),
            ExecutableSelectionSource::Explicit,
            |phase| {
                if phase == InspectionPhase::Hashing {
                    fs::OpenOptions::new()
                        .write(true)
                        .open(&executable)
                        .expect("open growing executable")
                        .set_len(MAX_EXECUTABLE_BYTES + 1)
                        .expect("grow sparse executable");
                }
            },
        );
        assert!(matches!(
            result,
            Err(ExecutableInspectionError::ChangedDuringInspection { .. })
        ));
    }

    #[test]
    fn initially_oversized_executable_is_rejected_before_hashing() {
        let directory = TestDirectory::new("oversized");
        let executable = directory.0.join("codex");
        write_executable(&executable, b"");
        fs::OpenOptions::new()
            .write(true)
            .open(&executable)
            .expect("open executable")
            .set_len(MAX_EXECUTABLE_BYTES + 1)
            .expect("make sparse executable oversized");

        assert!(matches!(
            inspect_executable_with_observer(
                executable,
                ExecutableSelectionSource::Explicit,
                |_| {}
            ),
            Err(ExecutableInspectionError::ExecutableTooLarge {
                size_bytes,
                max_bytes,
                ..
            }) if size_bytes == MAX_EXECUTABLE_BYTES + 1 && max_bytes == MAX_EXECUTABLE_BYTES
        ));
    }

    fn write_executable(path: &Path, content: &[u8]) {
        fs::write(path, content).expect("write executable");
        let mut permissions = fs::metadata(path).expect("metadata").permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(path, permissions).expect("permissions");
    }
}
