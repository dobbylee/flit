use std::{ffi::OsString, os::unix::process::CommandExt, path::PathBuf, process::Command};

use rustix::process::{Resource, Rlimit, geteuid, getrlimit, setrlimit};

use crate::{FileIdentity, GitExecutable, GitRunnerFailure, executable_matches};

const PROTOCOL: &str = "flit-git-noexec-v1";

pub(crate) fn arguments(executable: &GitExecutable, git_arguments: Vec<OsString>) -> Vec<OsString> {
    let identity = &executable.identity;
    let mut arguments = vec![
        OsString::from(PROTOCOL),
        executable.canonical_path.as_os_str().to_owned(),
        OsString::from(identity.device.to_string()),
        OsString::from(identity.inode.to_string()),
        OsString::from(identity.length.to_string()),
        OsString::from(identity.modified_seconds.to_string()),
        OsString::from(identity.modified_nanoseconds.to_string()),
        OsString::from(identity.changed_seconds.to_string()),
        OsString::from(identity.changed_nanoseconds.to_string()),
        OsString::from(identity.mode.to_string()),
        OsString::from("--"),
    ];
    arguments.extend(git_arguments);
    arguments
}

pub(crate) fn decode_failure(output: &crate::process::ProcessOutput) -> Option<GitRunnerFailure> {
    if !output.stdout.is_empty() {
        return None;
    }
    match (output.status.code(), output.stderr.as_slice()) {
        (Some(120), b"flit-git-noexec:v1:invalid-arguments\n") => {
            Some(GitRunnerFailure::InvalidArguments)
        }
        (Some(121), b"flit-git-noexec:v1:root-unsupported\n") => {
            Some(GitRunnerFailure::RootUnsupported)
        }
        (Some(122), b"flit-git-noexec:v1:limit-set-failed\n") => {
            Some(GitRunnerFailure::LimitSetFailed)
        }
        (Some(123), b"flit-git-noexec:v1:limit-verification-failed\n") => {
            Some(GitRunnerFailure::LimitVerificationFailed)
        }
        (Some(124), b"flit-git-noexec:v1:git-identity-changed\n") => {
            Some(GitRunnerFailure::GitIdentityChanged)
        }
        (Some(126), b"flit-git-noexec:v1:exec-failed\n") => Some(GitRunnerFailure::ExecFailed),
        _ => None,
    }
}

pub fn main() -> ! {
    let Some((executable, expected_identity, git_arguments)) = parse_arguments() else {
        fail(120, "flit-git-noexec:v1:invalid-arguments");
    };
    if geteuid().is_root() {
        fail(121, "flit-git-noexec:v1:root-unsupported");
    }
    if setrlimit(
        Resource::Nproc,
        Rlimit {
            current: Some(0),
            maximum: Some(0),
        },
    )
    .is_err()
    {
        fail(122, "flit-git-noexec:v1:limit-set-failed");
    }
    let effective = getrlimit(Resource::Nproc);
    if effective.current != Some(0) || effective.maximum != Some(0) {
        fail(123, "flit-git-noexec:v1:limit-verification-failed");
    }
    if !executable_matches(&executable, &expected_identity) {
        fail(124, "flit-git-noexec:v1:git-identity-changed");
    }

    let _error = Command::new(executable).args(git_arguments).exec();
    fail(126, "flit-git-noexec:v1:exec-failed");
}

fn parse_arguments() -> Option<(PathBuf, FileIdentity, Vec<OsString>)> {
    let mut arguments = std::env::args_os();
    arguments.next()?;
    if arguments.next()? != PROTOCOL {
        return None;
    }
    let executable = PathBuf::from(arguments.next()?);
    let identity = FileIdentity {
        device: parse(&arguments.next()?)?,
        inode: parse(&arguments.next()?)?,
        length: parse(&arguments.next()?)?,
        modified_seconds: parse(&arguments.next()?)?,
        modified_nanoseconds: parse(&arguments.next()?)?,
        changed_seconds: parse(&arguments.next()?)?,
        changed_nanoseconds: parse(&arguments.next()?)?,
        mode: parse(&arguments.next()?)?,
    };
    if arguments.next()?.as_os_str() != "--" {
        return None;
    }
    let git_arguments = arguments.collect::<Vec<_>>();
    if git_arguments.is_empty() || !executable.is_absolute() {
        return None;
    }
    Some((executable, identity, git_arguments))
}

fn parse<T: std::str::FromStr>(value: &std::ffi::OsStr) -> Option<T> {
    value.to_str()?.parse().ok()
}

fn fail(code: i32, message: &str) -> ! {
    eprintln!("{message}");
    std::process::exit(code);
}
