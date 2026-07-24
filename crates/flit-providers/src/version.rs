use std::{error::Error, ffi::OsString, fmt, io, time::Duration};

#[cfg(test)]
use crate::process::ProcessFault;
use crate::{
    ExecutableInspection, ExecutableInspectionError, inspect_codex_at,
    process::{ProcessError, ProcessPolicy, run_bounded},
};

pub const VERSION_PROBE_TIMEOUT: Duration = Duration::from_secs(3);
pub const MAX_VERSION_OUTPUT_BYTES: usize = 64 * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CodexVersionProbe {
    pub inspection: ExecutableInspection,
    pub executable_version: String,
    pub stderr_bytes: usize,
}

pub fn probe_codex_version(
    expected: &ExecutableInspection,
) -> Result<CodexVersionProbe, CodexVersionProbeError> {
    probe_codex_version_with_policy(
        expected,
        ProbePolicy {
            timeout: VERSION_PROBE_TIMEOUT,
            max_output_bytes: MAX_VERSION_OUTPUT_BYTES,
            #[cfg(test)]
            fault: ProcessFault::None,
        },
    )
}

#[derive(Clone, Copy)]
struct ProbePolicy {
    timeout: Duration,
    max_output_bytes: usize,
    #[cfg(test)]
    fault: ProcessFault,
}

fn probe_codex_version_with_policy(
    expected: &ExecutableInspection,
    policy: ProbePolicy,
) -> Result<CodexVersionProbe, CodexVersionProbeError> {
    let before =
        inspect_codex_at(&expected.selected_path).map_err(CodexVersionProbeError::Inspection)?;
    if !same_identity(expected, &before) {
        return Err(CodexVersionProbeError::ExecutableIdentityChanged);
    }

    let output = run_bounded(
        &before.canonical_path,
        &[OsString::from("--version")],
        ProcessPolicy {
            timeout: policy.timeout,
            max_output_bytes: policy.max_output_bytes,
            #[cfg(test)]
            fault: policy.fault,
        },
    )
    .map_err(CodexVersionProbeError::from)?;
    let after =
        inspect_codex_at(&expected.selected_path).map_err(CodexVersionProbeError::Inspection)?;
    if !same_identity(&before, &after) {
        return Err(CodexVersionProbeError::ExecutableIdentityChanged);
    }
    let executable_version = parse_version(&output.stdout)?;

    Ok(CodexVersionProbe {
        inspection: expected.clone(),
        executable_version,
        stderr_bytes: output.stderr_bytes,
    })
}

fn same_identity(left: &ExecutableInspection, right: &ExecutableInspection) -> bool {
    left.canonical_path == right.canonical_path
        && left.filesystem_id == right.filesystem_id
        && left.sha256 == right.sha256
}

fn parse_version(stdout: &[u8]) -> Result<String, CodexVersionProbeError> {
    let output =
        std::str::from_utf8(stdout).map_err(|_| CodexVersionProbeError::InvalidUtf8Output)?;
    let line = output
        .strip_suffix("\r\n")
        .or_else(|| output.strip_suffix('\n'))
        .unwrap_or(output);
    if line.contains(['\r', '\n']) {
        return Err(CodexVersionProbeError::InvalidVersionOutput);
    }
    let version = line
        .strip_prefix("codex-cli ")
        .ok_or(CodexVersionProbeError::InvalidVersionOutput)?;
    let parts = version.split('.').collect::<Vec<_>>();
    if parts.len() != 3
        || parts
            .iter()
            .any(|part| part.is_empty() || !part.bytes().all(|byte| byte.is_ascii_digit()))
    {
        return Err(CodexVersionProbeError::InvalidVersionOutput);
    }
    Ok(version.to_owned())
}

#[derive(Debug)]
pub enum CodexVersionProbeError {
    Inspection(ExecutableInspectionError),
    ExecutableIdentityChanged,
    Spawn(io::Error),
    MissingOutputPipe,
    Wait(io::Error),
    ReadOutput(io::Error),
    ConfigureOutput { message: String },
    TerminateProcessGroup { message: String },
    TimedOut,
    OutputDrainTimedOut,
    OutputTooLarge { max_bytes: usize },
    UnsuccessfulExit { code: Option<i32> },
    InvalidUtf8Output,
    InvalidVersionOutput,
}

