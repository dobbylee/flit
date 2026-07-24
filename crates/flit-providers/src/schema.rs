use std::{
    error::Error,
    ffi::OsString,
    fmt, fs,
    fs::{DirBuilder, File, Metadata},
    io::{self, Read},
    os::unix::fs::{DirBuilderExt, MetadataExt},
    path::{Path, PathBuf},
    process,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use rustix::fs::{Mode, OFlags, open, openat};
use sha2::{Digest, Sha256};

#[cfg(test)]
use crate::process::ProcessFault;
use crate::{
    ExecutableInspection, ExecutableInspectionError, inspect_codex_at,
    process::{ProcessError, ProcessPolicy, run_bounded},
};

pub const SCHEMA_PROBE_TIMEOUT: Duration = Duration::from_secs(10);
pub const MAX_SCHEMA_OUTPUT_BYTES: usize = 64 * 1024;
pub const MAX_SCHEMA_BYTES: u64 = 64 * 1024 * 1024;

const COMBINED_SCHEMA_FILE: &str = "codex_app_server_protocol.schemas.json";
const V2_SCHEMA_FILE: &str = "codex_app_server_protocol.v2.schemas.json";
static NEXT_PROBE_DIRECTORY: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SchemaArtifact {
    Combined,
    V2,
}

impl SchemaArtifact {
    fn file_name(self) -> &'static str {
        match self {
            Self::Combined => COMBINED_SCHEMA_FILE,
            Self::V2 => V2_SCHEMA_FILE,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CodexSchemaProbe {
    pub inspection: ExecutableInspection,
    pub combined_schema_sha256: String,
    pub v2_schema_sha256: String,
    pub stdout_bytes: usize,
    pub stderr_bytes: usize,
}

pub fn probe_codex_schema(
    expected: &ExecutableInspection,
) -> Result<CodexSchemaProbe, CodexSchemaProbeError> {
    probe_codex_schema_with_policy(
        expected,
        SchemaProbePolicy {
            timeout: SCHEMA_PROBE_TIMEOUT,
            max_output_bytes: MAX_SCHEMA_OUTPUT_BYTES,
            max_schema_bytes: MAX_SCHEMA_BYTES,
            #[cfg(test)]
            process_fault: ProcessFault::None,
        },
    )
}

#[derive(Clone, Copy)]
struct SchemaProbePolicy {
    timeout: Duration,
    max_output_bytes: usize,
    max_schema_bytes: u64,
    #[cfg(test)]
    process_fault: ProcessFault,
}

fn probe_codex_schema_with_policy(
    expected: &ExecutableInspection,
    policy: SchemaProbePolicy,
) -> Result<CodexSchemaProbe, CodexSchemaProbeError> {
    probe_codex_schema_with_policy_and_observer(expected, policy, |_| {})
}

fn probe_codex_schema_with_policy_and_observer(
    expected: &ExecutableInspection,
    policy: SchemaProbePolicy,
    observe_directory: impl FnOnce(&Path),
) -> Result<CodexSchemaProbe, CodexSchemaProbeError> {
    let directory = ProbeDirectory::create()?;
    observe_directory(&directory.path);
    let result = probe_codex_schema_in(expected, policy, &directory);
    let cleanup = directory.cleanup();
    match (result, cleanup) {
        (_, Err(error)) => Err(CodexSchemaProbeError::Cleanup(error)),
        (result, Ok(())) => result,
    }
}

fn probe_codex_schema_in(
    expected: &ExecutableInspection,
    policy: SchemaProbePolicy,
    directory: &ProbeDirectory,
) -> Result<CodexSchemaProbe, CodexSchemaProbeError> {
    let before =
        inspect_codex_at(&expected.selected_path).map_err(CodexSchemaProbeError::Inspection)?;
    if !same_identity(expected, &before) {
        return Err(CodexSchemaProbeError::ExecutableIdentityChanged);
    }

    let arguments = [
        OsString::from("app-server"),
        OsString::from("generate-json-schema"),
        OsString::from("--experimental"),
        OsString::from("--out"),
        directory.path.as_os_str().to_owned(),
    ];
    let output = run_bounded(
        &before.canonical_path,
        &arguments,
        ProcessPolicy {
            timeout: policy.timeout,
            max_output_bytes: policy.max_output_bytes,
            #[cfg(test)]
            fault: policy.process_fault,
        },
    )
    .map_err(CodexSchemaProbeError::from)?;

    let after =
        inspect_codex_at(&expected.selected_path).map_err(CodexSchemaProbeError::Inspection)?;
    if !same_identity(&before, &after) {
        return Err(CodexSchemaProbeError::ExecutableIdentityChanged);
    }
    directory.verify_path_identity()?;

    let combined_schema_sha256 =
        digest_artifact(directory, SchemaArtifact::Combined, policy.max_schema_bytes)?;
    let v2_schema_sha256 = digest_artifact(directory, SchemaArtifact::V2, policy.max_schema_bytes)?;

    Ok(CodexSchemaProbe {
        inspection: expected.clone(),
        combined_schema_sha256,
        v2_schema_sha256,
        stdout_bytes: output.stdout.len(),
        stderr_bytes: output.stderr_bytes,
    })
}

fn same_identity(left: &ExecutableInspection, right: &ExecutableInspection) -> bool {
    left.canonical_path == right.canonical_path
        && left.filesystem_id == right.filesystem_id
        && left.sha256 == right.sha256
}

fn digest_artifact(
    directory: &ProbeDirectory,
    artifact: SchemaArtifact,
    max_bytes: u64,
) -> Result<String, CodexSchemaProbeError> {
    let descriptor = openat(
        &directory.descriptor,
        artifact.file_name(),
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK,
        Mode::empty(),
    )
    .map_err(|error| CodexSchemaProbeError::OpenArtifact {
        artifact,
        source: io::Error::from_raw_os_error(error.raw_os_error()),
    })?;
    let mut file = File::from(descriptor);
    let before = file
        .metadata()
        .map_err(|source| CodexSchemaProbeError::InspectArtifact { artifact, source })?;
    if !before.is_file() {
        return Err(CodexSchemaProbeError::ArtifactNotRegular { artifact });
    }
    if before.len() > max_bytes {
        return Err(CodexSchemaProbeError::ArtifactTooLarge {
            artifact,
            max_bytes,
        });
    }

    let mut digest = Sha256::new();
    let mut total_bytes = 0_u64;
    let mut buffer = [0_u8; 16 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|source| CodexSchemaProbeError::ReadArtifact { artifact, source })?;
        if read == 0 {
            break;
        }
        total_bytes = total_bytes.saturating_add(read as u64);
        if total_bytes > max_bytes {
            return Err(CodexSchemaProbeError::ArtifactTooLarge {
                artifact,
                max_bytes,
            });
        }
        digest.update(&buffer[..read]);
    }
    let after = file
        .metadata()
        .map_err(|source| CodexSchemaProbeError::InspectArtifact { artifact, source })?;
    if !same_artifact(&before, &after) || total_bytes != after.len() {
        return Err(CodexSchemaProbeError::ArtifactChanged { artifact });
    }
    Ok(format!("{:x}", digest.finalize()))
}

fn same_artifact(left: &Metadata, right: &Metadata) -> bool {
    left.dev() == right.dev()
        && left.ino() == right.ino()
        && left.len() == right.len()
        && left.mtime() == right.mtime()
        && left.mtime_nsec() == right.mtime_nsec()
}

struct ProbeDirectory {
    path: PathBuf,
    descriptor: File,
    device: u64,
    inode: u64,
    cleaned: bool,
}

impl ProbeDirectory {
    fn create() -> Result<Self, CodexSchemaProbeError> {
        let base = std::env::temp_dir();
        for _ in 0..64 {
            let nonce = NEXT_PROBE_DIRECTORY.fetch_add(1, Ordering::Relaxed);
            let timestamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            let path = base.join(format!(
                "flit-codex-schema-{}-{timestamp}-{nonce}",
                process::id()
            ));
            let mut builder = DirBuilder::new();
            builder.mode(0o700);
            match builder.create(&path) {
                Ok(()) => {
                    let descriptor = match open(
                        &path,
                        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                        Mode::empty(),
                    )
                    .map(File::from)
                    {
                        Ok(descriptor) => descriptor,
                        Err(error) => {
                            let _ = fs::remove_dir_all(&path);
                            return Err(CodexSchemaProbeError::OpenProbeDirectory(
                                io::Error::from_raw_os_error(error.raw_os_error()),
                            ));
                        }
                    };
                    let metadata = match descriptor.metadata() {
                        Ok(metadata) => metadata,
                        Err(error) => {
                            let _ = fs::remove_dir_all(&path);
                            return Err(CodexSchemaProbeError::OpenProbeDirectory(error));
                        }
                    };
                    if !metadata.is_dir() {
                        let _ = fs::remove_dir_all(&path);
                        return Err(CodexSchemaProbeError::ProbeDirectoryChanged);
                    }
                    return Ok(Self {
                        path,
                        descriptor,
                        device: metadata.dev(),
                        inode: metadata.ino(),
                        cleaned: false,
                    });
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(CodexSchemaProbeError::CreateProbeDirectory(error)),
            }
        }
        Err(CodexSchemaProbeError::CreateProbeDirectory(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "could not allocate an exclusive probe directory",
        )))
    }

    fn verify_path_identity(&self) -> Result<(), CodexSchemaProbeError> {
        let metadata = fs::symlink_metadata(&self.path)
            .map_err(CodexSchemaProbeError::InspectProbeDirectory)?;
        if !metadata.is_dir() || metadata.dev() != self.device || metadata.ino() != self.inode {
            return Err(CodexSchemaProbeError::ProbeDirectoryChanged);
        }
        Ok(())
    }

    fn cleanup(mut self) -> Result<(), io::Error> {
        let result = fs::remove_dir_all(&self.path);
        if result.is_ok()
            || result
                .as_ref()
                .is_err_and(|error| error.kind() == io::ErrorKind::NotFound)
        {
            self.cleaned = true;
            Ok(())
        } else {
            result
        }
    }
}

impl Drop for ProbeDirectory {
    fn drop(&mut self) {
        if !self.cleaned {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

#[derive(Debug)]
pub enum CodexSchemaProbeError {
    Inspection(ExecutableInspectionError),
    ExecutableIdentityChanged,
    CreateProbeDirectory(io::Error),
    OpenProbeDirectory(io::Error),
    InspectProbeDirectory(io::Error),
    ProbeDirectoryChanged,
    Spawn(io::Error),
    MissingOutputPipe,
    Wait(io::Error),
    ReadOutput(io::Error),
    ConfigureOutput {
        message: String,
    },
    TerminateProcessGroup {
        message: String,
    },
    TimedOut,
    OutputDrainTimedOut,
    OutputTooLarge {
        max_bytes: usize,
    },
    UnsuccessfulExit {
        code: Option<i32>,
    },
    OpenArtifact {
        artifact: SchemaArtifact,
        source: io::Error,
    },
    InspectArtifact {
        artifact: SchemaArtifact,
        source: io::Error,
    },
    ArtifactNotRegular {
        artifact: SchemaArtifact,
    },
    ReadArtifact {
        artifact: SchemaArtifact,
        source: io::Error,
    },
    ArtifactTooLarge {
        artifact: SchemaArtifact,
        max_bytes: u64,
    },
    ArtifactChanged {
        artifact: SchemaArtifact,
    },
    Cleanup(io::Error),
}

impl From<ProcessError> for CodexSchemaProbeError {
    fn from(error: ProcessError) -> Self {
        match error {
            ProcessError::Spawn(error) => Self::Spawn(error),
            ProcessError::MissingOutputPipe => Self::MissingOutputPipe,
            ProcessError::Wait(error) => Self::Wait(error),
            ProcessError::ReadOutput(error) => Self::ReadOutput(error),
            ProcessError::ConfigureOutput { message } => Self::ConfigureOutput { message },
            ProcessError::TerminateProcessGroup { message } => {
                Self::TerminateProcessGroup { message }
            }
            ProcessError::TimedOut => Self::TimedOut,
            ProcessError::OutputDrainTimedOut => Self::OutputDrainTimedOut,
            ProcessError::OutputTooLarge { max_bytes } => Self::OutputTooLarge { max_bytes },
            ProcessError::UnsuccessfulExit { code } => Self::UnsuccessfulExit { code },
        }
    }
}

impl fmt::Display for CodexSchemaProbeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Inspection(error) => write!(formatter, "Codex inspection failed: {error}"),
            Self::ExecutableIdentityChanged => {
                formatter.write_str("Codex executable identity changed during schema probe")
            }
            Self::CreateProbeDirectory(error) => {
                write!(
                    formatter,
                    "could not create Codex schema probe directory: {error}"
                )
            }
            Self::OpenProbeDirectory(error) => {
                write!(
                    formatter,
                    "could not open Codex schema probe directory: {error}"
                )
            }
            Self::InspectProbeDirectory(error) => {
                write!(
                    formatter,
                    "could not inspect Codex schema probe directory: {error}"
                )
            }
            Self::ProbeDirectoryChanged => {
                formatter.write_str("Codex schema probe directory identity changed")
            }
            Self::Spawn(error) => write!(formatter, "could not start Codex schema probe: {error}"),
            Self::MissingOutputPipe => {
                formatter.write_str("Codex schema probe output pipe was unavailable")
            }
            Self::Wait(error) => {
                write!(formatter, "could not wait for Codex schema probe: {error}")
            }
            Self::ReadOutput(error) => {
                write!(
                    formatter,
                    "could not read Codex schema probe output: {error}"
                )
            }
            Self::ConfigureOutput { message } => {
                write!(
                    formatter,
                    "could not bound Codex schema probe output: {message}"
                )
            }
            Self::TerminateProcessGroup { message } => write!(
                formatter,
                "could not terminate the Codex schema probe process group: {message}"
            ),
            Self::TimedOut => formatter.write_str("Codex schema probe timed out"),
            Self::OutputDrainTimedOut => {
                formatter.write_str("Codex schema probe output did not close before the deadline")
            }
            Self::OutputTooLarge { max_bytes } => write!(
                formatter,
                "Codex schema probe output exceeded the {max_bytes}-byte limit"
            ),
            Self::UnsuccessfulExit { code } => {
                write!(
                    formatter,
                    "Codex schema probe exited unsuccessfully: {code:?}"
                )
            }
            Self::OpenArtifact { artifact, source } => {
                write!(
                    formatter,
                    "could not open {artifact:?} Codex schema: {source}"
                )
            }
            Self::InspectArtifact { artifact, source } => {
                write!(
                    formatter,
                    "could not inspect {artifact:?} Codex schema: {source}"
                )
            }
            Self::ArtifactNotRegular { artifact } => {
                write!(
                    formatter,
                    "{artifact:?} Codex schema was not a regular file"
                )
            }
            Self::ReadArtifact { artifact, source } => {
                write!(
                    formatter,
                    "could not read {artifact:?} Codex schema: {source}"
                )
            }
            Self::ArtifactTooLarge {
                artifact,
                max_bytes,
            } => write!(
                formatter,
                "{artifact:?} Codex schema exceeded the {max_bytes}-byte limit"
            ),
            Self::ArtifactChanged { artifact } => {
                write!(
                    formatter,
                    "{artifact:?} Codex schema changed while being hashed"
                )
            }
            Self::Cleanup(error) => {
                write!(
                    formatter,
                    "could not clean Codex schema probe directory: {error}"
                )
            }
        }
    }
}

impl Error for CodexSchemaProbeError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Inspection(error) => Some(error),
            Self::CreateProbeDirectory(error)
            | Self::OpenProbeDirectory(error)
            | Self::InspectProbeDirectory(error)
            | Self::Spawn(error)
            | Self::Wait(error)
            | Self::ReadOutput(error)
            | Self::Cleanup(error) => Some(error),
            Self::OpenArtifact { source, .. }
            | Self::InspectArtifact { source, .. }
            | Self::ReadArtifact { source, .. } => Some(source),
            _ => None,
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
        time::Duration,
    };

    use sha2::{Digest, Sha256};

    use crate::{SchemaArtifact, inspect_codex_at, process::ProcessFault};

    use super::{
        CodexSchemaProbeError, SchemaProbePolicy, probe_codex_schema_with_policy,
        probe_codex_schema_with_policy_and_observer,
    };

    static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(1);

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new(label: &str) -> Self {
            let nonce = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "flit-provider-schema-{label}-{}-{nonce}",
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
    fn exact_schema_files_are_hashed_without_retaining_content_and_directory_is_removed() {
        let directory = TestDirectory::new("success");
        let executable = directory.0.join("codex");
        let output_path_marker = directory.0.join("output-path");
        let combined = b"{\"combined\":true}\n";
        let v2 = b"{\"v2\":true}\n";
        write_script(
            &executable,
            &format!(
                "#!/bin/sh\n[ \"$1\" = app-server ] && [ \"$2\" = generate-json-schema ] && [ \"$3\" = --experimental ] && [ \"$4\" = --out ] || exit 9\nprintf '%s' \"$5\" > '{}'\nprintf 'private warning' >&2\nprintf '{{\"combined\":true}}\\n' > \"$5/codex_app_server_protocol.schemas.json\"\nprintf '{{\"v2\":true}}\\n' > \"$5/codex_app_server_protocol.v2.schemas.json\"\n",
                output_path_marker.display()
            ),
        );
        let inspection = inspect_codex_at(&executable).expect("inspection");

        let probe =
            probe_codex_schema_with_policy(&inspection, test_policy()).expect("schema probe");
        assert_eq!(
            probe.combined_schema_sha256,
            format!("{:x}", Sha256::digest(combined))
        );
        assert_eq!(probe.v2_schema_sha256, format!("{:x}", Sha256::digest(v2)));
        assert_eq!(probe.stdout_bytes, 0);
        assert_eq!(probe.stderr_bytes, "private warning".len());
        assert_eq!(probe.inspection, inspection);
        let output_path =
            PathBuf::from(fs::read_to_string(output_path_marker).expect("output path marker"));
        assert!(
            !output_path.exists(),
            "probe-owned directory must be removed after success"
        );
    }

    #[test]
    fn unsuccessful_generation_fails_closed_and_removes_directory() {
        let directory = TestDirectory::new("status");
        let executable = directory.0.join("codex");
        write_script(
            &executable,
            "#!/bin/sh\nprintf 'private failure' >&2\nexit 7\n",
        );
        let inspection = inspect_codex_at(&executable).expect("inspection");
        let mut output_path = None;

        let error =
            probe_codex_schema_with_policy_and_observer(&inspection, test_policy(), |path| {
                output_path = Some(path.to_owned())
            })
            .expect_err("unsuccessful generator must fail");
        assert!(matches!(
            error,
            CodexSchemaProbeError::UnsuccessfulExit { code: Some(7) }
        ));
        assert!(!output_path.expect("observed output path").exists());
    }

    #[test]
    fn timeout_and_combined_output_overflow_terminate_descendants_and_remove_directory() {
        let directory = TestDirectory::new("process-bounds");
        let timeout = directory.0.join("timeout");
        let timeout_descendant_marker = directory.0.join("timeout-descendant");
        write_script(
            &timeout,
            &format!(
                "#!/bin/sh\n(/bin/sleep 1.5; /usr/bin/touch '{}') &\n/bin/sleep 5\n",
                timeout_descendant_marker.display()
            ),
        );
        let inspection = inspect_codex_at(&timeout).expect("timeout inspection");
        let mut timeout_output_path = None;
        assert!(matches!(
            probe_codex_schema_with_policy_and_observer(&inspection, timeout_policy(), |path| {
                timeout_output_path = Some(path.to_owned())
            }),
            Err(CodexSchemaProbeError::TimedOut)
        ));
        std::thread::sleep(Duration::from_millis(1_700));
        assert!(!timeout_descendant_marker.exists());
        assert!(!timeout_output_path.expect("observed timeout path").exists());

        let overflow = directory.0.join("overflow");
        write_script(
            &overflow,
            "#!/bin/sh\ni=0\nwhile [ \"$i\" -lt 80 ]; do printf x; i=$((i + 1)); done\ni=0\nwhile [ \"$i\" -lt 80 ]; do printf y >&2; i=$((i + 1)); done\n",
        );
        let inspection = inspect_codex_at(&overflow).expect("overflow inspection");
        let mut overflow_output_path = None;
        let overflow_error =
            probe_codex_schema_with_policy_and_observer(&inspection, test_policy(), |path| {
                overflow_output_path = Some(path.to_owned())
            })
            .expect_err("combined output must overflow");
        assert!(
            matches!(
                &overflow_error,
                CodexSchemaProbeError::OutputTooLarge { max_bytes: 128 }
            ),
            "{overflow_error:?}"
        );
        assert!(
            !overflow_output_path
                .expect("observed overflow path")
                .exists()
        );
    }

    #[test]
    fn missing_symlink_and_oversized_schema_artifacts_fail_closed() {
        let directory = TestDirectory::new("artifacts");
        let cases = [
            (
                "missing",
                "printf '{}' > \"$5/codex_app_server_protocol.schemas.json\"\n",
                ArtifactFailure::MissingV2,
            ),
            (
                "symlink",
                "printf '{}' > \"$5/codex_app_server_protocol.schemas.json\"\n/bin/ln -s /dev/null \"$5/codex_app_server_protocol.v2.schemas.json\"\n",
                ArtifactFailure::SymlinkV2,
            ),
            (
                "oversized",
                "i=0\nwhile [ \"$i\" -lt 64 ]; do printf x; i=$((i + 1)); done > \"$5/codex_app_server_protocol.schemas.json\"\nprintf '{}' > \"$5/codex_app_server_protocol.v2.schemas.json\"\n",
                ArtifactFailure::OversizedCombined,
            ),
        ];

        for (name, body, expected) in cases {
            let executable = directory.0.join(name);
            write_script(&executable, &format!("#!/bin/sh\n{body}"));
            let inspection = inspect_codex_at(&executable).expect("inspection");
            let error = probe_codex_schema_with_policy(
                &inspection,
                SchemaProbePolicy {
                    max_schema_bytes: 32,
                    ..test_policy()
                },
            )
            .expect_err("invalid artifact must fail");
            assert!(expected.matches(&error), "{name}: {error:?}");
        }
    }

    #[test]
    fn fifo_schema_artifacts_fail_without_blocking_and_remove_directory() {
        let directory = TestDirectory::new("fifo-artifacts");
        let cases = [
            (
                "combined",
                "/usr/bin/mkfifo \"$5/codex_app_server_protocol.schemas.json\"\nprintf '{}' > \"$5/codex_app_server_protocol.v2.schemas.json\"\n",
                SchemaArtifact::Combined,
            ),
            (
                "v2",
                "printf '{}' > \"$5/codex_app_server_protocol.schemas.json\"\n/usr/bin/mkfifo \"$5/codex_app_server_protocol.v2.schemas.json\"\n",
                SchemaArtifact::V2,
            ),
        ];

        for (name, body, expected_artifact) in cases {
            let executable = directory.0.join(name);
            write_script(&executable, &format!("#!/bin/sh\n{body}"));
            let inspection = inspect_codex_at(&executable).expect("inspection");
            let (sender, receiver) = std::sync::mpsc::channel();
            let handle = std::thread::spawn(move || {
                let mut output_path = None;
                let result = probe_codex_schema_with_policy_and_observer(
                    &inspection,
                    test_policy(),
                    |path| output_path = Some(path.to_owned()),
                );
                sender
                    .send((result, output_path))
                    .expect("send FIFO probe result");
            });
            let (result, output_path) = receiver
                .recv_timeout(Duration::from_secs(30))
                .expect("FIFO artifact validation must return within thirty seconds");
            let error = result.expect_err("FIFO artifact must fail");
            assert!(matches!(
                error,
                CodexSchemaProbeError::ArtifactNotRegular { artifact }
                    if artifact == expected_artifact
            ));
            assert!(!output_path.expect("observed output path").exists());
            handle.join().expect("FIFO probe thread");
        }
    }

    #[test]
    fn executable_and_probe_directory_identity_drift_fail_closed() {
        let directory = TestDirectory::new("identity");
        let executable_drift = directory.0.join("executable-drift");
        write_script(
            &executable_drift,
            "#!/bin/sh\nprintf '#!/bin/sh\\n' > \"$0\"\n/bin/chmod 700 \"$0\"\nprintf '{}' > \"$5/codex_app_server_protocol.schemas.json\"\nprintf '{}' > \"$5/codex_app_server_protocol.v2.schemas.json\"\n",
        );
        let inspection = inspect_codex_at(&executable_drift).expect("inspection");
        assert!(matches!(
            probe_codex_schema_with_policy(&inspection, test_policy()),
            Err(CodexSchemaProbeError::ExecutableIdentityChanged)
        ));

        let directory_drift = directory.0.join("directory-drift");
        let output_path_marker = directory.0.join("drift-output-path");
        write_script(
            &directory_drift,
            &format!(
                "#!/bin/sh\nprintf '%s' \"$5\" > '{}'\n/bin/rm -rf \"$5\"\n/bin/mkdir \"$5\"\nprintf '{{}}' > \"$5/codex_app_server_protocol.schemas.json\"\nprintf '{{}}' > \"$5/codex_app_server_protocol.v2.schemas.json\"\n",
                output_path_marker.display()
            ),
        );
        let inspection = inspect_codex_at(&directory_drift).expect("inspection");
        assert!(matches!(
            probe_codex_schema_with_policy(&inspection, test_policy()),
            Err(CodexSchemaProbeError::ProbeDirectoryChanged)
        ));
        let output_path =
            PathBuf::from(fs::read_to_string(output_path_marker).expect("drift path marker"));
        assert!(!output_path.exists());
    }

    enum ArtifactFailure {
        MissingV2,
        SymlinkV2,
        OversizedCombined,
    }

    impl ArtifactFailure {
        fn matches(&self, error: &CodexSchemaProbeError) -> bool {
            matches!(
                (self, error),
                (
                    Self::MissingV2 | Self::SymlinkV2,
                    CodexSchemaProbeError::OpenArtifact {
                        artifact: SchemaArtifact::V2,
                        ..
                    }
                ) | (
                    Self::OversizedCombined,
                    CodexSchemaProbeError::ArtifactTooLarge {
                        artifact: SchemaArtifact::Combined,
                        max_bytes: 32
                    }
                )
            )
        }
    }

    fn test_policy() -> SchemaProbePolicy {
        SchemaProbePolicy {
            timeout: Duration::from_secs(5),
            max_output_bytes: 128,
            max_schema_bytes: 1024,
            process_fault: ProcessFault::None,
        }
    }

    fn timeout_policy() -> SchemaProbePolicy {
        SchemaProbePolicy {
            timeout: Duration::from_secs(1),
            ..test_policy()
        }
    }

    fn write_script(path: &Path, script: &str) {
        fs::write(path, script).expect("write script");
        let mut permissions = fs::metadata(path).expect("metadata").permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(path, permissions).expect("permissions");
    }
}
