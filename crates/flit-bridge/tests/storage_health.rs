use std::{
    env,
    fs::{self, File, OpenOptions},
    path::{Path, PathBuf},
    process::{self, Command},
    sync::atomic::{AtomicU64, Ordering},
};

use flit_bridge::{BridgeError, core_construction_count, initialize_core, system_health_json};
use flit_protocol::{HealthStatus, PROTOCOL_VERSION, SystemHealthResponse};

#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, PermissionsExt};

static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(1);

fn test_directory(label: &str) -> PathBuf {
    let nonce = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("flit-bridge-{label}-{}-{nonce}", process::id()))
}

fn health() -> SystemHealthResponse {
    serde_json::from_str(&system_health_json(PROTOCOL_VERSION.to_owned()).expect("health response"))
        .expect("typed health response")
}

fn open_lock(path: &Path) -> File {
    fs::create_dir_all(path.join("runtime")).expect("runtime directory");
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path.join("runtime/core.lock"))
        .expect("lock file")
}

#[test]
fn initialization_is_guarded_retryable_idempotent_and_truthful() {
    if let Some(path) = env::var_os("FLIT_LOCK_PROBE_PATH") {
        assert_eq!(
            initialize_core(
                PathBuf::from(path).to_string_lossy().into_owned(),
                PROTOCOL_VERSION.to_owned(),
            ),
            Err(BridgeError::CoreAlreadyRunning)
        );
        return;
    }

    let data_directory = test_directory("storage-health");
    let other_directory = test_directory("other-storage");
    let mismatch_directory = test_directory("mismatched-client");
    assert_eq!(health().storage, HealthStatus::NotConfigured);
    assert_eq!(
        initialize_core(
            mismatch_directory.to_string_lossy().into_owned(),
            "2.0".to_owned(),
        ),
        Err(BridgeError::ProtocolMismatch)
    );
    assert!(!mismatch_directory.exists());

    let lock = open_lock(&data_directory);
    lock.try_lock().expect("test owns writer lock");

    assert_eq!(
        initialize_core(
            data_directory.to_string_lossy().into_owned(),
            PROTOCOL_VERSION.to_owned(),
        ),
        Err(BridgeError::CoreAlreadyRunning)
    );
    assert!(!data_directory.join("flit.sqlite3").exists());
    assert_eq!(health().storage, HealthStatus::Unavailable);

    lock.unlock().expect("release test writer lock");
    drop(lock);

    #[cfg(unix)]
    {
        let lock_path = data_directory.join("runtime/core.lock");
        let symlink_target = test_directory("lock-symlink-target");
        fs::remove_file(&lock_path).expect("remove exact lock fixture");
        fs::write(&symlink_target, b"sentinel").expect("lock symlink target");
        std::os::unix::fs::symlink(&symlink_target, &lock_path).expect("lock symlink fixture");
        assert_eq!(
            initialize_core(
                data_directory.to_string_lossy().into_owned(),
                PROTOCOL_VERSION.to_owned(),
            ),
            Err(BridgeError::InvalidDataDirectory)
        );
        assert_eq!(
            fs::read(&symlink_target).expect("preserved symlink target"),
            b"sentinel"
        );
        fs::remove_file(lock_path).expect("remove exact lock symlink");
        fs::remove_file(symlink_target).expect("remove exact symlink target");

        let lock_path = data_directory.join("runtime/core.lock");
        let hardlink_target = test_directory("lock-hardlink-target");
        fs::write(&hardlink_target, b"sentinel").expect("lock hardlink target");
        fs::hard_link(&hardlink_target, &lock_path).expect("lock hardlink fixture");
        assert_eq!(
            initialize_core(
                data_directory.to_string_lossy().into_owned(),
                PROTOCOL_VERSION.to_owned(),
            ),
            Err(BridgeError::InvalidDataDirectory)
        );
        assert_eq!(
            fs::read(&hardlink_target).expect("preserved hardlink target"),
            b"sentinel"
        );
        fs::remove_file(lock_path).expect("remove exact lock hardlink");
        fs::remove_file(hardlink_target).expect("remove exact hardlink target");

        for suffix in ["-wal", "-shm"] {
            let sidecar_path = PathBuf::from(format!(
                "{}{suffix}",
                data_directory.join("flit.sqlite3").display()
            ));
            let hardlink_target = test_directory(&format!("sidecar{suffix}-target"));
            fs::write(&hardlink_target, b"sidecar sentinel").expect("sidecar hardlink target");
            fs::hard_link(&hardlink_target, &sidecar_path).expect("sidecar hardlink fixture");
            let before = fs::metadata(&hardlink_target).expect("sidecar target metadata");

            assert_eq!(
                initialize_core(
                    data_directory.to_string_lossy().into_owned(),
                    PROTOCOL_VERSION.to_owned(),
                ),
                Err(BridgeError::InvalidDataDirectory)
            );
            let after = fs::metadata(&hardlink_target).expect("preserved sidecar target metadata");
            assert_eq!(
                fs::read(&hardlink_target).expect("preserved sidecar target"),
                b"sidecar sentinel"
            );
            assert_eq!(after.len(), before.len());
            assert_eq!(after.permissions().mode(), before.permissions().mode());
            assert_eq!(after.nlink(), before.nlink());
            assert!(!data_directory.join("flit.sqlite3").exists());

            fs::remove_file(sidecar_path).expect("remove exact sidecar hardlink");
            fs::remove_file(hardlink_target).expect("remove exact sidecar target");
        }

        let dangling_target = test_directory("dangling-sidecar-target");
        let wal_path = PathBuf::from(format!(
            "{}-wal",
            data_directory.join("flit.sqlite3").display()
        ));
        std::os::unix::fs::symlink(&dangling_target, &wal_path).expect("dangling sidecar symlink");
        assert_eq!(
            initialize_core(
                data_directory.to_string_lossy().into_owned(),
                PROTOCOL_VERSION.to_owned(),
            ),
            Err(BridgeError::InvalidDataDirectory)
        );
        assert!(!dangling_target.exists());
        assert!(!data_directory.join("flit.sqlite3").exists());
        fs::remove_file(wal_path).expect("remove exact dangling sidecar");
    }

    fs::write(
        data_directory.join("flit.sqlite3"),
        b"not a SQLite database",
    )
    .expect("corrupt Store fixture");
    assert_eq!(
        initialize_core(
            data_directory.to_string_lossy().into_owned(),
            PROTOCOL_VERSION.to_owned(),
        ),
        Err(BridgeError::StorageFailure)
    );
    assert_eq!(health().storage, HealthStatus::Unavailable);
    fs::remove_file(data_directory.join("flit.sqlite3")).expect("remove exact corrupt fixture");

    initialize_core(
        data_directory.to_string_lossy().into_owned(),
        PROTOCOL_VERSION.to_owned(),
    )
    .expect("retry initializes storage");
    assert_eq!(health().storage, HealthStatus::Ready);
    assert!(data_directory.join("flit.sqlite3").is_file());
    #[cfg(unix)]
    {
        assert_eq!(
            fs::metadata(&data_directory)
                .expect("data-directory metadata")
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(data_directory.join("runtime"))
                .expect("runtime metadata")
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        for file in [
            data_directory.join("flit.sqlite3"),
            data_directory.join("runtime/core.lock"),
        ] {
            assert_eq!(
                fs::metadata(file)
                    .expect("owner-only file metadata")
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }

    let child_status = Command::new(env::current_exe().expect("current test executable"))
        .args([
            "--exact",
            "initialization_is_guarded_retryable_idempotent_and_truthful",
        ])
        .env("FLIT_LOCK_PROBE_PATH", &data_directory)
        .status()
        .expect("launch competing Core process");
    assert!(
        child_status.success(),
        "competing Core process must observe the held writer guard"
    );

    initialize_core(
        data_directory.to_string_lossy().into_owned(),
        PROTOCOL_VERSION.to_owned(),
    )
    .expect("same directory repeat is idempotent");
    assert_eq!(core_construction_count(), 1);
    #[cfg(unix)]
    {
        let alias = test_directory("storage-alias");
        std::os::unix::fs::symlink(&data_directory, &alias).expect("data-directory alias");
        initialize_core(
            alias.to_string_lossy().into_owned(),
            PROTOCOL_VERSION.to_owned(),
        )
        .expect("canonical alias repeat is idempotent");
        fs::remove_file(alias).expect("remove exact alias");
    }
    assert_eq!(
        initialize_core(
            other_directory.to_string_lossy().into_owned(),
            PROTOCOL_VERSION.to_owned(),
        ),
        Err(BridgeError::CoreAlreadyInitialized)
    );
    assert!(!other_directory.exists());

    fs::remove_dir_all(&data_directory).expect("remove exact test directory");
}
