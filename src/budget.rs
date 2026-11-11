//! Every time budget of the harness, in one table. Waits end on structured
//! signals (a readiness marker, a login prompt, an SSH login, a command's
//! done marker, the domain ending); a budget only bounds a wait that is still
//! making progress. Without KVM, libvirt runs the VMs under TCG emulation, so
//! every budget is multiplied by [`TCG_FACTOR`] (and that is printed once).

use std::sync::Once;
use std::time::Duration;

/// How much longer everything takes under TCG.
pub const TCG_FACTOR: u32 = 4;

/// The budgets, in seconds, before the TCG factor.
#[derive(Debug, Clone, Copy)]
pub enum Budget {
    /// Console silence that counts as a stalled boot, on every variant (it
    /// only fires on silence, so a healthy boot never waits for it).
    BootSilence,
    /// For the serial socket to appear after the domain is created.
    SerialConnect,
    /// For SSH to accept a login once the live system is up.
    SshReady,
    /// For a getty login to reach a shell.
    Login,
    /// A short guest command (a probe, a tool's `--version`).
    Command,
    /// Copying and running an evidence script.
    Evidence,
    /// One install step (recpart, recstrap, recsetup) or an A/B stage.
    Stage,
    /// For the guest to end after `poweroff` or `reboot`.
    Shutdown,
    /// A degraded A/B trial: boot, start-up, recab's decision and the reboot.
    TrialReboot,
    /// For recab-trial to commit a healthy trial.
    Commit,
    /// Explicit public payload download/interruption; bounded independently of local staging.
    PublicDownload,
}

impl Budget {
    fn seconds(self) -> u64 {
        match self {
            Self::BootSilence => 180,
            Self::SerialConnect => 20,
            Self::SshReady => 90,
            Self::Login => 30,
            Self::Command => 30,
            Self::Evidence => 120,
            Self::Stage => 900,
            Self::Shutdown => 180,
            Self::TrialReboot => 300,
            Self::Commit => 200,
            Self::PublicDownload => 3600,
        }
    }

    /// The budget on this host.
    pub fn get(self) -> Duration {
        let factor = if recqemu::libvirt::kvm_available() {
            1
        } else {
            static WARNED: Once = Once::new();
            WARNED.call_once(|| {
                eprintln!("warning: no /dev/kvm; VMs run under TCG and every budget is {TCG_FACTOR}x longer")
            });
            TCG_FACTOR
        };
        Duration::from_secs(self.seconds() * u64::from(factor))
    }
}
