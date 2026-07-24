use std::{
    collections::{BTreeSet, VecDeque},
    error::Error,
    ffi::{OsStr, OsString},
    fmt, fs,
    io::{self, Read, Write},
    os::unix::{
        fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt},
        process::CommandExt,
    },
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError, SyncSender},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use serde_json::Value;

use crate::{
    CapabilityStatus, CodexCompatibilityProbe, CodexCompatibilityProbeError, CodexContractError,
    CodexManagedScope, CodexManagedThreadConflict, CodexManagedThreadId, CodexStartedThread,
    CodexThreadRead, ExecutableInspection, ExecutableInspectionError, ProviderCapability,
    ProviderCompatibility, ProviderFingerprint, codex_initialize_request,
    codex_initialized_notification, codex_read_only_start_request, codex_read_request,
    codex_thread_list_request, decode_codex_initialize_response, decode_codex_read_response,
    decode_codex_start_response, decode_codex_thread_list_response, inspect_codex_at,
    probe_codex_compatibility_at, probe_codex_compatibility_on_path,
    process::{set_nonblocking, terminate_process_group},
};

pub const CODEX_APP_SERVER_REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
pub const MAX_CODEX_APP_SERVER_STDERR_BYTES: usize = 64 * 1024;
pub const MAX_CODEX_PENDING_NOTIFICATIONS: usize = 64;
pub const MAX_CODEX_PENDING_NOTIFICATION_BYTES: usize = 1024 * 1024;
pub const MAX_CODEX_LIST_PAGES: usize = 16;
const INBOUND_FRAME_QUEUE_CAPACITY: usize = 16;
const RECEIVE_POLL_INTERVAL: Duration = Duration::from_millis(10);
const IO_POLL_INTERVAL: Duration = Duration::from_millis(5);
static NEXT_STAGED_EXECUTABLE: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CodexManagedThreads {
    pub matched_thread_ids: Vec<CodexManagedThreadId>,
    pub conflicting_threads: Vec<CodexManagedThreadConflict>,
    pub missing_thread_ids: Vec<CodexManagedThreadId>,
    pub unrelated_thread_count: usize,
    pub page_count: usize,
}

struct StagedExecutable {
    directory: PathBuf,
    path: PathBuf,
}

impl Drop for StagedExecutable {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
        let _ = fs::remove_dir(&self.directory);
    }
}

pub struct CodexAppServer {
    child: Option<Child>,
    stdin: Option<ChildStdin>,
    inbound: Option<Receiver<InboundFrame>>,
    stdout_thread: Option<JoinHandle<()>>,
    stderr_thread: Option<JoinHandle<()>>,
    stderr_bytes: Arc<AtomicUsize>,
    stderr_overflowed: Arc<AtomicBool>,
    io_cancelled: Arc<AtomicBool>,
    retained_notification_bytes: Arc<AtomicUsize>,
    notifications: VecDeque<Vec<u8>>,
    next_request_id: u64,
    request_timeout: Duration,
    validated_profile: Option<ProviderFingerprint>,
    staged_executable: Option<StagedExecutable>,
}

impl CodexAppServer {
    pub fn connect_at(path: impl AsRef<Path>) -> Result<Self, CodexAppServerError> {
        let probe =
            probe_codex_compatibility_at(path).map_err(CodexAppServerError::CompatibilityProbe)?;
        connect_from_probe(probe)
    }

    pub fn connect_on_path(path_environment: Option<&OsStr>) -> Result<Self, CodexAppServerError> {
        let probe = probe_codex_compatibility_on_path(path_environment)
            .map_err(CodexAppServerError::CompatibilityProbe)?;
        connect_from_probe(probe)
    }

    pub fn start_read_only(
        &mut self,
        canonical_cwd: impl AsRef<Path>,
    ) -> Result<CodexStartedThread, CodexAppServerError> {
        let canonical_cwd = canonical_cwd.as_ref();
        let request_id = self.take_request_id()?;
        let request = codex_read_only_start_request(request_id, canonical_cwd)
            .map_err(CodexAppServerError::Contract)?;
        let response = self.exchange(request)?;
        self.decode_or_close(decode_codex_start_response(
            &response,
            request_id,
            canonical_cwd.to_owned(),
        ))
    }

    pub fn list_managed(
        &mut self,
        scope: &CodexManagedScope,
    ) -> Result<CodexManagedThreads, CodexAppServerError> {
        let mut cursor = None;
        let mut observed_cursors = BTreeSet::new();
        let mut matched = BTreeSet::new();
        let mut conflicts = Vec::new();
        let mut conflicted_ids = BTreeSet::new();
        let mut unrelated_thread_count = 0_usize;

        for page_index in 0..MAX_CODEX_LIST_PAGES {
            let request_id = self.take_request_id()?;
            let request =
                codex_thread_list_request(request_id, scope.canonical_cwd(), cursor.as_deref())
                    .map_err(CodexAppServerError::Contract)?;
            let response = self.exchange(request)?;
            let page = self.decode_or_close(decode_codex_thread_list_response(
                &response, request_id, scope,
            ))?;
            unrelated_thread_count =
                unrelated_thread_count.saturating_add(page.unrelated_thread_count);

            for thread_id in page.matched_thread_ids {
                if conflicted_ids.contains(&thread_id) || !matched.insert(thread_id) {
                    return self.close_with(CodexAppServerError::DuplicateManagedThread);
                }
            }
            for conflict in page.conflicting_threads {
                if matched.contains(&conflict.thread_id)
                    || !conflicted_ids.insert(conflict.thread_id.clone())
                {
                    return self.close_with(CodexAppServerError::DuplicateManagedThread);
                }
                conflicts.push(conflict);
            }

            let page_count = page_index + 1;
            let Some(next_cursor) = page.next_cursor else {
                let missing_thread_ids = scope
                    .exact_thread_ids()
                    .iter()
                    .filter(|thread_id| {
                        !matched.contains(*thread_id) && !conflicted_ids.contains(*thread_id)
                    })
                    .cloned()
                    .collect();
                return Ok(CodexManagedThreads {
                    matched_thread_ids: matched.into_iter().collect(),
                    conflicting_threads: conflicts,
                    missing_thread_ids,
                    unrelated_thread_count,
                    page_count,
                });
            };
            if !observed_cursors.insert(next_cursor.clone()) {
                return self.close_with(CodexAppServerError::PaginationCycle);
            }
            cursor = Some(next_cursor);
        }

        self.close_with(CodexAppServerError::PaginationLimit)
    }

    pub fn read_managed(
        &mut self,
        thread_id: &CodexManagedThreadId,
    ) -> Result<CodexThreadRead, CodexAppServerError> {
        let request_id = self.take_request_id()?;
        let request =
            codex_read_request(request_id, thread_id).map_err(CodexAppServerError::Contract)?;
        let response = self.exchange(request)?;
        self.decode_or_close(decode_codex_read_response(&response, request_id, thread_id))
    }

    pub fn pending_notification_count(&self) -> usize {
        self.notifications.len()
    }

