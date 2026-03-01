//! Advisory file locking for session files.
//!
//! Uses OS-level advisory locks (`flock(2)` on Unix, `LockFileEx` on Windows)
//! via the `fs2` crate to prevent two processes from concurrently writing to
//! the same session file.
//!
//! The lock is held for the entire lifetime of [`SessionLock`] and released
//! automatically when the value is dropped.

use fs2::FileExt;
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// An advisory exclusive lock on a session file.
///
/// Creating a `SessionLock` opens (or creates) a companion `.lock` file next
/// to the session and acquires an exclusive `flock` on it.  The session data
/// file itself is untouched; the lock file is only used as a lock target so
/// that readers who never call `try_acquire` are unaffected.
///
/// The lock is released when the `SessionLock` is dropped.
#[derive(Debug)]
pub struct SessionLock {
    /// The open file descriptor that holds the OS lock.
    file: File,
    /// Path to the `.lock` companion file.
    path: PathBuf,
}

impl SessionLock {
    /// Derive the companion `.lock` file path from a session file path.
    ///
    /// Given `/path/to/abc12345.jsonl`, returns `/path/to/abc12345.jsonl.lock`.
    fn companion_lock_path(session_path: &Path) -> PathBuf {
        let mut p = session_path.as_os_str().to_os_string();
        p.push(".lock");
        PathBuf::from(p)
    }

    /// Open (or create) the companion lock file for `session_path`.
    fn open_lock_file(lock_path: &Path) -> std::io::Result<File> {
        // Create parent directories if they do not yet exist.
        if let Some(parent) = lock_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(lock_path)
    }

    /// Try to acquire an **exclusive, non-blocking** advisory lock on the
    /// companion `.lock` file for `session_path`.
    ///
    /// Returns `Ok(SessionLock)` if the lock was obtained immediately, or an
    /// `Err` with `ErrorKind::WouldBlock` if another process already holds the
    /// lock.
    ///
    /// # Errors
    ///
    /// - `std::io::ErrorKind::WouldBlock` – lock is held by another process.
    /// - Any other `io::Error` from opening or locking the file.
    pub fn try_acquire(session_path: &Path) -> std::io::Result<Self> {
        let lock_path = Self::companion_lock_path(session_path);
        let file = Self::open_lock_file(&lock_path)?;
        file.try_lock_exclusive()?;
        Ok(Self {
            file,
            path: lock_path,
        })
    }

    /// Acquire an **exclusive** advisory lock on the companion `.lock` file,
    /// blocking the calling thread until either the lock is obtained or
    /// `timeout` elapses.
    ///
    /// The implementation polls with `try_lock_exclusive` at an exponential
    /// back-off (starting at 1 ms, doubling up to 128 ms, then staying there)
    /// so that the caller does not spin-wait.
    ///
    /// # Errors
    ///
    /// - `std::io::ErrorKind::TimedOut` – timeout elapsed before the lock
    ///   could be obtained.
    /// - Any other `io::Error` from opening or locking the file.
    pub fn acquire_with_timeout(session_path: &Path, timeout: Duration) -> std::io::Result<Self> {
        let lock_path = Self::companion_lock_path(session_path);
        let file = Self::open_lock_file(&lock_path)?;

        let deadline = Instant::now() + timeout;
        let mut wait = Duration::from_millis(1);
        const MAX_WAIT: Duration = Duration::from_millis(128);

        loop {
            match file.try_lock_exclusive() {
                Ok(()) => {
                    return Ok(Self {
                        file,
                        path: lock_path,
                    });
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    let now = Instant::now();
                    if now >= deadline {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::TimedOut,
                            format!(
                                "could not acquire session lock on {} within {:?}",
                                lock_path.display(),
                                timeout,
                            ),
                        ));
                    }
                    // Do not sleep longer than remaining time.
                    let remaining = deadline - now;
                    let sleep_for = wait.min(remaining);
                    std::thread::sleep(sleep_for);
                    wait = (wait * 2).min(MAX_WAIT);
                }
                Err(e) => return Err(e),
            }
        }
    }

    /// Explicitly release the lock and consume `self`.
    ///
    /// Equivalent to dropping the value, but makes the intent clear at the
    /// call site.  The companion `.lock` file is left on disk (it is cheap and
    /// harmless to leave around; re-using it on the next open avoids a
    /// potential TOCTOU race between delete + create).
    pub fn release(self) {
        // Unlock before the File is closed so that the unlock syscall is
        // explicit rather than implicit.  Ignore the result because the only
        // realistic error is that the file is already closed/unlocked, which
        // is fine.
        let _ = self.file.unlock();
        // `self` is dropped here, closing the file descriptor.
    }

    /// Return the path of the companion `.lock` file.
    pub fn lock_path(&self) -> &Path {
        &self.path
    }
}

