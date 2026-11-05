//! Build tooling for Clicky: `cargo xtask <command>`.

mod bundle;
mod catalog;
mod decode;
mod dsp;
mod packs;
mod sign;
mod sources;
#[cfg(test)]
mod testutil;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// Errors are human-readable strings; this is a build tool.
pub type Result<T, E = String> = std::result::Result<T, E>;

const USAGE: &str = "usage: cargo xtask <command>

commands:
  packs               convert assets/soundpacks into target/packs/*.clickypack
  bundle [options]    build target/bundle/Clicky.app (universal) and sign it
  run [options]       bundle, then quit any running Clicky and open the new one

options:
  --native            build for this Mac's architecture only (default: universal)
  --adhoc             sign ad-hoc even if the local signing identity exists";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let root = workspace_root();
    let rest = args.get(1..).unwrap_or_default();
    let result = match args.first().map(String::as_str) {
        Some("packs") => packs::run(&root),
        Some("bundle") => bundle::run(&root, rest).map(|_| ()),
        Some("run") => bundle::run(&root, rest).and_then(|app| bundle::launch(&app)),
        _ => {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

/// The repository root (this crate lives in `crates/xtask`).
pub fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("crates/xtask sits two levels below the workspace root")
        .to_path_buf()
}