    pub fn stderr_bytes(&self) -> usize {
        self.stderr_bytes
            .load(Ordering::Acquire)
            .min(MAX_CODEX_APP_SERVER_STDERR_BYTES)
    }

    pub fn validated_profile(&self) -> Option<&ProviderFingerprint> {
        self.validated_profile.as_ref()
    }

    pub fn shutdown(mut self) -> Result<(), CodexAppServerError> {
        self.terminate_owned()
    }

    fn spawn_and_handshake(
        executable: &Path,
        arguments: &[OsString],
        request_timeout: Duration,
    ) -> Result<Self, CodexAppServerError> {
        Self::spawn_and_handshake_with_reader_hook(
            executable,
            arguments,
            request_timeout,
            || Ok(()),
        )
    }

    fn spawn_and_handshake_with_reader_hook(
        executable: &Path,
        arguments: &[OsString],
        request_timeout: Duration,
        before_stderr_reader: impl FnOnce() -> io::Result<()>,
    ) -> Result<Self, CodexAppServerError> {
        let mut command = Command::new(executable);
        command
            .args(arguments)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0);
        let mut child = command.spawn().map_err(CodexAppServerError::Spawn)?;
        let Some(stdin) = child.stdin.take() else {
            let _ = terminate_process_group(&mut child);
            return Err(CodexAppServerError::MissingProcessPipe);
        };
        let Some(stdout) = child.stdout.take() else {
            let _ = terminate_process_group(&mut child);
            return Err(CodexAppServerError::MissingProcessPipe);
        };
        let Some(stderr) = child.stderr.take() else {
            let _ = terminate_process_group(&mut child);
            return Err(CodexAppServerError::MissingProcessPipe);
        };
        if set_nonblocking(&stdin).is_err()
            || set_nonblocking(&stdout).is_err()
            || set_nonblocking(&stderr).is_err()
        {
            let _ = terminate_process_group(&mut child);
            return Err(CodexAppServerError::ConfigureProcessPipe);
        }

        let (sender, receiver) = mpsc::sync_channel(INBOUND_FRAME_QUEUE_CAPACITY);
        let io_cancelled = Arc::new(AtomicBool::new(false));
        let retained_notification_bytes = Arc::new(AtomicUsize::new(0));
        let stdout_thread = thread::Builder::new()
            .name("flit-codex-stdout".to_owned())
            .spawn({
                let io_cancelled = Arc::clone(&io_cancelled);
                let retained_notification_bytes = Arc::clone(&retained_notification_bytes);
                move || {
                    read_stdout_frames(stdout, sender, &io_cancelled, &retained_notification_bytes);
                }
            })
            .map_err(|error| {
                let _ = terminate_process_group(&mut child);
                CodexAppServerError::SpawnReader(error)
            })?;
        let stderr_bytes = Arc::new(AtomicUsize::new(0));
        let stderr_overflowed = Arc::new(AtomicBool::new(false));
        let stderr_thread = {
            let stderr_bytes = Arc::clone(&stderr_bytes);
            let stderr_overflowed = Arc::clone(&stderr_overflowed);
            let stderr_io_cancelled = Arc::clone(&io_cancelled);
            match before_stderr_reader().and_then(|()| {
                thread::Builder::new()
                    .name("flit-codex-stderr".to_owned())
                    .spawn(move || {
                        drain_stderr(
                            stderr,
                            &stderr_bytes,
                            &stderr_overflowed,
                            &stderr_io_cancelled,
                        );
                    })
            }) {
                Ok(thread) => thread,
                Err(error) => {
                    io_cancelled.store(true, Ordering::Release);
                    drop(receiver);
                    let _ = terminate_process_group(&mut child);
                    let _ = stdout_thread.join();
                    return Err(CodexAppServerError::SpawnReader(error));
                }
            }
        };

