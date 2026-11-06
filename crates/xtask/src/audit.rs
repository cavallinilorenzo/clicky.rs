//! `cargo xtask audit`: checks the built app for what it must never do, from
//! the binary itself. Run by `cargo xtask bundle` after signing, so CI fails
//! when any of these appear:
//!
//! - networking: socket or DNS calls, CFNetwork / Network.framework /
//!   NSURLSession and friends (imported or looked up by name at run time),
//!   network entitlements;
//! - typed text: `CGEventKeyboardGetUnicodeString`, `UCKeyTranslate`, and
//!   NSEvent's character selectors;
//! - a slice built for anything but macOS [`MIN_MACOS`].
//!
//! objc2 looks classes and selectors up by name at run time, so they show up
//! as C strings in the binary, not as imports: both are checked.

use std::path::Path;
use std::process::Command;

use crate::bundle::{APP_NAME, MIN_MACOS};
use crate::cmd::output;

/// Imported symbols (as `nm -u` prints them) that reach the network or typed text.
const FORBIDDEN_IMPORTS: [&str; 15] = [
    "_socket",
    "_connect",
    "_bind",
    "_listen",
    "_accept",
    "_send",
    "_sendto",
    "_sendmsg",
    "_recv",
    "_recvfrom",
    "_recvmsg",
    "_getaddrinfo",
    "_gethostbyname",
    "_CGEventKeyboardGetUnicodeString",
    "_UCKeyTranslate",
];

/// Prefixes of imported symbols that reach the network.
const FORBIDDEN_IMPORT_PREFIXES: [&str; 7] = [
    "_CFSocket",
    "_CFHost",
    "_CFHTTP",
    "_CFNetwork",
    "_CFStreamCreatePairWithSocket",
    "_SCNetwork",
    "_nw_",
];

/// Objective-C classes that reach the network, imported or looked up by name.
const NETWORK_CLASSES: [&str; 12] = [
    "NSURLSession",
    "NSURLConnection",
    "NSURLRequest",
    "NSMutableURLRequest",
    "NSURLDownload",
    "NSStream",
    "NSInputStream",
    "NSOutputStream",
    "NSNetService",
    "NSHost",
    "NSSocketPort",
    "NWConnection",
];

/// Selectors that yield typed characters, and C functions that do (as
/// `dlsym` would look them up), by name.
const TEXT_NAMES: [&str; 5] = [
    "characters",
    "charactersIgnoringModifiers",
    "charactersByApplyingModifiers:",
    "CGEventKeyboardGetUnicodeString",
    "UCKeyTranslate",
];

/// Libraries (as `otool -L` prints them) that only networking code needs.
const FORBIDDEN_LIBRARIES: [&str; 5] = [
    "CFNetwork.framework",
    "Network.framework",
    "NetworkExtension.framework",
    "SystemConfiguration.framework",
    "libcurl",
];

/// What the built binary shows, from the tools that read it.
pub struct Evidence<'a> {
    /// `nm -u` of the executable.
    pub imports: &'a str,
    /// `otool -L` of the executable.
    pub libraries: &'a str,
    /// `codesign -d --entitlements - --xml` of the app.
    pub entitlements: &'a str,
    /// `vtool -show-build` of the executable.
    pub build: &'a str,
    /// The executable's bytes.
    pub binary: &'a [u8],
}

/// Every violation found, one line each; empty when the binary is clean.
pub fn check(evidence: &Evidence) -> Vec<String> {
    let mut problems = Vec::new();
    for symbol in evidence.imports.split_whitespace() {
        let class = symbol.strip_prefix("_OBJC_CLASS_$_");
        if FORBIDDEN_IMPORTS.contains(&symbol)
            || FORBIDDEN_IMPORT_PREFIXES.iter().any(|p| symbol.starts_with(p))
            || class.is_some_and(|c| NETWORK_CLASSES.contains(&c))
        {
            problems.push(format!("imports {symbol}"));
        }
    }
    for line in evidence.libraries.lines() {
        if let Some(library) = FORBIDDEN_LIBRARIES.iter().find(|l| line.contains(*l)) {
            problems.push(format!("links {library}: {}", line.trim()));
        }
    }
    // C strings end with a NUL; Rust's own strings run into them unseparated,
    // so a name counts when a C string ends with it.
    for name in NETWORK_CLASSES.iter().chain(&TEXT_NAMES) {
        let needle = [name.as_bytes(), b"\0"].concat();
        if evidence.binary.windows(needle.len()).any(|w| w == needle.as_slice()) {
            problems.push(format!("names {name} (looked up at run time)"));
        }
    }
    for part in evidence.entitlements.split(['<', '>']) {
        if part.contains("network") {
            problems.push(format!("entitlement {part}"));
        }
    }
    let minos: Vec<&str> = evidence
        .build
        .lines()
        .filter_map(|line| line.trim().strip_prefix("minos "))
        .map(str::trim)
        .collect();
    if minos.is_empty() {
        problems.push("no minimum macOS (LC_BUILD_VERSION) found".to_owned());
    }
    for version in minos.iter().filter(|v| **v != MIN_MACOS) {
        problems.push(format!("a slice requires macOS {version}, not {MIN_MACOS}"));
    }
    problems
}

