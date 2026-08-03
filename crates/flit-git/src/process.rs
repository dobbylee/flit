use std::{
    ffi::OsString,
    io::{self, Read},
    os::unix::process::CommandExt,
    path::Path,
    process::{Command, ExitStatus, Stdio},
    thread,
    time::{Duration, Instant},
};

use rustix::{
    fs::{OFlags, fcntl_getfl, fcntl_setfl},
    process::{Pid, Signal, kill_process_group},
};

#[derive(Clone, Copy)]
pub(crate) struct ProcessPolicy {
    pub timeout: Duration,
    pub max_output_bytes: usize,
}

#[derive(Debug)]
pub(crate) struct ProcessOutput {
    pub status: ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

pub(crate) fn run_bounded(
    executable: &Path,
    arguments: &[OsString],
    policy: ProcessPolicy,
) -> Result<ProcessOutput, ProcessError> {
    let mut command = Command::new(executable);
    command
        .args(arguments)
        .env_clear()
        .env("GIT_ATTR_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_NO_LAZY_FETCH", "1")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);
    let mut child = command.spawn().map_err(|_| ProcessError::Spawn)?;
    let Some(mut stdout) = child.stdout.take() else {
        terminate_process_group(&mut child)?;
        return Err(ProcessError::MissingOutputPipe);
    };
    let Some(mut stderr) = child.stderr.take() else {
        terminate_process_group(&mut child)?;
        return Err(ProcessError::MissingOutputPipe);
    };
    if let Err(error) = set_nonblocking(&stdout).and_then(|()| set_nonblocking(&stderr)) {
        terminate_process_group(&mut child)?;
        return Err(error);
    }