        let mut server = Self {
            child: Some(child),
            stdin: Some(stdin),
            inbound: Some(receiver),
            stdout_thread: Some(stdout_thread),
            stderr_thread: Some(stderr_thread),
            stderr_bytes,
            stderr_overflowed,
            io_cancelled,
            retained_notification_bytes,
            notifications: VecDeque::new(),
            next_request_id: 1,
            request_timeout,
            validated_profile: None,
            staged_executable: None,
        };
        let request_id = server.take_request_id()?;
        let request =
            codex_initialize_request(request_id).map_err(CodexAppServerError::Contract)?;
        let response = server.exchange(request)?;
        server.decode_or_close(decode_codex_initialize_response(&response, request_id))?;
        server.write_frame(&codex_initialized_notification())?;
        Ok(server)
    }

    fn exchange(&mut self, frame: Vec<u8>) -> Result<Vec<u8>, CodexAppServerError> {
        let expected_request_id = frame_request_id(&frame)?;
        let deadline = Instant::now() + self.request_timeout;
        self.write_frame_until(&frame, deadline)?;

        loop {
            if self.stderr_overflowed.load(Ordering::Acquire) {
                return self.close_with(CodexAppServerError::StderrTooLarge);
            }
            let now = Instant::now();
            if now >= deadline {
                return self.close_with(CodexAppServerError::TimedOut);
            }
            let wait = deadline
                .saturating_duration_since(now)
                .min(RECEIVE_POLL_INTERVAL);
            let inbound = self
                .inbound
                .as_ref()
                .ok_or(CodexAppServerError::ConnectionClosed)?;
            match inbound.recv_timeout(wait) {
                Ok(InboundFrame::Frame(frame)) => {
                    let envelope = match classify_envelope(&frame) {
                        Ok(envelope) => envelope,
                        Err(error) => {
                            self.release_retained_bytes(frame.len());
                            return self.close_with(error);
                        }
                    };
                    match envelope {
                        EnvelopeKind::Response(request_id) if request_id == expected_request_id => {
                            self.release_retained_bytes(frame.len());
                            if self.stderr_overflowed.load(Ordering::Acquire) {
                                return self.close_with(CodexAppServerError::StderrTooLarge);
                            }
                            return Ok(frame);
                        }
                        EnvelopeKind::Response(_) => {
                            self.release_retained_bytes(frame.len());
                            return self.close_with(CodexAppServerError::UnexpectedResponseId);
                        }
                        EnvelopeKind::Notification => self.queue_notification(frame)?,
                    }
                }
                Ok(InboundFrame::FrameTooLarge) => {
                    return self.close_with(CodexAppServerError::StdoutFrameTooLarge);
                }
                Ok(InboundFrame::UnterminatedFrame) => {
                    return self.close_with(CodexAppServerError::UnterminatedStdoutFrame);
                }
                Ok(InboundFrame::ReadFailed) => {
                    return self.close_with(CodexAppServerError::ReadStdout);
                }
                Ok(InboundFrame::RetainedBytesExceeded) => {
                    return self.close_with(CodexAppServerError::NotificationBufferFull);
                }
                Ok(InboundFrame::Eof) | Err(RecvTimeoutError::Disconnected) => {
                    return self.close_with(CodexAppServerError::UnexpectedEof);
                }
                Err(RecvTimeoutError::Timeout) => {}
            }
        }
    }

    fn write_frame(&mut self, frame: &[u8]) -> Result<(), CodexAppServerError> {
        let deadline = Instant::now() + self.request_timeout;
        self.write_frame_until(frame, deadline)
    }

    fn write_frame_until(
        &mut self,
        frame: &[u8],
        deadline: Instant,
    ) -> Result<(), CodexAppServerError> {
        let mut written = 0_usize;
        while written < frame.len() {
            if self.stderr_overflowed.load(Ordering::Acquire) {
                return self.close_with(CodexAppServerError::StderrTooLarge);
            }
            if Instant::now() >= deadline {
                return self.close_with(CodexAppServerError::TimedOut);
            }
            let result = match self.stdin.as_mut() {
                Some(stdin) => stdin.write(&frame[written..]),
                None => return Err(CodexAppServerError::ConnectionClosed),
            };
            match result {
                Ok(0) => {
                    return self.close_with(CodexAppServerError::WriteStdin(
                        io::ErrorKind::WriteZero.into(),
                    ));
                }
                Ok(count) => written += count,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    thread::sleep(IO_POLL_INTERVAL);
                }
                Err(error) => {
                    return self.close_with(CodexAppServerError::WriteStdin(error));
                }
            }
        }
        Ok(())
    }

    fn queue_notification(&mut self, frame: Vec<u8>) -> Result<(), CodexAppServerError> {
        if self.notifications.len() >= MAX_CODEX_PENDING_NOTIFICATIONS {
            return self.close_with(CodexAppServerError::NotificationBufferFull);
        }
        self.notifications.push_back(frame);
        Ok(())
    }

    fn release_retained_bytes(&self, bytes: usize) {
        self.retained_notification_bytes
            .fetch_sub(bytes, Ordering::AcqRel);
    }

    fn take_request_id(&mut self) -> Result<u64, CodexAppServerError> {
        let request_id = self.next_request_id;
        self.next_request_id = self
            .next_request_id
            .checked_add(1)
            .ok_or(CodexAppServerError::RequestIdExhausted)?;
        Ok(request_id)
    }

    fn decode_or_close<T>(
        &mut self,
        result: Result<T, CodexContractError>,
    ) -> Result<T, CodexAppServerError> {
        match result {
            Ok(value) => Ok(value),
            Err(error) => self.close_with(CodexAppServerError::Contract(error)),
        }
    }

    fn close_with<T>(&mut self, error: CodexAppServerError) -> Result<T, CodexAppServerError> {
        match self.terminate_owned() {
            Ok(()) => Err(error),
            Err(cleanup) => Err(CodexAppServerError::CleanupAfterFailure {
                operation: Box::new(error),
                cleanup: Box::new(cleanup),
            }),
        }
    }

    fn terminate_owned(&mut self) -> Result<(), CodexAppServerError> {
        self.io_cancelled.store(true, Ordering::Release);
        self.stdin.take();
        self.inbound.take();
        let termination = self
            .child
            .as_mut()
            .map(terminate_process_group)
            .transpose()
            .map_err(|_| CodexAppServerError::TerminateProcessGroup);
        self.child.take();
        let stdout_join = self
            .stdout_thread
            .take()
            .map(JoinHandle::join)
            .transpose()
            .map(|_| ())
            .map_err(|_| CodexAppServerError::ReaderPanicked);
        let stderr_join = self
            .stderr_thread
            .take()
            .map(JoinHandle::join)
            .transpose()
            .map(|_| ())
            .map_err(|_| CodexAppServerError::ReaderPanicked);
        self.staged_executable.take();
        termination.and(stdout_join).and(stderr_join)
    }
}

impl Drop for CodexAppServer {
    fn drop(&mut self) {
        let _ = self.terminate_owned();
    }
}

fn connect_from_probe(
    probe: CodexCompatibilityProbe,
) -> Result<CodexAppServer, CodexAppServerError> {
    ensure_launch_allowed(&probe)?;
    let Some(validated_profile) = probe.validated_profile.clone() else {
        return Err(CodexAppServerError::CapabilityUnavailable {
            compatibility: probe.capability_snapshot.compatibility,
            launch: probe.capability_snapshot.status(ProviderCapability::Launch),
        });
    };
    let reinspection = inspect_codex_at(&probe.runtime_fingerprint.canonical_executable)
        .map_err(CodexAppServerError::ExecutableReinspection)?;
    if reinspection.canonical_path != probe.runtime_fingerprint.canonical_executable
        || reinspection.sha256 != probe.runtime_fingerprint.executable_sha256
    {
        return Err(CodexAppServerError::ExecutableChanged);
    }
    let staged_executable = stage_validated_executable(&reinspection)?;
    let mut server = CodexAppServer::spawn_and_handshake(
        &staged_executable.path,
        &[
            OsString::from("app-server"),
            OsString::from("--listen"),
            OsString::from("stdio://"),
        ],
        CODEX_APP_SERVER_REQUEST_TIMEOUT,
    )?;
    server.validated_profile = Some(validated_profile);
    server.staged_executable = Some(staged_executable);
    Ok(server)
}

fn stage_validated_executable(
    inspection: &ExecutableInspection,
) -> Result<StagedExecutable, CodexAppServerError> {
    stage_validated_executable_with_hook(inspection, |_| Ok(()))
}

fn stage_validated_executable_with_hook(
    inspection: &ExecutableInspection,
    after_directory_created: impl FnOnce(&StagedExecutable) -> io::Result<()>,
) -> Result<StagedExecutable, CodexAppServerError> {
    let mut directory = None;
    for _ in 0..64 {
        let nonce = NEXT_STAGED_EXECUTABLE.fetch_add(1, Ordering::Relaxed);
        let candidate = std::env::temp_dir().join(format!(
            "flit-codex-executable-{}-{nonce}",
            std::process::id()
        ));
        let mut builder = fs::DirBuilder::new();
        builder.mode(0o700);
        match builder.create(&candidate) {
            Ok(()) => {
                directory = Some(candidate);
                break;
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(CodexAppServerError::StageExecutable(error)),
        }
    }
    let Some(directory) = directory else {
        return Err(CodexAppServerError::StageDirectoryExhausted);
    };
    let staged = StagedExecutable {
        path: directory.join("codex"),
        directory,
    };
    fs::set_permissions(&staged.directory, fs::Permissions::from_mode(0o700))
        .map_err(CodexAppServerError::StageExecutable)?;
    after_directory_created(&staged).map_err(CodexAppServerError::StageExecutable)?;
    let mut source =
        fs::File::open(&inspection.canonical_path).map_err(CodexAppServerError::StageExecutable)?;
    let mut destination = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o500)
        .open(&staged.path)
        .map_err(CodexAppServerError::StageExecutable)?;
    io::copy(&mut source, &mut destination).map_err(CodexAppServerError::StageExecutable)?;
    destination
        .sync_all()
        .map_err(CodexAppServerError::StageExecutable)?;
    drop(destination);
    fs::set_permissions(&staged.path, fs::Permissions::from_mode(0o500))
        .map_err(CodexAppServerError::StageExecutable)?;

    let staged_inspection =
        inspect_codex_at(&staged.path).map_err(CodexAppServerError::StagedExecutableInspection)?;
    let source_after = inspect_codex_at(&inspection.canonical_path)
        .map_err(CodexAppServerError::ExecutableReinspection)?;
    if staged_inspection.sha256 != inspection.sha256
        || source_after.canonical_path != inspection.canonical_path
        || source_after.filesystem_id != inspection.filesystem_id
        || source_after.sha256 != inspection.sha256
    {
        return Err(CodexAppServerError::ExecutableChanged);
    }
    Ok(staged)
}

