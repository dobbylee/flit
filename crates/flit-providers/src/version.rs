use std::{
    error::Error,
    fmt,
    io::{self, Read},
    os::unix::process::CommandExt,
    process::{Command, ExitStatus, Stdio},
    thread,
    time::{Duration, Instant},
};

use rustix::{
    fs::{OFlags, fcntl_getfl, fcntl_setfl},
    process::{Pid, Signal, kill_process_group},
};

use crate::{ExecutableInspection, ExecutableInspectionError, inspect_codex_at};

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
            fault: ProbeFault::None,
        },
    )
}

#[derive(Clone, Copy)]
struct ProbePolicy {
    timeout: Duration,
    max_output_bytes: usize,
    #[cfg(test)]
    fault: ProbeFault,
}

#[cfg(test)]
#[derive(Clone, Copy)]
enum ProbeFault {
    None,
    ConfigureOutput,
    ReadOutput,
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

    let mut child = Command::new(&before.canonical_path)
        .arg("--version")
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()
        .map_err(CodexVersionProbeError::Spawn)?;
    let Some(mut stdout) = child.stdout.take() else {
        terminate_process_group(&mut child)?;
        return Err(CodexVersionProbeError::MissingOutputPipe);
    };
    let Some(mut stderr) = child.stderr.take() else {
        terminate_process_group(&mut child)?;
        return Err(CodexVersionProbeError::MissingOutputPipe);
    };
    let configuration = if inject_configuration_failure(&policy) {
        Err(CodexVersionProbeError::ConfigureOutput {
            message: "injected failure".to_owned(),
        })
    } else {
        set_nonblocking(&stdout).and_then(|()| set_nonblocking(&stderr))
    };
    if let Err(error) = configuration {
        terminate_process_group(&mut child)?;
        return Err(error);
    }
    let mut stdout_capture = CapturedOutput::default();
    let mut stderr_capture = CapturedOutput::default();
    let mut combined_bytes = 0_usize;
    let mut exit_status = None;
    let started = Instant::now();
    let outcome = loop {
        let stdout_result = drain_output(
            &mut stdout,
            &mut stdout_capture,
            &mut combined_bytes,
            policy.max_output_bytes,
        );
        if let Err(error) = stdout_result {
            terminate_process_group(&mut child)?;
            return Err(error);
        }
        if combined_bytes > policy.max_output_bytes {
            terminate_process_group(&mut child)?;
            break ProcessOutcome::OutputExceeded;
        }
        let stderr_result = if inject_read_failure(&policy) {
            Err(CodexVersionProbeError::ReadOutput(io::Error::other(
                "injected failure",
            )))
        } else {
            drain_output(
                &mut stderr,
                &mut stderr_capture,
                &mut combined_bytes,
                policy.max_output_bytes,
            )
        };
        if let Err(error) = stderr_result {
            terminate_process_group(&mut child)?;
            return Err(error);
        }
        if combined_bytes > policy.max_output_bytes {
            terminate_process_group(&mut child)?;
            break ProcessOutcome::OutputExceeded;
        }
        if exit_status.is_none() {
            match child.try_wait() {
                Err(error) => {
                    terminate_process_group(&mut child)?;
                    break ProcessOutcome::WaitFailed(error);
                }
                Ok(Some(status)) => {
                    terminate_process_group(&mut child)?;
                    exit_status = Some(status);
                }
                Ok(None) => {}
            }
        }
        if stdout_capture.eof
            && stderr_capture.eof
            && let Some(status) = exit_status
        {
            break ProcessOutcome::Exited(status);
        }
        if started.elapsed() >= policy.timeout {
            terminate_process_group(&mut child)?;
            break if exit_status.is_some() {
                ProcessOutcome::OutputDrainTimedOut
            } else {
                ProcessOutcome::TimedOut
            };
        }
        thread::sleep(Duration::from_millis(5));
    };

    match outcome {
        ProcessOutcome::TimedOut => return Err(CodexVersionProbeError::TimedOut),
        ProcessOutcome::OutputDrainTimedOut => {
            return Err(CodexVersionProbeError::OutputDrainTimedOut);
        }
        ProcessOutcome::OutputExceeded => {
            return Err(CodexVersionProbeError::OutputTooLarge {
                max_bytes: policy.max_output_bytes,
            });
        }
        ProcessOutcome::WaitFailed(error) => return Err(CodexVersionProbeError::Wait(error)),
        ProcessOutcome::Exited(status) if !status.success() => {
            return Err(CodexVersionProbeError::UnsuccessfulExit {
                code: status.code(),
            });
        }
        ProcessOutcome::Exited(_) => {}
    }
    let after =
        inspect_codex_at(&expected.selected_path).map_err(CodexVersionProbeError::Inspection)?;
    if !same_identity(&before, &after) {
        return Err(CodexVersionProbeError::ExecutableIdentityChanged);
    }
    let executable_version = parse_version(&stdout_capture.bytes)?;