impl From<ProcessError> for CodexVersionProbeError {
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

impl fmt::Display for CodexVersionProbeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Inspection(error) => write!(formatter, "Codex inspection failed: {error}"),
            Self::ExecutableIdentityChanged => {
                formatter.write_str("Codex executable identity changed during version probe")
            }
            Self::Spawn(error) => write!(formatter, "could not start Codex version probe: {error}"),
            Self::MissingOutputPipe => {
                formatter.write_str("Codex version probe output pipe was unavailable")
            }
            Self::Wait(error) => {
                write!(formatter, "could not wait for Codex version probe: {error}")
            }
            Self::ReadOutput(error) => {
                write!(formatter, "could not read Codex version output: {error}")
            }
            Self::ConfigureOutput { message } => {
                write!(formatter, "could not bound Codex version output: {message}")
            }
            Self::TerminateProcessGroup { message } => write!(
                formatter,
                "could not terminate the Codex version probe process group: {message}"
            ),
            Self::TimedOut => formatter.write_str("Codex version probe timed out"),
            Self::OutputDrainTimedOut => {
                formatter.write_str("Codex version output did not close before the deadline")
            }
            Self::OutputTooLarge { max_bytes } => write!(
                formatter,
                "Codex version output exceeded the {max_bytes}-byte limit"
            ),
            Self::UnsuccessfulExit { code } => {
                write!(
                    formatter,
                    "Codex version probe exited unsuccessfully: {code:?}"
                )
            }
            Self::InvalidUtf8Output => {
                formatter.write_str("Codex version output was not valid UTF-8")
            }
            Self::InvalidVersionOutput => {
                formatter.write_str("Codex version output did not match the expected format")
            }
        }
    }
}