impl Drop for SessionLock {
    fn drop(&mut self) {
        // Explicitly call unlock so that the OS releases the lock before the
        // fd is closed.  This is technically redundant (closing the fd also
        // releases the lock on Linux/macOS), but it makes the intent
        // unmistakable and avoids relying on that side-effect on every
        // platform.
        let _ = self.file.unlock();
    }
}

// -----------------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use uuid::Uuid;

    fn temp_session_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("pi-rs-lock-test-{name}-{}.jsonl", Uuid::new_v4()))
    }

    // -------------------------------------------------------------------------
    // 1. Acquire and release a lock successfully
    // -------------------------------------------------------------------------

    #[test]
    fn acquire_and_release_succeeds() {
        let session = temp_session_path("acquire-release");

        let lock =
            SessionLock::try_acquire(&session).expect("should acquire lock on fresh session path");

        // The companion .lock file must exist while the lock is held.
        assert!(
            lock.lock_path().exists(),
            ".lock file should exist while lock is held"
        );

        // Explicitly releasing should not panic.
        lock.release();

        // After release the companion file still exists (by design) but the
        // lock is no longer held.  We verify re-acquisition works.
        let lock2 =
            SessionLock::try_acquire(&session).expect("should re-acquire lock after release");
        lock2.release();
    }

    // -------------------------------------------------------------------------
    // 2. A second try_acquire on the same path fails while the first is held
    // -------------------------------------------------------------------------
    //
    // This test relies on `flock(2)` per-open-file-description semantics:
    // two separate `open()` calls within the same process create independent
    // file descriptions, so the second `try_lock_exclusive` returns
    // `EWOULDBLOCK`.  This holds on macOS and Linux.  If this test fails on
    // a new platform, the lock still works across *processes* — this is
    // specifically a same-process, different-fd contention test.

    #[test]
    fn double_acquire_fails() {
        let session = temp_session_path("double-acquire");

        let _lock1 = SessionLock::try_acquire(&session).expect("first acquire should succeed");

        // A second acquire via a fresh open() creates a new file description,
        // so flock should reject it.
        let result = SessionLock::try_acquire(&session);

        assert!(
            result.is_err(),
            "second try_acquire should fail while first lock is held"
        );

        // On most platforms this is WouldBlock; accept any error kind since
        // the exact mapping varies (e.g. some report PermissionDenied on
        // Windows).
    }

    // -------------------------------------------------------------------------
    // 3. Lock is released when the SessionLock is dropped (RAII)
    // -------------------------------------------------------------------------

    #[test]
    fn lock_released_on_drop() {
        let session = temp_session_path("drop-release");

        {
            let _lock = SessionLock::try_acquire(&session).expect("first acquire should succeed");
            // `_lock` is dropped here at the end of this block.
        }

        // After drop, re-acquisition must succeed.
        let lock2 = SessionLock::try_acquire(&session)
            .expect("should acquire lock after previous lock was dropped");
        lock2.release();
    }

    // -------------------------------------------------------------------------
    // 4. acquire_with_timeout succeeds when no contention exists
    // -------------------------------------------------------------------------

    #[test]
    fn acquire_with_timeout_succeeds_when_uncontested() {
        let session = temp_session_path("timeout-uncontested");

        let lock = SessionLock::acquire_with_timeout(&session, Duration::from_secs(1))
            .expect("should acquire lock within timeout when uncontested");

        lock.release();
    }

    // -------------------------------------------------------------------------
    // 5. acquire_with_timeout returns TimedOut when lock is held by another
    //    thread / open-file-description for longer than the timeout.
    // -------------------------------------------------------------------------

    #[test]
    fn acquire_with_timeout_returns_timed_out() {
        let session = temp_session_path("timeout-contended");

        // Hold the lock in the current thread.
        let _lock = SessionLock::try_acquire(&session).expect("first acquire should succeed");

        // Attempt to acquire from a second file description with a very short
        // timeout so the test does not take long.
        let result = SessionLock::acquire_with_timeout(&session, Duration::from_millis(30));

        assert!(
            result.is_err(),
            "acquire_with_timeout should fail when lock is held"
        );

        let err = result.unwrap_err();
        assert_eq!(
            err.kind(),
            std::io::ErrorKind::TimedOut,
            "error kind should be TimedOut, got: {:?}",
            err
        );
    }

    // -------------------------------------------------------------------------
    // 6. Lock files for different sessions are independent
    // -------------------------------------------------------------------------

    #[test]
    fn different_sessions_are_independent() {
        let session_a = temp_session_path("indep-a");
        let session_b = temp_session_path("indep-b");

        let lock_a =
            SessionLock::try_acquire(&session_a).expect("lock on session A should succeed");
        let lock_b = SessionLock::try_acquire(&session_b)
            .expect("lock on session B should succeed even while A is locked");

        lock_a.release();
        lock_b.release();
    }
}