fn ensure_launch_allowed(probe: &CodexCompatibilityProbe) -> Result<(), CodexAppServerError> {
    let launch = probe.capability_snapshot.status(ProviderCapability::Launch);
    if probe.validated_profile.is_none()
        || probe.capability_snapshot.compatibility != ProviderCompatibility::Supported
        || !launch.is_available()
    {
        return Err(CodexAppServerError::CapabilityUnavailable {
            compatibility: probe.capability_snapshot.compatibility,
            launch,
        });
    }
    Ok(())
}

fn read_stdout_frames(
    mut stdout: impl Read,
    sender: SyncSender<InboundFrame>,
    cancelled: &AtomicBool,
    retained_bytes: &AtomicUsize,
) {
    let mut frame = Vec::new();
    let mut buffer = [0_u8; 8 * 1024];
    loop {
        if cancelled.load(Ordering::Acquire) {
            return;
        }
        match stdout.read(&mut buffer) {
            Ok(0) => {
                let event = if frame.is_empty() {
                    InboundFrame::Eof
                } else {
                    InboundFrame::UnterminatedFrame
                };
                let _ = sender.send(event);
                return;
            }
            Ok(read) => {
                for byte in &buffer[..read] {
                    if *byte == b'\n' {
                        let completed = std::mem::take(&mut frame);
                        if !reserve_retained_bytes(retained_bytes, completed.len()) {
                            let _ = sender.send(InboundFrame::RetainedBytesExceeded);
                            return;
                        }
                        let completed_len = completed.len();
                        if sender.send(InboundFrame::Frame(completed)).is_err() {
                            retained_bytes.fetch_sub(completed_len, Ordering::AcqRel);
                            return;
                        }
                    } else {
                        if frame.len() >= crate::MAX_CODEX_APP_SERVER_FRAME_BYTES {
                            let _ = sender.send(InboundFrame::FrameTooLarge);
                            return;
                        }
                        frame.push(*byte);
                    }
                }
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(IO_POLL_INTERVAL);
            }
            Err(_) => {
                let _ = sender.send(InboundFrame::ReadFailed);
                return;
            }
        }
    }
}

fn reserve_retained_bytes(retained_bytes: &AtomicUsize, bytes: usize) -> bool {
    retained_bytes
        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
            let next = current.checked_add(bytes)?;
            (next <= MAX_CODEX_PENDING_NOTIFICATION_BYTES).then_some(next)
        })
        .is_ok()
}

fn drain_stderr(
    mut stderr: impl Read,
    total_bytes: &AtomicUsize,
    overflowed: &AtomicBool,
    cancelled: &AtomicBool,
) {
    let mut buffer = [0_u8; 8 * 1024];
    loop {
        if cancelled.load(Ordering::Acquire) {
            return;
        }
        match stderr.read(&mut buffer) {
            Ok(0) => return,
            Ok(read) => {
                let previous = total_bytes.fetch_add(read, Ordering::AcqRel);
                if previous.saturating_add(read) > MAX_CODEX_APP_SERVER_STDERR_BYTES {
                    overflowed.store(true, Ordering::Release);
                }
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(IO_POLL_INTERVAL);
            }
            Err(_) => return,
        }
    }
}

fn frame_request_id(frame: &[u8]) -> Result<u64, CodexAppServerError> {
    let value: Value =
        serde_json::from_slice(frame).map_err(|_| CodexAppServerError::InvalidEnvelope)?;
    value
        .get("id")
        .and_then(Value::as_u64)
        .ok_or(CodexAppServerError::InvalidEnvelope)
}

fn classify_envelope(frame: &[u8]) -> Result<EnvelopeKind, CodexAppServerError> {
    let value: Value =
        serde_json::from_slice(frame).map_err(|_| CodexAppServerError::InvalidEnvelope)?;
    let object = value
        .as_object()
        .ok_or(CodexAppServerError::InvalidEnvelope)?;
    match (
        object.get("id").and_then(Value::as_u64),
        object.contains_key("result") || object.contains_key("error"),
        object.get("method").and_then(Value::as_str),
    ) {
        (Some(request_id), true, _) => Ok(EnvelopeKind::Response(request_id)),
        (_, false, Some(_)) => Ok(EnvelopeKind::Notification),
        _ => Err(CodexAppServerError::InvalidEnvelope),
    }
}

enum EnvelopeKind {
    Response(u64),
    Notification,
}

enum InboundFrame {
    Frame(Vec<u8>),
    FrameTooLarge,
    RetainedBytesExceeded,
    UnterminatedFrame,
    ReadFailed,
    Eof,
}

#[derive(Debug)]
pub enum CodexAppServerError {
    CompatibilityProbe(CodexCompatibilityProbeError),
    ExecutableReinspection(ExecutableInspectionError),
    CapabilityUnavailable {
        compatibility: ProviderCompatibility,
        launch: CapabilityStatus,
    },
    ExecutableChanged,
    StageExecutable(io::Error),
    StagedExecutableInspection(ExecutableInspectionError),
    StageDirectoryExhausted,
    Spawn(io::Error),
    SpawnReader(io::Error),
    MissingProcessPipe,
    ConfigureProcessPipe,
    WriteStdin(io::Error),
    TimedOut,
    UnexpectedEof,
    ReadStdout,
    StdoutFrameTooLarge,
    UnterminatedStdoutFrame,
    StderrTooLarge,
    InvalidEnvelope,
    UnexpectedResponseId,
    NotificationBufferFull,
    PaginationCycle,
    PaginationLimit,
    DuplicateManagedThread,
    RequestIdExhausted,
    ConnectionClosed,
    Contract(CodexContractError),
    TerminateProcessGroup,
    ReaderPanicked,
    CleanupAfterFailure {
        operation: Box<CodexAppServerError>,
        cleanup: Box<CodexAppServerError>,
    },
}

