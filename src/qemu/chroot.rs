//! Chroot management for QEMU console.
//!
//! Provides enter_chroot(), exit_chroot(), and exec_chroot* functions
//! for running commands inside a chroot environment.

use anyhow::{bail, Context, Result};
use distro_spec::{mounts_in_order, mounts_in_unmount_order};
use std::time::Duration;

use super::console::{CommandResult, Console};

/// Build a properly escaped chroot command.
/// Bug #4 fix: Quote the path with proper escaping.
fn build_chroot_command(path: &str, command: &str) -> String {
    format!(
        "chroot '{}' /bin/bash -c '{}'",
        path.replace('\'', "'\\''"),
        command.replace('\'', "'\\''")
    )
}

impl Console {
    /// Enter a chroot environment.
    pub fn enter_chroot(&mut self, path: &str) -> Result<()> {
        if self.in_chroot {
            bail!("Already in chroot at {:?}", self.chroot_path);
        }

        // Bind mount essential filesystems in order from levitate-spec
        for mount in mounts_in_order() {
            let target = mount.full_target(path);
            let result = self.exec(&mount.mount_command(path), Duration::from_secs(5))?;

            if !result.success() && mount.required {
                bail!(
                    "Failed to mount {} -> {}: {}",
                    mount.source,
                    target,
                    result.output
                );
            }
            // Non-required mount failures are silently ignored
        }

        self.in_chroot = true;
        self.chroot_path = Some(path.to_string());
        Ok(())
    }

    /// Execute command in chroot context.
    pub fn exec_chroot(&mut self, command: &str, timeout: Duration) -> Result<CommandResult> {
        let path = self.chroot_path.as_ref().context("Not in chroot")?;

        let chroot_cmd = build_chroot_command(path, command);
        self.exec(&chroot_cmd, timeout)
    }

    /// Execute command in chroot context, expecting success.
    pub fn exec_chroot_ok(&mut self, command: &str, timeout: Duration) -> Result<String> {
        let result = self.exec_chroot(command, timeout)?;
        if !result.success() {
            bail!(
                "Chroot command failed (exit {}): {}\nOutput: {}",
                result.exit_code,
                command,
                result.output
            );
        }
        Ok(result.output)
    }

    /// Execute a long-running command in chroot with STALL DETECTION.
    ///
    /// Same as exec_streaming but runs the command inside the chroot.
    /// Use this for commands like dracut that legitimately take a long time.
    pub fn exec_chroot_streaming(
        &mut self,
        command: &str,
        stall_timeout: Duration,
        error_patterns: &[&str],
    ) -> Result<CommandResult> {
        let path = self.chroot_path.as_ref().context("Not in chroot")?;

        let chroot_cmd = build_chroot_command(path, command);
        self.exec_streaming(&chroot_cmd, stall_timeout, error_patterns)
    }

    /// Exit chroot environment.
    pub fn exit_chroot(&mut self) -> Result<()> {
        if !self.in_chroot {
            bail!("Not in chroot");
        }

        let path = self.chroot_path.take().unwrap();
        self.in_chroot = false;

        // Unmount in reverse order from levitate-spec
        for mount in mounts_in_unmount_order() {
            let target = mount.full_target(&path);
            // Use lazy unmount to avoid busy errors
            let _ = self.exec(&format!("umount -l {}", target), Duration::from_secs(5));
        }

        Ok(())
    }

    /// Check if currently in chroot.
    pub fn is_in_chroot(&self) -> bool {
        self.in_chroot
    }
}