    Ok(CodexVersionProbe {
        inspection: expected.clone(),
        executable_version,
        stderr_bytes: stderr_capture.total_bytes,
    })
}

fn inject_configuration_failure(policy: &ProbePolicy) -> bool {
    #[cfg(test)]
    {
        matches!(policy.fault, ProbeFault::ConfigureOutput)
    }
    #[cfg(not(test))]
    {
        let _ = policy;
        false
    }
}

fn inject_read_failure(policy: &ProbePolicy) -> bool {
    #[cfg(test)]
    {
        matches!(policy.fault, ProbeFault::ReadOutput)
    }
    #[cfg(not(test))]
    {
        let _ = policy;
        false
    }
}

fn same_identity(left: &ExecutableInspection, right: &ExecutableInspection) -> bool {
    left.canonical_path == right.canonical_path
        && left.filesystem_id == right.filesystem_id
        && left.sha256 == right.sha256
}

enum ProcessOutcome {
    Exited(ExitStatus),
    TimedOut,
    OutputDrainTimedOut,
    OutputExceeded,
    WaitFailed(io::Error),
}

#[derive(Default)]
struct CapturedOutput {
    bytes: Vec<u8>,
    total_bytes: usize,
    eof: bool,
}

fn set_nonblocking(fd: &impl std::os::fd::AsFd) -> Result<(), CodexVersionProbeError> {
    let flags = fcntl_getfl(fd).map_err(|error| CodexVersionProbeError::ConfigureOutput {
        message: error.to_string(),
    })?;
    fcntl_setfl(fd, flags | OFlags::NONBLOCK).map_err(|error| {
        CodexVersionProbeError::ConfigureOutput {
            message: error.to_string(),
        }
    })
}

fn drain_output(
    reader: &mut impl Read,
    captured: &mut CapturedOutput,
    combined_bytes: &mut usize,
    max_bytes: usize,
) -> Result<(), CodexVersionProbeError> {
    let mut buffer = [0_u8; 4096];
    loop {
        match reader.read(&mut buffer) {
            Ok(0) => {
                captured.eof = true;
                return Ok(());
            }
            Ok(read) => {
                captured.total_bytes = captured.total_bytes.saturating_add(read);
                let available = max_bytes.saturating_sub(*combined_bytes);
                captured
                    .bytes
                    .extend_from_slice(&buffer[..read.min(available)]);
                *combined_bytes = combined_bytes.saturating_add(read);
                if *combined_bytes > max_bytes {
                    return Ok(());
                }
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(()),
            Err(error) => return Err(CodexVersionProbeError::ReadOutput(error)),
        }
    }
}

fn terminate_process_group(child: &mut std::process::Child) -> Result<(), CodexVersionProbeError> {
    let mut group_error = None;
    if let Some(pid) = Pid::from_raw(child.id() as i32)
        && let Err(error) = kill_process_group(pid, Signal::KILL)
        && error != rustix::io::Errno::SRCH
    {
        group_error = Some(error);
    }
    let _ = child.kill();
    child.wait().map_err(CodexVersionProbeError::Wait)?;
    if let Some(error) = group_error {
        return Err(CodexVersionProbeError::TerminateProcessGroup {
            message: error.to_string(),
        });
    }
    Ok(())
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

    use super::{CodexVersionProbeError, ProbeFault, ProbePolicy, probe_codex_version_with_policy};

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
                "#!/bin/sh\n(/bin/sleep 0.3; /usr/bin/touch '{}') &\nwhile :; do :; done\n",
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
                fault: ProbeFault::None,
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
                    fault: ProbeFault::None,
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
                ProbeFault::ConfigureOutput,
                FailureKind::ConfigureOutput,
            ),
            ("read", ProbeFault::ReadOutput, FailureKind::ReadOutput),
        ] {
            let executable = directory.0.join(name);
            let marker = directory.0.join(format!("{name}-marker"));
            write_script(
                &executable,
                &format!(
                    "#!/bin/sh\n(/bin/sleep 0.3; /usr/bin/touch '{}') &\nwhile :; do :; done\n",
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
            timeout: Duration::from_secs(2),
            max_output_bytes: 128,
            fault: ProbeFault::None,
        }
    }

    fn timeout_test_policy() -> ProbePolicy {
        ProbePolicy {
            timeout: Duration::from_millis(100),
            max_output_bytes: 128,
            fault: ProbeFault::None,
        }
    }

    fn write_script(path: &Path, script: &str) {
        fs::write(path, script).expect("write script");
        let mut permissions = fs::metadata(path).expect("metadata").permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(path, permissions).expect("permissions");
    }
}
