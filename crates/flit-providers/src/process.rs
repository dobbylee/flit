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

#[cfg(test)]
static TEST_PROCESS_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[derive(Clone, Copy)]
pub(crate) struct ProcessPolicy {
    pub timeout: Duration,
    pub max_output_bytes: usize,
    #[cfg(test)]
    pub fault: ProcessFault,
}

#[cfg(test)]
#[derive(Clone, Copy)]
pub(crate) enum ProcessFault {
    None,
    ConfigureOutput,
    ReadOutput,
}

pub(crate) struct ProcessOutput {
    pub stdout: Vec<u8>,
    pub stderr_bytes: usize,
}

pub(crate) fn run_bounded(
    executable: &Path,
    arguments: &[OsString],
    policy: ProcessPolicy,
) -> Result<ProcessOutput, ProcessError> {
    #[cfg(test)]
    let _test_process_guard = TEST_PROCESS_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut command = Command::new(executable);
    command
        .args(arguments)
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);
    let mut child = command.spawn().map_err(ProcessError::Spawn)?;
    let Some(mut stdout) = child.stdout.take() else {
        terminate_process_group(&mut child)?;
        return Err(ProcessError::MissingOutputPipe);
    };
    let Some(mut stderr) = child.stderr.take() else {
        terminate_process_group(&mut child)?;
        return Err(ProcessError::MissingOutputPipe);
    };
    let configuration = if inject_configuration_failure(&policy) {
        Err(ProcessError::ConfigureOutput {
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
        if combined_bytes > policy.max_output_bytes {
            ensure_terminated(&mut child, &mut group_terminated)?;
            break ProcessOutcome::OutputExceeded;
        }
        let stderr_result = if inject_read_failure(&policy) {
            Err(ProcessError::ReadOutput(io::Error::other(
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
            ensure_terminated(&mut child, &mut group_terminated)?;
            return Err(error);
        }
        if combined_bytes > policy.max_output_bytes {
            ensure_terminated(&mut child, &mut group_terminated)?;
            break ProcessOutcome::OutputExceeded;
        }
        if exit_status.is_none() {
            match child.try_wait() {
                Err(error) => {
                    ensure_terminated(&mut child, &mut group_terminated)?;
                    break ProcessOutcome::WaitFailed(error);
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
        ProcessOutcome::OutputExceeded => Err(ProcessError::OutputTooLarge {
            max_bytes: policy.max_output_bytes,
        }),
        ProcessOutcome::WaitFailed(error) => Err(ProcessError::Wait(error)),
        ProcessOutcome::Exited(status) if !status.success() => {
            Err(ProcessError::UnsuccessfulExit {
                code: status.code(),
            })
        }
        ProcessOutcome::Exited(_) => Ok(ProcessOutput {
            stdout: stdout_capture.bytes,
            stderr_bytes: stderr_capture.total_bytes,
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

fn inject_configuration_failure(policy: &ProcessPolicy) -> bool {
    #[cfg(test)]
    {
        matches!(policy.fault, ProcessFault::ConfigureOutput)
    }
    #[cfg(not(test))]
    {
        let _ = policy;
        false
    }
}

fn inject_read_failure(policy: &ProcessPolicy) -> bool {
    #[cfg(test)]
    {
        matches!(policy.fault, ProcessFault::ReadOutput)
    }
    #[cfg(not(test))]
    {
        let _ = policy;
        false
    }
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

fn set_nonblocking(fd: &impl std::os::fd::AsFd) -> Result<(), ProcessError> {
    let flags = fcntl_getfl(fd).map_err(|error| ProcessError::ConfigureOutput {
        message: error.to_string(),
    })?;
    fcntl_setfl(fd, flags | OFlags::NONBLOCK).map_err(|error| ProcessError::ConfigureOutput {
        message: error.to_string(),
    })
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
            Err(error) => return Err(ProcessError::ReadOutput(error)),
        }
    }
}

fn terminate_process_group(child: &mut std::process::Child) -> Result<(), ProcessError> {
    let mut group_error = None;
    if let Some(pid) = Pid::from_raw(child.id() as i32) {
        for attempt in 0..20 {
            match kill_process_group(pid, Signal::KILL) {
                Ok(()) | Err(rustix::io::Errno::SRCH) => break,
                Err(error) if error == rustix::io::Errno::PERM => {
                    if child.try_wait().map_err(ProcessError::Wait)?.is_some() {
                        break;
                    }
                    if attempt == 19 {
                        group_error = Some(error);
                        break;
                    }
                    thread::sleep(Duration::from_millis(1));
                }
                Err(error) => {
                    group_error = Some(error);
                    break;
                }
            }
        }
    }
    let _ = child.kill();
    child.wait().map_err(ProcessError::Wait)?;
    if let Some(error) = group_error {
        return Err(ProcessError::TerminateProcessGroup {
            message: error.to_string(),
        });
    }
    Ok(())
}

#[derive(Debug)]
pub(crate) enum ProcessError {
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
}