impl fmt::Display for CodexAppServerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CompatibilityProbe(error) => write!(formatter, "Codex probe failed: {error}"),
            Self::ExecutableReinspection(error) => {
                write!(formatter, "Codex executable reinspection failed: {error}")
            }
            Self::CapabilityUnavailable {
                compatibility,
                launch,
            } => write!(
                formatter,
                "Codex launch is unavailable for compatibility {compatibility:?} and capability {launch:?}"
            ),
            Self::ExecutableChanged => {
                formatter.write_str("Codex executable changed after compatibility probing")
            }
            Self::StageExecutable(error) => {
                write!(
                    formatter,
                    "could not stage the validated Codex executable: {error}"
                )
            }
            Self::StagedExecutableInspection(error) => {
                write!(
                    formatter,
                    "staged Codex executable inspection failed: {error}"
                )
            }
            Self::StageDirectoryExhausted => {
                formatter.write_str("could not reserve a private Codex staging directory")
            }
            Self::Spawn(error) => write!(formatter, "could not start Codex app-server: {error}"),
            Self::SpawnReader(error) => {
                write!(formatter, "could not start a Codex output reader: {error}")
            }
            Self::MissingProcessPipe => {
                formatter.write_str("Codex app-server did not expose required stdio pipes")
            }
            Self::ConfigureProcessPipe => {
                formatter.write_str("could not configure bounded Codex stdio pipes")
            }
            Self::WriteStdin(error) => write!(formatter, "could not write to Codex: {error}"),
            Self::TimedOut => formatter.write_str("Codex app-server request timed out"),
            Self::UnexpectedEof => formatter.write_str("Codex app-server exited before responding"),
            Self::ReadStdout => formatter.write_str("could not read Codex app-server stdout"),
            Self::StdoutFrameTooLarge => {
                formatter.write_str("Codex app-server stdout frame exceeded its limit")
            }
            Self::UnterminatedStdoutFrame => {
                formatter.write_str("Codex app-server stdout ended inside a frame")
            }
            Self::StderrTooLarge => {
                formatter.write_str("Codex app-server stderr exceeded its limit")
            }
            Self::InvalidEnvelope => formatter.write_str("invalid Codex app-server envelope"),
            Self::UnexpectedResponseId => {
                formatter.write_str("Codex app-server returned an unexpected response ID")
            }
            Self::NotificationBufferFull => {
                formatter.write_str("Codex app-server notification buffer is full")
            }
            Self::PaginationCycle => formatter.write_str("Codex list cursor repeated"),
            Self::PaginationLimit => formatter.write_str("Codex list exceeded its page limit"),
            Self::DuplicateManagedThread => {
                formatter.write_str("Codex repeated an exact managed thread across pages")
            }
            Self::RequestIdExhausted => formatter.write_str("Codex request IDs are exhausted"),
            Self::ConnectionClosed => formatter.write_str("Codex app-server connection is closed"),
            Self::Contract(error) => write!(formatter, "Codex contract failed: {error}"),
            Self::TerminateProcessGroup => {
                formatter.write_str("could not terminate the Codex app-server process group")
            }
            Self::ReaderPanicked => formatter.write_str("a Codex output reader panicked"),
            Self::CleanupAfterFailure { operation, cleanup } => {
                write!(formatter, "{operation}; cleanup also failed: {cleanup}")
            }
        }
    }
}

