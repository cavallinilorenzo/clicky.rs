//! Build tooling for Clicky: `cargo xtask <command>`.

mod audit;
mod bundle;
mod catalog;
mod decode;
mod dsp;
mod loudness;
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
  bundle [options]    build target/bundle/Clicky.app (universal), sign and audit it
  run [options]       bundle, then quit any running Clicky and open the new one
  audit [app]         check the app for networking, typed-text APIs and its minimum
                      macOS (default target/bundle/Clicky.app; bundle runs it too)

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
        Some("audit") => audit::run(&rest.first().map_or_else(
            || root.join("target/bundle").join(format!("{}.app", bundle::APP_NAME)),
            PathBuf::from,
        )),
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
