//! Running external tools, with errors that name the command.

use std::process::Command;

/// Runs `cmd`, failing unless it exits successfully.
pub fn run(cmd: &mut Command) -> crate::Result<()> {
    let status = cmd.status().map_err(|e| format!("{cmd:?}: {e}"))?;
    if status.success() { Ok(()) } else { Err(format!("{cmd:?} failed with {status}")) }
}

/// Runs `cmd` and returns its standard output, failing unless it exits successfully.
pub fn output(cmd: &mut Command) -> crate::Result<String> {
    let out = cmd.output().map_err(|e| format!("{cmd:?}: {e}"))?;
    if !out.status.success() {
        return Err(format!("{cmd:?} failed with {}", out.status));
    }
    String::from_utf8(out.stdout).map_err(|e| e.to_string())
}
