//! Small shared helpers used across the backend.

use std::ffi::OsStr;
use std::process::Command;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

/// Windows flag that suppresses the console window when spawning a child process.
/// Previously duplicated in acp.rs / lib.rs / providers.rs — hoisted here so every
/// spawn site (including the new job runner) shares one definition.
#[cfg(windows)]
pub const CREATE_NO_WINDOW: u32 = 0x08000000;

/// Build a `Command` that never flashes a console window on Windows. Use this
/// for every process spawn so no code path forgets the flag (the cause of the
/// old console-flash bugs in `which_on_path` / `grok_version_at`).
pub fn hidden_command<S: AsRef<OsStr>>(program: S) -> Command {
    #[allow(unused_mut)]
    let mut cmd = Command::new(program);
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);
    cmd
}