impl Error for CodexVersionProbeError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Inspection(error) => Some(error),
            Self::Spawn(error) | Self::Wait(error) | Self::ReadOutput(error) => Some(error),
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

    use crate::inspect_codex_at;

    use crate::process::ProcessFault;

    use super::{CodexVersionProbeError, ProbePolicy, probe_codex_version_with_policy};

    static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(1);

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new(label: &str) -> Self {
            let nonce = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "flit-provider-version-{label}-{}-{nonce}",
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
    fn exact_version_is_parsed_without_retaining_stderr_content() {
        let directory = TestDirectory::new("success");
        let executable = directory.0.join("codex");
        write_script(
            &executable,
            "#!/bin/sh\n/bin/sleep 5 &\nprintf 'private warning' >&2\nprintf 'codex-cli 0.145.0\\n'\n",
        );
        let inspection = inspect_codex_at(&executable).expect("inspection");

        let started = std::time::Instant::now();
        let result = probe_codex_version_with_policy(&inspection, test_policy()).expect("probe");
        assert!(started.elapsed() < Duration::from_millis(2_500));
        assert_eq!(result.executable_version, "0.145.0");
        assert_eq!(result.stderr_bytes, "private warning".len());
        assert_eq!(result.inspection, inspection);
    }

    #[test]
    fn timeout_and_combined_output_overflow_fail_closed() {
        let directory = TestDirectory::new("bounds");
        let timeout = directory.0.join("timeout");
        let descendant_marker = directory.0.join("descendant-marker");
        write_script(
            &timeout,
            &format!(
                "#!/bin/sh\n(/bin/sleep 0.3; /usr/bin/touch '{}') &\n/bin/sleep 5\n",
                descendant_marker.display()
            ),
        );
        let timeout_inspection = inspect_codex_at(&timeout).expect("timeout inspection");
        assert!(matches!(
            probe_codex_version_with_policy(&timeout_inspection, timeout_test_policy()),
            Err(CodexVersionProbeError::TimedOut)
        ));
        std::thread::sleep(Duration::from_millis(400));
        assert!(
            !descendant_marker.exists(),
            "timeout must terminate the whole probe process group"
        );

        let overflow = directory.0.join("overflow");
        let overflow_marker = directory.0.join("overflow-marker");
        write_script(
            &overflow,
            &format!(
                "#!/bin/sh\n(/bin/sleep 0.3; /usr/bin/touch '{}') &\ni=0\nwhile [ \"$i\" -lt 80 ]; do printf x; i=$((i + 1)); done\ni=0\nwhile [ \"$i\" -lt 80 ]; do printf y >&2; i=$((i + 1)); done\n",
                overflow_marker.display()
            ),
        );
        let overflow_inspection = inspect_codex_at(&overflow).expect("overflow inspection");
        assert!(matches!(
            probe_codex_version_with_policy(&overflow_inspection, test_policy()),
            Err(CodexVersionProbeError::OutputTooLarge { max_bytes: 128 })
        ));
        std::thread::sleep(Duration::from_millis(400));
        assert!(
            !overflow_marker.exists(),
            "overflow must terminate descendant work"
        );

        let escaped = directory.0.join("escaped");
        let escaped_ready = directory.0.join("escaped-ready");
        write_script(
            &escaped,
            &format!(
                "#!/bin/sh\n/usr/bin/perl -MPOSIX -e 'POSIX::setsid(); open(F, \">{}\"); close(F); $|=1; while (1) {{ print STDOUT \"x\"; select(undef, undef, undef, 0.1); }}' &\nwhile [ ! -e '{}' ]; do :; done\nprintf 'codex-cli 0.1.0\\n'\n",
                escaped_ready.display(),
                escaped_ready.display()
            ),
        );
        let escaped_inspection = inspect_codex_at(&escaped).expect("escaped inspection");
        let started = std::time::Instant::now();
        let escaped_error = probe_codex_version_with_policy(
            &escaped_inspection,
            ProbePolicy {
                timeout: Duration::from_secs(1),
                max_output_bytes: 128,
                fault: ProcessFault::None,
            },
        )
        .expect_err("escaped pipe must fail");
        assert!(
            matches!(escaped_error, CodexVersionProbeError::OutputDrainTimedOut),
            "{escaped_error:?}"
        );
        assert!(started.elapsed() < Duration::from_millis(1_500));
    }

    #[test]
    fn infinite_output_and_post_spawn_errors_clean_up_descendants() {
        let directory = TestDirectory::new("cleanup");
        let infinite = directory.0.join("infinite");
        let infinite_marker = directory.0.join("infinite-marker");
        write_script(
            &infinite,
            &format!(
                "#!/bin/sh\n(/bin/sleep 0.3; /usr/bin/touch '{}') &\nwhile :; do printf x; done\n",
                infinite_marker.display()
            ),
        );
        let inspection = inspect_codex_at(&infinite).expect("infinite inspection");
        let started = std::time::Instant::now();
        assert!(matches!(
            probe_codex_version_with_policy(
                &inspection,
                ProbePolicy {
                    timeout: Duration::from_secs(2),
                    max_output_bytes: 128,
                    fault: ProcessFault::None,
                }
            ),
            Err(CodexVersionProbeError::OutputTooLarge { max_bytes: 128 })
        ));
        assert!(started.elapsed() < Duration::from_millis(2_500));
        std::thread::sleep(Duration::from_millis(400));
        assert!(!infinite_marker.exists());

        for (name, fault, expected) in [
            (
                "configure",
                ProcessFault::ConfigureOutput,
                FailureKind::ConfigureOutput,
            ),
            ("read", ProcessFault::ReadOutput, FailureKind::ReadOutput),
        ] {
            let executable = directory.0.join(name);
            let marker = directory.0.join(format!("{name}-marker"));
            write_script(
                &executable,
                &format!(
                    "#!/bin/sh\n(/bin/sleep 0.3; /usr/bin/touch '{}') &\n/bin/sleep 5\n",
                    marker.display()
                ),
            );
            let inspection = inspect_codex_at(&executable).expect("fault inspection");
            let error = probe_codex_version_with_policy(
                &inspection,
                ProbePolicy {
                    timeout: Duration::from_secs(1),
                    max_output_bytes: 128,
                    fault,
                },
            )
            .expect_err("injected probe failure");
            assert!(expected.matches(&error));
            std::thread::sleep(Duration::from_millis(400));
            assert!(!marker.exists(), "{name} descendant survived");
        }
    }

    #[test]
    fn invalid_status_output_and_identity_drift_fail_closed() {
        let directory = TestDirectory::new("failures");
        for (name, script, expected) in [
            ("status", "#!/bin/sh\nexit 7\n", FailureKind::Unsuccessful),
            (
                "multiline",
                "#!/bin/sh\nprintf 'codex-cli 0.1.0\\nextra\\n'\n",
                FailureKind::InvalidFormat,
            ),
            (
                "non-utf8",
                "#!/bin/sh\nprintf '\\377'\n",
                FailureKind::InvalidUtf8,
            ),
        ] {
            let executable = directory.0.join(name);
            write_script(&executable, script);
            let inspection = inspect_codex_at(&executable).expect("inspection");
            let error = probe_codex_version_with_policy(&inspection, test_policy())
                .expect_err("probe must fail");
            assert!(expected.matches(&error), "{name}: {error:?}");
        }

        let drift = directory.0.join("drift");
        write_script(
            &drift,
            "#!/bin/sh\nprintf '#!/bin/sh\\n' > \"$0\"\nchmod 700 \"$0\"\nprintf 'codex-cli 0.1.0\\n'\n",
        );
        let inspection = inspect_codex_at(&drift).expect("drift inspection");
        assert!(matches!(
            probe_codex_version_with_policy(&inspection, test_policy()),
            Err(CodexVersionProbeError::ExecutableIdentityChanged)
        ));
    }

    enum FailureKind {
        Unsuccessful,
        InvalidFormat,
        InvalidUtf8,
        ConfigureOutput,
        ReadOutput,
    }

    impl FailureKind {
        fn matches(&self, error: &CodexVersionProbeError) -> bool {
            matches!(
                (self, error),
                (
                    Self::Unsuccessful,
                    CodexVersionProbeError::UnsuccessfulExit { code: Some(7) }
                ) | (
                    Self::InvalidFormat,
                    CodexVersionProbeError::InvalidVersionOutput
                ) | (Self::InvalidUtf8, CodexVersionProbeError::InvalidUtf8Output)
                    | (
                        Self::ConfigureOutput,
                        CodexVersionProbeError::ConfigureOutput { .. }
                    )
                    | (Self::ReadOutput, CodexVersionProbeError::ReadOutput(_))
            )
        }
    }

    fn test_policy() -> ProbePolicy {
        ProbePolicy {
            timeout: Duration::from_secs(5),
            max_output_bytes: 128,
            fault: ProcessFault::None,
        }
    }

    fn timeout_test_policy() -> ProbePolicy {
        ProbePolicy {
            timeout: Duration::from_millis(100),
            max_output_bytes: 128,
            fault: ProcessFault::None,
        }
    }

    fn write_script(path: &Path, script: &str) {
        fs::write(path, script).expect("write script");
        let mut permissions = fs::metadata(path).expect("metadata").permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(path, permissions).expect("permissions");
    }
}
