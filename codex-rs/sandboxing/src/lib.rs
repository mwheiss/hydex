#[cfg(target_os = "linux")]
mod bwrap;
pub mod landlock;
mod manager;
pub mod policy_transforms;
#[cfg(target_os = "macos")]
pub mod seatbelt;

#[cfg(target_os = "linux")]
pub use bwrap::find_system_bwrap_in_path;
#[cfg(target_os = "linux")]
pub use bwrap::system_bwrap_warning;
pub use manager::SandboxCommand;
pub use manager::SandboxExecRequest;
pub use manager::SandboxManager;
pub use manager::SandboxTransformError;
pub use manager::SandboxTransformRequest;
pub use manager::SandboxType;
pub use manager::SandboxablePreference;
pub use manager::compatibility_sandbox_policy_for_permission_profile;
#[cfg(target_os = "linux")]
pub use manager::ensure_legacy_landlock_supports_managed_mitm;
pub use manager::get_platform_sandbox;
pub use manager::prepare_managed_network_child;

use codex_protocol::error::CodexErr;

#[cfg(not(target_os = "linux"))]
pub fn system_bwrap_warning(
    _permission_profile: &codex_protocol::models::PermissionProfile,
) -> Option<String> {
    None
}

impl From<SandboxTransformError> for CodexErr {
    fn from(err: SandboxTransformError) -> Self {
        match err {
            error @ SandboxTransformError::InvalidCommandCwd { .. }
            | error @ SandboxTransformError::InvalidSandboxPolicyCwd { .. } => {
                CodexErr::InvalidRequest(error.to_string())
            }
            SandboxTransformError::MissingLinuxSandboxExecutable => {
                CodexErr::LandlockSandboxExecutableNotProvided
            }
            SandboxTransformError::ManagedMitmCaPathUnderWritableRoot => {
                CodexErr::UnsupportedOperation(
                    "managed MITM CA isolation requires its proxy directory to be outside sandbox-writable roots"
                        .to_string(),
                )
            }
            SandboxTransformError::ManagedMitmCustomCaUnsupportedOnWindows => {
                CodexErr::UnsupportedOperation(
                    "CA directories and command-specific CA overrides with managed MITM are not supported in the Windows sandbox because its read grants persist across commands"
                        .to_string(),
                )
            }
            #[cfg(target_os = "linux")]
            SandboxTransformError::LegacyLandlockUnsupportedWithManagedMitm => {
                CodexErr::UnsupportedOperation(
                    "managed MITM CA isolation requires bubblewrap and is incompatible with legacy Landlock"
                        .to_string(),
                )
            }
            #[cfg(target_os = "linux")]
            SandboxTransformError::Wsl1UnsupportedForBubblewrap => {
                CodexErr::UnsupportedOperation(crate::bwrap::WSL1_BWRAP_WARNING.to_string())
            }
            #[cfg(not(target_os = "macos"))]
            SandboxTransformError::SeatbeltUnavailable => CodexErr::UnsupportedOperation(
                "seatbelt sandbox is only available on macOS".to_string(),
            ),
        }
    }
}