impl Error for CodexAppServerError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::CompatibilityProbe(error) => Some(error),
            Self::ExecutableReinspection(error) => Some(error),
            Self::StagedExecutableInspection(error) => Some(error),
            Self::StageExecutable(error)
            | Self::Spawn(error)
            | Self::SpawnReader(error)
            | Self::WriteStdin(error) => Some(error),
            Self::Contract(error) => Some(error),
            Self::CleanupAfterFailure { operation, .. } => Some(operation),
            _ => None,
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::{
        fs,
        os::unix::fs::{PermissionsExt, symlink},
        path::PathBuf,
        process,
        sync::atomic::{AtomicU64, Ordering},
        thread,
        time::Instant,
    };

    use crate::{CapabilityEntry, ProviderCapabilitySnapshot, ProviderFingerprint, ProviderKind};
    use rustix::{
        fs::Mode,
        process::{
            Pid, Signal, kill_process, kill_process_group, test_kill_process,
            test_kill_process_group, umask,
        },
    };

    use super::*;

    static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(1);
    const TEST_CWD: &str = "/private/tmp/flit-provider-transport-project";

    struct TestDirectory(PathBuf);

    struct UmaskGuard(Mode);

    impl TestDirectory {
        fn new(label: &str) -> Self {
            let nonce = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "flit-codex-transport-{label}-{}-{nonce}",
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

    impl UmaskGuard {
        fn set(mask: Mode) -> Self {
            Self(umask(mask))
        }
    }

    impl Drop for UmaskGuard {
        fn drop(&mut self) {
            umask(self.0);
        }
    }

    #[test]
    fn fake_transport_handshakes_starts_lists_and_reads_exact_scope() {
        let _process_guard = test_process_guard();
        let directory = TestDirectory::new("success");
        let executable = directory.0.join("codex");
        write_script(
            &executable,
            &format!(
                r#"#!/bin/sh
printf '%s' "$$" > "$0.pgid"
while IFS= read -r line; do
  case "$line" in
    *'"method":"initialize"'*) printf '%s\n' '{{"id":1,"result":{{}}}}' ;;
    *'"method":"thread/start"'*) printf '%s\n' '{{"id":2,"result":{{"thread":{{"id":"managed-1","sessionId":"managed-1","source":"other"}},"sandbox":{{"type":"readOnly","networkAccess":false}},"approvalPolicy":"never"}}}}' ;;
    *'"method":"thread/list"'*) printf '%s\n' '{{"id":3,"result":{{"data":[{{"id":"managed-1","sessionId":"managed-1","cwd":"{TEST_CWD}","source":"other"}},{{"id":"unrelated","sessionId":"unrelated","cwd":"{TEST_CWD}","source":"flit"}}],"nextCursor":null}}}}' ;;
    *'"method":"thread/read"'*) printf '%s\n' '{{"id":4,"result":{{"thread":{{"id":"managed-1","sessionId":"managed-1","turns":[{{"id":"turn-1","status":"completed"}}]}}}}}}' ;;
  esac
done
"#
            ),
        );
        let mut server = connect_fake(&executable, Duration::from_secs(1)).expect("fake handshake");
        let started = server.start_read_only(TEST_CWD).expect("fake start");
        let scope =
            CodexManagedScope::new(TEST_CWD, [started.thread_id.clone(), thread_id("missing")])
                .expect("scope");
        let listed = server.list_managed(&scope).expect("fake list");
        assert_eq!(listed.matched_thread_ids, [thread_id("managed-1")]);
        assert_eq!(listed.missing_thread_ids, [thread_id("missing")]);
        assert_eq!(listed.unrelated_thread_count, 1);
        assert_eq!(listed.page_count, 1);
        assert!(listed.conflicting_threads.is_empty());
        let read = server.read_managed(&started.thread_id).expect("fake read");
        assert_eq!(read.state, crate::CodexThreadState::Completed);
        assert_eq!(server.pending_notification_count(), 0);
        assert_eq!(server.stderr_bytes(), 0);
        assert!(server.validated_profile().is_none());
        server.shutdown().expect("clean shutdown");
        assert_process_group_gone(&executable);
    }

    #[test]
    fn staged_executable_identity_survives_source_path_replacement() {
        let _process_guard = test_process_guard();
        let directory = TestDirectory::new("staged-identity");
        let executable = directory.0.join("codex");
        let replacement_marker = directory.0.join("replacement-ran");
        write_script(
            &executable,
            r#"#!/bin/sh
while IFS= read -r line; do
  case "$line" in
    *'"method":"initialize"'*) printf '%s\n' '{"id":1,"result":{}}' ;;
  esac
done
"#,
        );
        let inspection = inspect_codex_at(&executable).expect("original inspection");
        let staged = stage_validated_executable(&inspection).expect("staged executable");
        let staged_path = staged.path.clone();
        write_script(
            &executable,
            &format!(
                "#!/bin/sh\nprintf replaced > '{}'\nexit 9\n",
                replacement_marker.display()
            ),
        );

        let mut server = CodexAppServer::spawn_and_handshake(
            &staged.path,
            &[
                OsString::from("app-server"),
                OsString::from("--listen"),
                OsString::from("stdio://"),
            ],
            Duration::from_secs(1),
        )
        .expect("staged original must handshake");
        server.staged_executable = Some(staged);
        assert!(!replacement_marker.exists());
        server.shutdown().expect("staged shutdown");
        assert!(!staged_path.exists());
        assert!(!replacement_marker.exists());
    }

    #[test]
    fn staging_is_private_and_never_follows_a_competing_symlink() {
        let _process_guard = test_process_guard();
        let directory = TestDirectory::new("private-staging");
        let executable = directory.0.join("codex");
        let external_target = directory.0.join("external-target");
        write_script(&executable, "#!/bin/sh\nexit 0\n");
        fs::write(&external_target, b"unchanged").expect("external target");
        let inspection = inspect_codex_at(&executable).expect("source inspection");
        let _umask_guard = UmaskGuard::set(Mode::empty());

        let error = match stage_validated_executable_with_hook(&inspection, |staged| {
            let directory_mode = fs::metadata(&staged.directory)?.permissions().mode() & 0o777;
            assert_eq!(directory_mode, 0o700);
            symlink(&external_target, &staged.path)
        }) {
            Ok(_) => panic!("competing symlink unexpectedly accepted"),
            Err(error) => error,
        };

        assert!(matches!(
            error,
            CodexAppServerError::StageExecutable(ref source)
                if source.kind() == io::ErrorKind::AlreadyExists
        ));
        assert_eq!(
            fs::read(&external_target).expect("unchanged external target"),
            b"unchanged"
        );
    }

    #[test]
    fn nonblocking_stdin_write_obeys_the_connection_deadline() {
        let _process_guard = test_process_guard();
        let directory = TestDirectory::new("stdin-backpressure");
        let executable = directory.0.join("codex");
        write_script(
            &executable,
            r#"#!/bin/sh
printf '%s' "$$" > "$0.pgid"
IFS= read -r line
printf '%s\n' '{"id":1,"result":{}}'
IFS= read -r line
/bin/sleep 30 & wait
"#,
        );
        let mut server =
            connect_fake(&executable, Duration::from_secs(1)).expect("backpressure handshake");
        server.request_timeout = Duration::from_millis(200);
        let large_frame = vec![b'x'; crate::MAX_CODEX_APP_SERVER_FRAME_BYTES];
        let started = Instant::now();
        let mut observed_timeout = false;
        for _ in 0..16 {
            match server.write_frame(&large_frame) {
                Ok(()) => {}
                Err(error) => {
                    assert!(ErrorKind::TimedOut.matches(&error), "{error:?}");
                    observed_timeout = true;
                    break;
                }
            }
        }
        assert!(
            observed_timeout,
            "non-reading stdin never applied backpressure"
        );
        assert!(started.elapsed() < Duration::from_secs(2));
        assert_process_group_gone(&executable);
    }

    #[test]
    fn cancelled_readers_make_shutdown_bounded_when_an_escaped_child_holds_pipes() {
        let _process_guard = test_process_guard();
        let directory = TestDirectory::new("escaped-pipe-holder");
        let executable = directory.0.join("codex");
        let escaped_pid_path = directory.0.join("escaped.pid");
        write_script(
            &executable,
            &format!(
                r#"#!/bin/sh
printf '%s' "$$" > "$0.pgid"
set -m
/bin/sleep 30 &
printf '%s' "$!" > '{}'
while IFS= read -r line; do
  case "$line" in
    *'"method":"initialize"'*) printf '%s\n' '{{"id":1,"result":{{}}}}' ;;
  esac
done
"#,
                escaped_pid_path.display()
            ),
        );
        let server =
            connect_fake(&executable, Duration::from_secs(1)).expect("escaped child handshake");
        let escaped_pid = read_pid(&escaped_pid_path);
        assert!(test_kill_process(escaped_pid).is_ok());
        let started = Instant::now();
        server.shutdown().expect("bounded shutdown");
        assert!(started.elapsed() < Duration::from_secs(1));
        assert_process_group_gone(&executable);

        let _ = kill_process_group(escaped_pid, Signal::KILL);
        let _ = kill_process(escaped_pid, Signal::KILL);
    }

    #[test]
    fn stderr_reader_spawn_failure_cancels_stdout_before_joining() {
        let _process_guard = test_process_guard();
        let directory = TestDirectory::new("stderr-reader-spawn-failure");
        let executable = directory.0.join("codex");
        let escaped_pid_path = directory.0.join("escaped.pid");
        write_script(
            &executable,
            &format!(
                r#"#!/bin/sh
printf '%s' "$$" > "$0.pgid"
set -m
/bin/sleep 30 &
printf '%s' "$!" > '{}'
while IFS= read -r line; do :; done
"#,
                escaped_pid_path.display()
            ),
        );

        let started = Instant::now();
        let error = match CodexAppServer::spawn_and_handshake_with_reader_hook(
            &executable,
            &[
                OsString::from("app-server"),
                OsString::from("--listen"),
                OsString::from("stdio://"),
            ],
            Duration::from_secs(1),
            || {
                let deadline = Instant::now() + Duration::from_secs(1);
                while !escaped_pid_path.exists() {
                    if Instant::now() >= deadline {
                        return Err(io::Error::new(
                            io::ErrorKind::TimedOut,
                            "escaped child did not start",
                        ));
                    }
                    thread::sleep(Duration::from_millis(10));
                }
                Err(io::Error::other("injected stderr reader spawn failure"))
            },
        ) {
            Ok(_) => panic!("injected reader failure unexpectedly connected"),
            Err(error) => error,
        };
        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(matches!(error, CodexAppServerError::SpawnReader(_)));
        assert_process_group_gone(&executable);

        let escaped_pid = read_pid(&escaped_pid_path);
        let _ = kill_process_group(escaped_pid, Signal::KILL);
        let _ = kill_process(escaped_pid, Signal::KILL);
    }

    #[test]
    fn managed_list_paginates_with_bounded_cursor_and_duplicate_protection() {
        let _process_guard = test_process_guard();
        let directory = TestDirectory::new("pagination");
        let executable = directory.0.join("codex");
        write_script(
            &executable,
            &format!(
                r#"#!/bin/sh
printf '%s' "$$" > "$0.pgid"
while IFS= read -r line; do
  case "$line" in
    *'"method":"initialize"'*) printf '%s\n' '{{"id":1,"result":{{}}}}' ;;
    *'"cursor":"next"'*) printf '%s\n' '{{"id":3,"result":{{"data":[{{"id":"managed-2","sessionId":"managed-2","cwd":"{TEST_CWD}"}}],"nextCursor":null}}}}' ;;
    *'"method":"thread/list"'*) printf '%s\n' '{{"id":2,"result":{{"data":[{{"id":"managed-1","sessionId":"managed-1","cwd":"{TEST_CWD}"}}],"nextCursor":"next"}}}}' ;;
  esac
