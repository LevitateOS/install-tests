//! Chroot execution for QEMU console.
//!
//! Uses `recchroot` (like arch-chroot) to run commands inside a chroot.
//! recchroot handles bind mounts automatically - no state tracking needed.

use anyhow::Result;
use std::time::Duration;

use super::console::{CommandResult, Console};

impl Console {
    /// Execute command in chroot using recchroot.
    ///
    /// recchroot handles bind mounts (/dev, /proc, /sys, /run) automatically.
    /// Each call is independent - no enter/exit state to manage.
    pub fn exec_chroot(&mut self, path: &str, command: &str, timeout: Duration) -> Result<CommandResult> {
        // recchroot handles all the bind mount setup/teardown
        let full_cmd = format!(
            "recchroot '{}' /bin/bash -c '{}'",
            path.replace('\'', "'\\''"),
            command.replace('\'', "'\\''")
        );
        self.exec(&full_cmd, timeout)
    }

    /// Execute a long-running command in chroot with STALL DETECTION.
    ///
    /// Same as exec_streaming but runs the command inside the chroot via recchroot.
    /// Use this for commands like dracut that legitimately take a long time.
    pub fn exec_chroot_streaming(
        &mut self,
        path: &str,
        command: &str,
        stall_timeout: Duration,
        error_patterns: &[&str],
    ) -> Result<CommandResult> {
        let full_cmd = format!(
            "recchroot '{}' /bin/bash -c '{}'",
            path.replace('\'', "'\\''"),
            command.replace('\'', "'\\''")
        );
        self.exec_streaming(&full_cmd, stall_timeout, error_patterns)
    }
}