    let mut stdout_capture = CapturedOutput::default();
    let mut stderr_capture = CapturedOutput::default();
    let mut combined_bytes = 0_usize;
    let mut exit_status = None;
    let mut group_terminated = false;
    let started = Instant::now();
    let outcome = loop {
        let stdout_result = drain_output(
            &mut stdout,
            &mut stdout_capture,
            &mut combined_bytes,
            policy.max_output_bytes,
        );
        if let Err(error) = stdout_result {
            ensure_terminated(&mut child, &mut group_terminated)?;
            return Err(error);
        }
        let stderr_result = drain_output(
            &mut stderr,
            &mut stderr_capture,
            &mut combined_bytes,
            policy.max_output_bytes,
        );
        if let Err(error) = stderr_result {
            ensure_terminated(&mut child, &mut group_terminated)?;
            return Err(error);
        }
        if combined_bytes > policy.max_output_bytes {
            ensure_terminated(&mut child, &mut group_terminated)?;
            break ProcessOutcome::OutputExceeded;
        }
        if exit_status.is_none() {
            match child.try_wait() {
                Err(_) => {
                    ensure_terminated(&mut child, &mut group_terminated)?;
                    break ProcessOutcome::WaitFailed;
                }
                Ok(Some(status)) => {
                    ensure_terminated(&mut child, &mut group_terminated)?;
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
            ensure_terminated(&mut child, &mut group_terminated)?;
            break if exit_status.is_some() {
                ProcessOutcome::OutputDrainTimedOut
            } else {
                ProcessOutcome::TimedOut
            };
        }
        thread::sleep(Duration::from_millis(5));
    };

    match outcome {
        ProcessOutcome::TimedOut => Err(ProcessError::TimedOut),
        ProcessOutcome::OutputDrainTimedOut => Err(ProcessError::OutputDrainTimedOut),
        ProcessOutcome::OutputExceeded => Err(ProcessError::OutputTooLarge),
        ProcessOutcome::WaitFailed => Err(ProcessError::Wait),
        ProcessOutcome::Exited(status) => Ok(ProcessOutput {
            status,
            stdout: stdout_capture.bytes,
            stderr: stderr_capture.bytes,
        }),
    }
}

fn ensure_terminated(
    child: &mut std::process::Child,
    group_terminated: &mut bool,
) -> Result<(), ProcessError> {
    if !*group_terminated {
        terminate_process_group(child)?;
        *group_terminated = true;
    }
    Ok(())
}

enum ProcessOutcome {
    Exited(ExitStatus),
    TimedOut,
    OutputDrainTimedOut,
    OutputExceeded,
    WaitFailed,
}

#[derive(Default)]
struct CapturedOutput {
    bytes: Vec<u8>,
    eof: bool,
}

fn set_nonblocking(fd: &impl std::os::fd::AsFd) -> Result<(), ProcessError> {
    let flags = fcntl_getfl(fd).map_err(|_| ProcessError::ConfigureOutput)?;
    fcntl_setfl(fd, flags | OFlags::NONBLOCK).map_err(|_| ProcessError::ConfigureOutput)
}

fn drain_output(
    reader: &mut impl Read,
    captured: &mut CapturedOutput,
    combined_bytes: &mut usize,
    max_bytes: usize,
) -> Result<(), ProcessError> {
    let mut buffer = [0_u8; 4096];
    loop {
        match reader.read(&mut buffer) {
            Ok(0) => {
                captured.eof = true;
                return Ok(());
            }
            Ok(read) => {
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
            Err(_) => return Err(ProcessError::ReadOutput),
        }
    }
}

fn terminate_process_group(child: &mut std::process::Child) -> Result<(), ProcessError> {
    let mut group_failed = false;
    if let Some(pid) = Pid::from_raw(child.id() as i32) {
        for attempt in 0..20 {
            match kill_process_group(pid, Signal::KILL) {
                Ok(()) | Err(rustix::io::Errno::SRCH) => break,
                Err(error) if error == rustix::io::Errno::PERM => {
                    if child.try_wait().map_err(|_| ProcessError::Wait)?.is_some() {
                        break;
                    }
                    if attempt == 19 {
                        group_failed = true;
                        break;
                    }
                    thread::sleep(Duration::from_millis(1));
                }
                Err(_) => {
                    group_failed = true;
                    break;
                }
            }
        }
    }
    let _ = child.kill();
    child.wait().map_err(|_| ProcessError::Wait)?;
    if group_failed {
        return Err(ProcessError::TerminateProcessGroup);
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ProcessError {
    Spawn,
    MissingOutputPipe,
    Wait,
    ReadOutput,
    ConfigureOutput,
    TerminateProcessGroup,
    TimedOut,
    OutputDrainTimedOut,
    OutputTooLarge,
}

#[cfg(test)]
mod tests {
    use std::{
        ffi::OsString,
        fs, process, thread,
        time::{Duration, Instant},
    };

    use rustix::process::{Pid, test_kill_process_group};

    use super::{ProcessError, ProcessPolicy, run_bounded};

    #[test]
    fn timeout_terminates_the_process_group() {
        let process_group_path =
            std::env::temp_dir().join(format!("flit-git-timeout-process-group-{}", process::id()));
        let result = run_bounded(
            std::path::Path::new("/bin/sh"),
            &[
                OsString::from("-c"),
                OsString::from("printf '%s' \"$$\" > \"$1\"; /bin/sleep 30 & wait"),
                OsString::from("flit-git-test"),
                process_group_path.as_os_str().to_owned(),
            ],
            ProcessPolicy {
                timeout: Duration::from_millis(100),
                max_output_bytes: 1024,
            },
        );

        assert_eq!(result.expect_err("timeout"), ProcessError::TimedOut);
        let process_group = fs::read_to_string(&process_group_path)
            .expect("process group marker")
            .parse::<i32>()
            .ok()
            .and_then(Pid::from_raw)
            .expect("positive process group ID");
        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            match test_kill_process_group(process_group) {
                Err(error) if error == rustix::io::Errno::SRCH => break,
                _ if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
                result => panic!("process group survived cleanup: {result:?}"),
            }
        }
        fs::remove_file(process_group_path).expect("remove process group marker");
    }

    #[test]
    fn combined_stdout_and_stderr_overflow_terminates_the_process_group() {
        let result = run_bounded(
            std::path::Path::new("/bin/sh"),
            &[
                OsString::from("-c"),
                OsString::from("while :; do printf 1234567890; printf abcdefghij >&2; done"),
            ],
            ProcessPolicy {
                timeout: Duration::from_secs(1),
                max_output_bytes: 128,
            },
        );

        assert_eq!(
            result.expect_err("output overflow"),
            ProcessError::OutputTooLarge
        );
    }
}