done
"#
            ),
        );
        let mut server =
            connect_fake(&executable, Duration::from_secs(1)).expect("pagination handshake");
        let scope = CodexManagedScope::new(
            TEST_CWD,
            [
                thread_id("managed-1"),
                thread_id("managed-2"),
                thread_id("missing"),
            ],
        )
        .expect("pagination scope");
        let listed = server.list_managed(&scope).expect("two list pages");
        assert_eq!(
            listed.matched_thread_ids,
            [thread_id("managed-1"), thread_id("managed-2")]
        );
        assert_eq!(listed.missing_thread_ids, [thread_id("missing")]);
        assert_eq!(listed.page_count, 2);
        server.shutdown().expect("pagination shutdown");
        assert_process_group_gone(&executable);

        let duplicate_directory = TestDirectory::new("pagination-duplicate");
        let duplicate_executable = duplicate_directory.0.join("codex");
        write_script(
            &duplicate_executable,
            &format!(
                r#"#!/bin/sh
printf '%s' "$$" > "$0.pgid"
while IFS= read -r line; do
  case "$line" in
    *'"method":"initialize"'*) printf '%s\n' '{{"id":1,"result":{{}}}}' ;;
    *'"cursor":"next"'*) printf '%s\n' '{{"id":3,"result":{{"data":[{{"id":"managed-1","sessionId":"managed-1","cwd":"{TEST_CWD}"}}],"nextCursor":null}}}}' ;;
    *'"method":"thread/list"'*) printf '%s\n' '{{"id":2,"result":{{"data":[{{"id":"managed-1","sessionId":"managed-1","cwd":"{TEST_CWD}"}}],"nextCursor":"next"}}}}' ;;
  esac