/// `cargo xtask audit [path/to/Clicky.app]` (default `target/bundle/Clicky.app`).
pub fn run(app: &Path) -> crate::Result<()> {
    let exe = app.join("Contents/MacOS").join(APP_NAME);
    let imports = output(Command::new("nm").arg("-u").arg(&exe))?;
    let libraries = output(Command::new("otool").arg("-L").arg(&exe))?;
    let entitlements =
        output(Command::new("codesign").args(["-d", "--entitlements", "-", "--xml"]).arg(app))?;
    let build = output(Command::new("vtool").arg("-show-build").arg(&exe))?;
    let binary = std::fs::read(&exe).map_err(|e| format!("{}: {e}", exe.display()))?;
    let problems = check(&Evidence {
        imports: &imports,
        libraries: &libraries,
        entitlements: &entitlements,
        build: &build,
        binary: &binary,
    });
    if problems.is_empty() {
        println!("audit: no networking, no typed-text APIs, macOS {MIN_MACOS} on every slice");
        Ok(())
    } else {
        Err(format!("audit of {} failed:\n  {}", exe.display(), problems.join("\n  ")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const IMPORTS: &str =
        "_AudioOutputUnitStart\n_CGEventGetIntegerValueField\n_objc_msgSend\n_write\n";
    const LIBRARIES: &str = "Clicky (architecture arm64):\n\t/System/Library/Frameworks/AppKit.framework/Versions/C/AppKit (compatibility version 45.0.0)\n\t/usr/lib/libSystem.B.dylib (compatibility version 1.0.0)\n";
    const BUILD: &str = "Clicky (architecture x86_64):\nLoad command 9\n      cmd LC_BUILD_VERSION\n platform MACOS\n    minos 13.0\n      sdk 27.0\nClicky (architecture arm64):\nLoad command 9\n      cmd LC_BUILD_VERSION\n platform MACOS\n    minos 13.0\n      sdk 27.0\n";
    const BINARY: &[u8] = b"\0NSURL\0URLWithString:\0openURL:\0modifierFlags\0KeyReleaseSounds\0";

    fn clean() -> Evidence<'static> {
        Evidence {
            imports: IMPORTS,
            libraries: LIBRARIES,
            entitlements: "",
            build: BUILD,
            binary: BINARY,
        }
    }

    #[test]
    fn a_clean_binary_passes() {
        assert_eq!(check(&clean()), Vec::<String>::new());
    }

    #[test]
    fn networking_imports_fail() {
        for symbol in [
            "_connect",
            "_getaddrinfo",
            "_CFSocketCreate",
            "_nw_connection_start",
            "_OBJC_CLASS_$_NSURLSession",
        ] {
            let imports = format!("{IMPORTS}{symbol}\n");
            let problems = check(&Evidence { imports: &imports, ..clean() });
            assert_eq!(problems.len(), 1, "{symbol}: {problems:?}");
            assert!(problems[0].contains(symbol), "{problems:?}");
        }
    }

    #[test]
    fn typed_text_imports_fail() {
        let imports = format!("{IMPORTS}_CGEventKeyboardGetUnicodeString\n");
        assert_eq!(check(&Evidence { imports: &imports, ..clean() }).len(), 1);
    }

    #[test]
    fn imports_with_a_forbidden_name_inside_pass() {
        // `_connect` must match the whole symbol, not a prefix of it.
        let imports =
            format!("{IMPORTS}_connectx_unrelated\n_sendfile_like\n_OBJC_CLASS_$_NSURL\n");
        assert_eq!(check(&Evidence { imports: &imports, ..clean() }), Vec::<String>::new());
    }

    #[test]
    fn network_libraries_fail() {
        let libraries = format!(
            "{LIBRARIES}\t/System/Library/Frameworks/CFNetwork.framework/Versions/A/CFNetwork (compatibility version 1.0.0)\n"
        );
        let problems = check(&Evidence { libraries: &libraries, ..clean() });
        assert_eq!(problems.len(), 1);
        assert!(problems[0].contains("CFNetwork"), "{problems:?}");
    }

    #[test]
    fn classes_and_selectors_looked_up_by_name_fail() {
        for name in ["NSURLSession", "charactersIgnoringModifiers", "characters"] {
            // Rust packs its strings without separators; the C string ends with the name.
            let binary = [BINARY, b"Variation", name.as_bytes(), b"\0"].concat();
            let problems = check(&Evidence { binary: &binary, ..clean() });
            assert_eq!(problems.len(), 1, "{name}: {problems:?}");
            assert!(problems[0].contains(name), "{problems:?}");
        }
    }

    #[test]
    fn network_entitlements_fail() {
        let entitlements =
            "<plist><dict><key>com.apple.security.network.client</key><true/></dict></plist>";
        let problems = check(&Evidence { entitlements, ..clean() });
        assert_eq!(problems.len(), 1);
        assert!(problems[0].contains("network.client"), "{problems:?}");
    }

    #[test]
    fn every_slice_must_target_the_minimum_macos() {
        let build = BUILD.replacen("minos 13.0", "minos 14.0", 1);
        let problems = check(&Evidence { build: &build, ..clean() });
        assert_eq!(problems.len(), 1);
        assert!(problems[0].contains("14.0"), "{problems:?}");
        let problems = check(&Evidence { build: "", ..clean() });
        assert_eq!(problems.len(), 1, "no build version at all: {problems:?}");
    }
}
