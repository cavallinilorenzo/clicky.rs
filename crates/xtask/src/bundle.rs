//! `cargo xtask bundle`: builds the release binary (universal by default),
//! converts the soundpacks, and assembles and signs `target/bundle/Clicky.app`.
//!
//! Derived from Rustyvibes 2 (MIT, © 2021–2026 Kunal Bagaria,
//! https://github.com/KunalBagaria/rustyvibes-v2), `crates/xtask/src/bundle.rs`.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::cmd::{output, run as run_cmd};
use crate::packs::{self, EXTENSION};
use crate::sign;

pub const APP_NAME: &str = "Clicky";
pub const BUNDLE_ID: &str = "io.github.cavallinilorenzo.clicky";
pub const MIN_MACOS: &str = "13.0";
/// Generated from its SVG by `scripts/build-brand.sh` and committed.
const APP_ICON: &str = "assets/brand/app-icon/Clicky.icns";
const TARGETS: [&str; 2] = ["aarch64-apple-darwin", "x86_64-apple-darwin"];
/// Architectures `lipo -archs` must report for a universal build.
const ARCHS: [&str; 2] = ["arm64", "x86_64"];

pub struct Options {
    /// Build for Apple Silicon and Intel (default) instead of the host only.
    pub universal: bool,
    /// Sign ad-hoc even when the local identity is available.
    pub adhoc: bool,
}

impl Options {
    pub fn parse(args: &[String]) -> crate::Result<Options> {
        let mut options = Options { universal: true, adhoc: false };
        for arg in args {
            match arg.as_str() {
                "--native" => options.universal = false,
                "--adhoc" => options.adhoc = true,
                other => return Err(format!("unknown option {other:?}")),
            }
        }
        Ok(options)
    }
}

/// `cargo xtask bundle [--native] [--adhoc]`; returns the path of the app.
pub fn run(root: &Path, args: &[String]) -> crate::Result<PathBuf> {
    let options = Options::parse(args)?;
    let version = env!("CARGO_PKG_VERSION");
    let binaries = compile(root, options.universal)?;
    let packs_dir = root.join("target/packs");
    let reports = packs::build_all(&root.join("assets/soundpacks/catalog.json"), &packs_dir)?;

    let app = root.join("target/bundle").join(format!("{APP_NAME}.app"));
    if app.exists() {
        std::fs::remove_dir_all(&app).map_err(|e| format!("{}: {e}", app.display()))?;
    }
    let contents = app.join("Contents");
    let resources = contents.join("Resources");
    for dir in [contents.join("MacOS"), resources.join("Packs"), resources.join("Licenses")] {
        std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }

    let exe = contents.join("MacOS").join(APP_NAME);
    run_cmd(Command::new("lipo").arg("-create").arg("-output").arg(&exe).args(&binaries))?;
    let archs = output(Command::new("lipo").arg("-archs").arg(&exe))?;
    if options.universal && !ARCHS.iter().all(|a| archs.split_whitespace().any(|x| x == *a)) {
        return Err(format!("expected a universal binary, lipo reports {archs:?}"));
    }
    write(&contents.join("Info.plist"), &info_plist(version))?;
    write(&contents.join("PkgInfo"), "APPL????")?;
    copy(&root.join(APP_ICON), &resources.join(format!("{APP_NAME}.icns")))?;
    for report in &reports {
        let name = format!("{}.{EXTENSION}", report.id);
        copy(&packs_dir.join(&name), &resources.join("Packs").join(&name))?;
    }
    for (source, name) in [
        ("THIRD_PARTY_NOTICES.md", "Third-Party Notices.txt"),
        ("assets/soundpacks/mechvibes/LICENSE", "Mechvibes.txt"),
        ("assets/soundpacks/kbsim/LICENSE.md", "kbsim.txt"),
    ] {
        copy(&root.join(source), &resources.join("Licenses").join(name))?;
    }

    let signature = sign::sign(&app, options.adhoc)?;
    crate::audit::run(&app)?;
    println!(
        "{} ({}; {} soundpack(s)), {}",
        app.display(),
        archs.trim(),
        reports.len(),
        sign::describe(&signature)
    );
    Ok(app)
}

/// Quits a running Clicky (by bundle id, so no other process named Clicky is
/// touched), then opens `app` with LaunchServices.
pub fn launch(app: &Path) -> crate::Result<()> {
    // `is running` never launches the app, so "not running" is no error.
    let script = format!(
        "if application id \"{BUNDLE_ID}\" is running then \
         tell application id \"{BUNDLE_ID}\" to quit"
    );
    if let Err(e) = run_cmd(Command::new("osascript").args(["-e", &script])) {
        eprintln!("warning: could not quit the running {APP_NAME} ({e})");
    }
    std::thread::sleep(std::time::Duration::from_millis(300));
    run_cmd(Command::new("open").arg(app))
}