done
"#
            ),
        );
        let mut duplicate_server = connect_fake(&duplicate_executable, Duration::from_secs(1))
            .expect("duplicate pagination handshake");
        let duplicate_scope =
            CodexManagedScope::new(TEST_CWD, [thread_id("managed-1")]).expect("duplicate scope");
        assert!(matches!(
            duplicate_server.list_managed(&duplicate_scope),
            Err(CodexAppServerError::DuplicateManagedThread)
        ));
        assert_process_group_gone(&duplicate_executable);
    }

    #[test]
    fn timeout_and_oversized_stdout_terminate_descendants() {
        let _process_guard = test_process_guard();
        for (label, behavior, timeout, expected) in [
            (
                "timeout",
                "while IFS= read -r line; do /bin/sleep 30 & wait; done",
                Duration::from_millis(500),
                ErrorKind::TimedOut,
            ),
            (
                "overflow",
                "while IFS= read -r line; do /bin/sleep 30 & /bin/dd if=/dev/zero bs=262145 count=1 2>/dev/null; printf '\\n'; wait; done",
                Duration::from_secs(1),
                ErrorKind::StdoutTooLarge,
            ),
        ] {
            let directory = TestDirectory::new(label);
            let executable = directory.0.join("codex");
            write_script(
                &executable,
                &format!("#!/bin/sh\nprintf '%s' \"$$\" > \"$0.pgid\"\n{behavior}\n"),
            );
            let error = match connect_fake(&executable, timeout) {
                Ok(_) => panic!("fake failure unexpectedly connected"),
                Err(error) => error,
            };
            assert!(expected.matches(&error), "{error:?}");
            assert_process_group_gone(&executable);
        }
    }

    #[test]
    fn malformed_wrong_id_stderr_overflow_and_early_eof_fail_closed() {
        let _process_guard = test_process_guard();
        for (label, response, expected) in [
            ("malformed", "printf '{\\n'", ErrorKind::InvalidEnvelope),
            (
                "wrong-id",
                "printf '%s\\n' '{\"id\":99,\"result\":{}}'",
                ErrorKind::UnexpectedResponseId,
            ),
            (
                "stderr-overflow",
                "/bin/dd if=/dev/zero bs=65537 count=1 >&2 2>/dev/null; /bin/sleep 1",
                ErrorKind::StderrTooLarge,
            ),
            ("early-eof", "exit 0", ErrorKind::UnexpectedEof),
            (
                "unterminated",
                "printf '%s' '{\"id\":1,\"result\":{}}'; exit 0",
                ErrorKind::UnterminatedFrame,
            ),
        ] {
            let directory = TestDirectory::new(label);
            let executable = directory.0.join("codex");
            write_script(
                &executable,
                &format!(
                    "#!/bin/sh\nprintf '%s' \"$$\" > \"$0.pgid\"\nIFS= read -r line\n{response}\n"
                ),
            );
            let error = match connect_fake(&executable, Duration::from_secs(1)) {
                Ok(_) => panic!("{label} unexpectedly connected"),
                Err(error) => error,
            };
            assert!(expected.matches(&error), "{label}: {error:?}");
            assert_process_group_gone(&executable);
        }
    }

    #[test]
    fn notification_flood_and_drop_are_bounded_and_clean_the_process_group() {
        let _process_guard = test_process_guard();
        let flood_directory = TestDirectory::new("notification-flood");
        let flood_executable = flood_directory.0.join("codex");
        write_script(
            &flood_executable,
            r#"#!/bin/sh
printf '%s' "$$" > "$0.pgid"
IFS= read -r line
i=0
while [ "$i" -le 64 ]; do
  printf '%s\n' '{"method":"turn/completed","params":{}}'
  i=$((i + 1))
done
printf '%s\n' '{"id":1,"result":{}}'
"#,
        );
        let error = match connect_fake(&flood_executable, Duration::from_secs(1)) {
            Ok(_) => panic!("notification flood unexpectedly connected"),
            Err(error) => error,
        };
        assert!(
            ErrorKind::NotificationBufferFull.matches(&error),
            "{error:?}"
        );
        assert_process_group_gone(&flood_executable);

        let drop_directory = TestDirectory::new("drop");
        let drop_executable = drop_directory.0.join("codex");
        write_script(
            &drop_executable,
            r#"#!/bin/sh
printf '%s' "$$" > "$0.pgid"
while IFS= read -r line; do
  case "$line" in
    *'"method":"initialize"'*) printf '%s\n' '{"id":1,"result":{}}' ;;
  esac
done
"#,
        );
        let server =
            connect_fake(&drop_executable, Duration::from_secs(1)).expect("drop fixture handshake");
        drop(server);
        assert_process_group_gone(&drop_executable);
    }

    #[test]
    fn inbound_and_queued_notifications_share_one_byte_budget() {
        let _process_guard = test_process_guard();
        let directory = TestDirectory::new("notification-byte-budget");
        let executable = directory.0.join("codex");
        let payload = directory.0.join("notifications.jsonl");
        let padding = "a".repeat(240 * 1024);
        let mut frames = Vec::new();
        for _ in 0..5 {
            frames.extend_from_slice(
                format!(
                    "{{\"method\":\"item/started\",\"params\":{{\"padding\":\"{padding}\"}}}}\n"
                )
                .as_bytes(),
            );
        }
        fs::write(&payload, frames).expect("notification payload");
        write_script(
            &executable,
            &format!(
                r#"#!/bin/sh
printf '%s' "$$" > "$0.pgid"
IFS= read -r line
/bin/cat '{}'
printf '%s\n' '{{"id":1,"result":{{}}}}'
"#,
                payload.display()
            ),
        );
        let error = match connect_fake(&executable, Duration::from_secs(1)) {
            Ok(_) => panic!("notification byte flood unexpectedly connected"),
            Err(error) => error,
        };
        assert!(
            ErrorKind::NotificationBufferFull.matches(&error),
            "{error:?}"
        );
        assert_process_group_gone(&executable);
    }

    #[test]
    fn unknown_capability_is_rejected_before_spawn() {
        let _process_guard = test_process_guard();
        let fingerprint = ProviderFingerprint {
            canonical_executable: PathBuf::from("/untrusted/codex"),
            executable_version: "9.9.9".to_owned(),
            executable_sha256: "unknown".to_owned(),
            combined_schema_sha256: "unknown".to_owned(),
            v2_schema_sha256: "unknown".to_owned(),
            method_allowlist_sha256: String::new(),
            fixture_sha256: String::new(),
            smoke_run_id: String::new(),
        };
        let probe = CodexCompatibilityProbe {
            runtime_fingerprint: crate::CodexRuntimeFingerprint {
                canonical_executable: fingerprint.canonical_executable.clone(),
                executable_version: fingerprint.executable_version.clone(),
                executable_sha256: fingerprint.executable_sha256.clone(),
                combined_schema_sha256: fingerprint.combined_schema_sha256.clone(),
                v2_schema_sha256: fingerprint.v2_schema_sha256.clone(),
            },
            validated_profile: None,
            capability_snapshot: ProviderCapabilitySnapshot {
                provider: ProviderKind::Codex,
                compatibility: ProviderCompatibility::Unknown,
                capabilities: ProviderCapability::ALL
                    .map(|capability| CapabilityEntry {
                        capability,
                        status: CapabilityStatus::Unknown,
                    })
                    .to_vec(),
                fingerprint_mismatches: Vec::new(),
            },
            version_stderr_bytes: 0,
            schema_stdout_bytes: 0,
            schema_stderr_bytes: 0,
        };
        assert!(matches!(
            ensure_launch_allowed(&probe),
            Err(CodexAppServerError::CapabilityUnavailable { .. })
        ));
    }

    #[test]
    fn public_unknown_probe_never_invokes_app_server_mode() {
        let directory = TestDirectory::new("public-unknown");
        let executable = directory.0.join("codex");
        let app_server_marker = directory.0.join("app-server-started");
        write_script(
            &executable,
            &format!(
                r#"#!/bin/sh
if [ "$1" = "--version" ]; then
  printf '%s\n' 'codex-cli 9.9.9'
  exit 0
fi
if [ "$1" = "app-server" ] && [ "$2" = "generate-json-schema" ]; then
  printf '{{}}' > "$5/codex_app_server_protocol.schemas.json"
  printf '{{}}' > "$5/codex_app_server_protocol.v2.schemas.json"
  exit 0
fi
printf started > '{}'
exit 9
"#,
                app_server_marker.display()
            ),
        );
        assert!(matches!(
            CodexAppServer::connect_at(&executable),
            Err(CodexAppServerError::CapabilityUnavailable { .. })
        ));
        assert!(!app_server_marker.exists());
    }

    fn connect_fake(
        executable: &Path,
        timeout: Duration,
    ) -> Result<CodexAppServer, CodexAppServerError> {
        CodexAppServer::spawn_and_handshake(
            executable,
            &[
                OsString::from("app-server"),
                OsString::from("--listen"),
                OsString::from("stdio://"),
            ],
            timeout,
        )
    }

    fn test_process_guard() -> std::sync::MutexGuard<'static, ()> {
        crate::process::TEST_PROCESS_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn write_script(path: &Path, body: &str) {
        fs::write(path, body).expect("write fake Codex");
        let mut permissions = fs::metadata(path).expect("fake metadata").permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(path, permissions).expect("fake permissions");
    }

    fn assert_process_group_gone(executable: &Path) {
        let pid_text =
            fs::read_to_string(format!("{}.pgid", executable.display())).unwrap_or_else(|error| {
                panic!(
                    "fake process group ID for {}: {error}",
                    executable.display()
                )
            });
        let pid = parsed_pid(&pid_text);
        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            match test_kill_process_group(pid) {
                Err(error) if error == rustix::io::Errno::SRCH => return,
                _ if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
                result => panic!("process group {pid_text} survived cleanup: {result:?}"),
            }
        }
    }

    fn read_pid(path: &Path) -> Pid {
        parsed_pid(
            &fs::read_to_string(path)
                .unwrap_or_else(|error| panic!("process ID at {}: {error}", path.display())),
        )
    }

    fn parsed_pid(value: &str) -> Pid {
        Pid::from_raw(value.parse::<i32>().expect("numeric process ID"))
            .expect("positive process ID")
    }

    fn thread_id(value: &str) -> CodexManagedThreadId {
        CodexManagedThreadId::new(value).expect("valid test thread ID")
    }

    enum ErrorKind {
        TimedOut,
        StdoutTooLarge,
        InvalidEnvelope,
        UnexpectedResponseId,
        StderrTooLarge,
        UnexpectedEof,
        UnterminatedFrame,
        NotificationBufferFull,
    }

    impl ErrorKind {
        fn matches(&self, error: &CodexAppServerError) -> bool {
            match (self, error) {
                (Self::TimedOut, CodexAppServerError::TimedOut)
                | (Self::StdoutTooLarge, CodexAppServerError::StdoutFrameTooLarge)
                | (Self::InvalidEnvelope, CodexAppServerError::InvalidEnvelope)
                | (Self::UnexpectedResponseId, CodexAppServerError::UnexpectedResponseId)
                | (Self::StderrTooLarge, CodexAppServerError::StderrTooLarge)
                | (Self::UnexpectedEof, CodexAppServerError::UnexpectedEof)
                | (Self::UnterminatedFrame, CodexAppServerError::UnterminatedStdoutFrame)
                | (Self::NotificationBufferFull, CodexAppServerError::NotificationBufferFull) => {
                    true
                }
                (_, CodexAppServerError::CleanupAfterFailure { operation, .. }) => {
                    self.matches(operation)
                }
                _ => false,
            }
        }
    }
}
