//! Cross-process file locking for Mind mutations.

use std::cell::Cell;
use std::path::PathBuf;

use types::{RustyBrainError, error_codes};

use super::Mind;

thread_local! {
    static IN_FILE_LOCK: Cell<bool> = const { Cell::new(false) };
}

impl Mind {
    /// Execute a closure while holding an exclusive file lock on the `.mv2` file.
    ///
    /// Uses `fs2::FileExt::try_lock_exclusive` with exponential backoff
    /// (100ms base, 5 retries, 2x multiplier). The lock file is created
    /// adjacent to the memory file with `.lock` extension and 0600 permissions.
    ///
    /// # Errors
    ///
    /// Returns `RustyBrainError::LockTimeout` if the lock cannot be acquired
    /// after all retries. Propagates any error from the closure.
    pub fn with_lock<F, T>(&self, f: F) -> Result<T, RustyBrainError>
    where
        F: FnOnce(&Self) -> Result<T, RustyBrainError>,
    {
        use fs2::FileExt;

        if IN_FILE_LOCK.with(Cell::get) {
            return f(self);
        }

        let mut lock_os = self.memory_path.as_os_str().to_os_string();
        lock_os.push(".lock");
        let lock_path = PathBuf::from(lock_os);
        let lock_file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&lock_path)
            .map_err(|e| RustyBrainError::FileSystem {
                code: error_codes::E_FS_IO_ERROR,
                message: format!("failed to open lock file: {}", lock_path.display()),
                source: Some(e),
            })?;

        // Set lock file permissions to 0600.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::Permissions::from_mode(0o600);
            std::fs::set_permissions(&lock_path, perms).map_err(|e| {
                RustyBrainError::FileSystem {
                    code: error_codes::E_FS_IO_ERROR,
                    message: format!(
                        "failed to set lock file permissions: {}",
                        lock_path.display()
                    ),
                    source: Some(e),
                }
            })?;
        }

        // Exponential backoff: 100ms base, 5 retries, 2x multiplier.
        let max_retries: u32 = 5;
        for attempt in 0..=max_retries {
            match lock_file.try_lock_exclusive() {
                Ok(()) => {
                    let _reentrant_guard = ReentrantLockGuard::enter();
                    let result = f(self);
                    // Dropping the file closes the fd, releasing the flock.
                    drop(lock_file);
                    return result;
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock && attempt < max_retries => {
                    let delay = std::time::Duration::from_millis(100) * 2u32.pow(attempt);
                    std::thread::sleep(delay);
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    return Err(RustyBrainError::LockTimeout {
                        code: error_codes::E_LOCK_TIMEOUT,
                        message: format!(
                            "failed to acquire lock after {max_retries} retries: {}",
                            lock_path.display()
                        ),
                    });
                }
                Err(e) => {
                    return Err(RustyBrainError::FileSystem {
                        code: error_codes::E_FS_IO_ERROR,
                        message: format!("failed to acquire lock: {}", lock_path.display()),
                        source: Some(e),
                    });
                }
            }
        }

        unreachable!()
    }
}

struct ReentrantLockGuard {
    previous: bool,
}

impl ReentrantLockGuard {
    fn enter() -> Self {
        let previous = IN_FILE_LOCK.with(|locked| {
            let prev = locked.get();
            locked.set(true);
            prev
        });
        Self { previous }
    }
}

impl Drop for ReentrantLockGuard {
    fn drop(&mut self) {
        IN_FILE_LOCK.with(|locked| locked.set(self.previous));
    }
}