/// Fails, naming the command to run, unless every target in `needed` is in
/// `installed` (the output of `rustup target list --installed`). The build
/// never installs toolchain components by itself.
fn check_targets(installed: &str, needed: &[&str]) -> crate::Result<()> {
    let missing: Vec<&str> = needed
        .iter()
        .copied()
        .filter(|target| !installed.lines().any(|line| line.trim() == *target))
        .collect();
    if missing.is_empty() {
        return Ok(());
    }
    Err(format!(
        "the universal build needs the Rust target(s) {}. Install them with \
         `rustup target add {}`, or build for this Mac only with --native",
        missing.join(", "),
        missing.join(" ")
    ))
}

/// Builds `clicky` in release mode for each target and returns the binaries.
fn compile(root: &Path, universal: bool) -> crate::Result<Vec<PathBuf>> {
    let host = if cfg!(target_arch = "aarch64") { TARGETS[0] } else { TARGETS[1] };
    let targets: Vec<&str> = if universal { TARGETS.to_vec() } else { vec![host] };
    if universal {
        let installed = output(Command::new("rustup").args(["target", "list", "--installed"]))?;
        check_targets(&installed, &targets)?;
    }
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let mut binaries = Vec::new();
    for target in targets {
        run_cmd(
            Command::new(&cargo)
                .current_dir(root)
                .env("MACOSX_DEPLOYMENT_TARGET", MIN_MACOS)
                .args(["build", "--release", "--locked", "-p", "clicky", "--target", target]),
        )?;
        binaries.push(root.join("target").join(target).join("release/clicky"));
    }
    Ok(binaries)
}

pub fn info_plist(version: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleDevelopmentRegion</key>
	<string>en</string>
	<key>CFBundleDisplayName</key>
	<string>{APP_NAME}</string>
	<key>CFBundleExecutable</key>
	<string>{APP_NAME}</string>
	<key>CFBundleIconFile</key>
	<string>{APP_NAME}</string>
	<key>CFBundleIdentifier</key>
	<string>{BUNDLE_ID}</string>
	<key>CFBundleInfoDictionaryVersion</key>
	<string>6.0</string>
	<key>CFBundleName</key>
	<string>{APP_NAME}</string>
	<key>CFBundlePackageType</key>
	<string>APPL</string>
	<key>CFBundleShortVersionString</key>
	<string>{version}</string>
	<key>CFBundleVersion</key>
	<string>{version}</string>
	<key>LSApplicationCategoryType</key>
	<string>public.app-category.utilities</string>
	<key>LSMinimumSystemVersion</key>
	<string>{MIN_MACOS}</string>
	<key>LSUIElement</key>
	<true/>
	<key>NSHighResolutionCapable</key>
	<true/>
	<key>NSSupportsAutomaticTermination</key>
	<false/>
	<key>NSSupportsSuddenTermination</key>
	<true/>
</dict>
</plist>
"#
    )
}

fn copy(from: &Path, to: &Path) -> crate::Result<()> {
    std::fs::copy(from, to)
        .map(|_| ())
        .map_err(|e| format!("copy {} → {}: {e}", from.display(), to.display()))
}

fn write(path: &Path, text: &str) -> crate::Result<()> {
    std::fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn info_plist_makes_a_menu_bar_only_app_for_macos_13() {
        let plist = info_plist("1.2.3");
        for needle in [
            "<key>CFBundleIdentifier</key>\n\t<string>io.github.cavallinilorenzo.clicky</string>",
            "<key>CFBundleExecutable</key>\n\t<string>Clicky</string>",
            "<key>CFBundleIconFile</key>\n\t<string>Clicky</string>",
            "<key>CFBundleShortVersionString</key>\n\t<string>1.2.3</string>",
            "<key>LSMinimumSystemVersion</key>\n\t<string>13.0</string>",
            "<key>LSUIElement</key>\n\t<true/>",
        ] {
            assert!(plist.contains(needle), "missing {needle}");
        }
    }

    #[test]
    fn a_missing_target_names_the_command_that_adds_it() {
        let installed = "aarch64-apple-darwin\nwasm32-unknown-unknown\n";
        assert_eq!(check_targets(installed, &["aarch64-apple-darwin"]), Ok(()));
        let error = check_targets(installed, &TARGETS).unwrap_err();
        assert!(error.contains("rustup target add x86_64-apple-darwin"), "{error}");
        assert!(!error.contains("add aarch64"), "{error}");
        assert!(error.contains("--native"), "offers the host-only build: {error}");
    }

    #[test]
    fn options_default_to_a_universal_signed_build() {
        let options = Options::parse(&[]).unwrap();
        assert!(options.universal && !options.adhoc);
        let options = Options::parse(&["--native".into(), "--adhoc".into()]).unwrap();
        assert!(!options.universal && options.adhoc);
        assert!(Options::parse(&["--fast".into()]).is_err());
    }
}
