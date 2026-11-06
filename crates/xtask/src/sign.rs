//! Code signing per ADR 0002: sign with the local self-signed identity when it
//! exists, so the designated requirement (and with it the Input Monitoring
//! permission) survives rebuilds; otherwise sign ad-hoc and warn.
//!
//! The identity is created once by `scripts/create-signing-identity.sh`.

use std::path::Path;
use std::process::Command;

use crate::cmd::{output, run};

/// Common name of the self-signed code-signing certificate.
pub const IDENTITY_NAME: &str = "Clicky Local Signing";
/// Overrides [`IDENTITY_NAME`] (a name or a SHA-1 hash).
pub const IDENTITY_ENV: &str = "CLICKY_SIGNING_IDENTITY";

/// How the bundle ended up signed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Signature {
    /// Signed with the stable local identity (SHA-1 of its certificate).
    Identity { name: String, sha1: String },
    /// Signed ad-hoc: macOS will forget Input Monitoring after each rebuild.
    AdHoc,
}

/// The SHA-1 of the first identity called `name` in the output of
/// `security find-identity -p codesigning`. Untrusted identities count too:
/// codesign accepts them, and the designated requirement pins the leaf hash.
pub fn find_identity(listing: &str, name: &str) -> Option<String> {
    let quoted = format!("\"{name}\"");
    listing
        .lines()
        .filter(|line| line.contains(&quoted))
        .filter_map(|line| line.split_whitespace().nth(1))
        .find(|hash| is_sha1(hash))
        .map(str::to_owned)
}

/// Whether `text` is a SHA-1 hash: 40 hex digits.
fn is_sha1(text: &str) -> bool {
    text.len() == 40 && text.chars().all(|c| c.is_ascii_hexdigit())
}

/// The SHA-1 to sign with, from the identity listing (or the failure to get
/// it) and the identity `name` asked for, which may itself be a hash. `Err`
/// is the warning to print before falling back to ad-hoc.
fn choose_identity(listing: crate::Result<String>, name: &str) -> Result<String, String> {
    if is_sha1(name) {
        return Ok(name.to_owned());
    }
    match listing {
        Ok(listing) => find_identity(&listing, name)
            .ok_or_else(|| format!("no code-signing identity named {name:?} in the keychain")),
        Err(e) => Err(format!("could not list the keychain's code-signing identities ({e})")),
    }
}

/// Signs `app` with the local identity, or ad-hoc when it is missing, when
/// `adhoc` is set or when signing with it fails.
pub fn sign(app: &Path, adhoc: bool) -> crate::Result<Signature> {
    let name = std::env::var(IDENTITY_ENV).unwrap_or_else(|_| IDENTITY_NAME.to_owned());
    if !adhoc {
        let listing = output(Command::new("security").args(["find-identity", "-p", "codesigning"]));
        match choose_identity(listing, &name) {
            Ok(sha1) => match codesign(app, &sha1) {
                Ok(()) => return Ok(Signature::Identity { name, sha1 }),
                Err(e) => eprintln!("warning: signing with {name:?} failed ({e})"),
            },
            Err(warning) => eprintln!("warning: {warning}"),
        }
    }
    codesign(app, "-")?;
    Ok(Signature::AdHoc)
}

/// What to tell the developer about the signature.
pub fn describe(signature: &Signature) -> String {
    match signature {
        Signature::Identity { name, sha1 } => format!("signed with {name:?} ({sha1})"),
        Signature::AdHoc => "signed AD-HOC. WARNING: macOS ties Input Monitoring to the \
             signature, so it will forget the permission after every rebuild. Run \
             scripts/create-signing-identity.sh once to create the stable local identity \
             (ADR 0002)."
            .to_owned(),
    }
}

fn codesign(app: &Path, identity: &str) -> crate::Result<()> {
    run(Command::new("codesign")
        .args(["--force", "--timestamp=none", "--sign", identity])
        .arg(app))?;
    run(Command::new("codesign").args(["--verify", "--strict"]).arg(app))
}

#[cfg(test)]
mod tests {
    use super::*;

    const LISTING: &str = r#"
Policy: Code Signing
  Matching identities
  1) 0123456789ABCDEF0123456789ABCDEF01234567 "Apple Development: someone@example.com (ABCDE12345)"
  2) 89ABCDEF0123456789ABCDEF0123456789ABCDEF "Clicky Local Signing" (CSSMERR_TP_NOT_TRUSTED)
     2 identities found

  Valid identities only
  1) 0123456789ABCDEF0123456789ABCDEF01234567 "Apple Development: someone@example.com (ABCDE12345)"
     1 valid identities found
"#;

    #[test]
    fn finds_the_local_identity_even_when_untrusted() {
        assert_eq!(
            find_identity(LISTING, IDENTITY_NAME).as_deref(),
            Some("89ABCDEF0123456789ABCDEF0123456789ABCDEF")
        );
    }

    #[test]
    fn a_missing_identity_means_ad_hoc() {
        assert_eq!(find_identity(LISTING, "Someone Else"), None);
        assert_eq!(find_identity("     0 identities found\n", IDENTITY_NAME), None);
        assert_eq!(find_identity(LISTING, "Clicky"), None, "names must match exactly");
    }

    #[test]
    fn a_sha1_is_forty_hex_digits() {
        assert!(is_sha1("89ABCDEF0123456789ABCDEF0123456789ABCDEF"));
        assert!(is_sha1("89abcdef0123456789abcdef0123456789abcdef"));
        assert!(!is_sha1("89ABCDEF0123456789ABCDEF0123456789ABCDE"), "39 digits");
        assert!(!is_sha1("89ABCDEF0123456789ABCDEF0123456789ABCDEG"), "not hex");
        assert!(!is_sha1(IDENTITY_NAME));
    }

    #[test]
    fn a_failed_identity_lookup_is_told_apart_from_a_missing_identity() {
        let sha1 = "89ABCDEF0123456789ABCDEF0123456789ABCDEF";
        assert_eq!(choose_identity(Ok(LISTING.to_owned()), IDENTITY_NAME), Ok(sha1.to_owned()));

        let missing = choose_identity(Ok(String::new()), IDENTITY_NAME).unwrap_err();
        assert!(missing.contains("no code-signing identity named"), "{missing}");

        let failed =
            choose_identity(Err("security failed with exit status: 51".into()), IDENTITY_NAME)
                .unwrap_err();
        assert!(
            failed.contains("could not list") && failed.contains("exit status: 51"),
            "{failed}"
        );
        assert!(!failed.contains("no code-signing identity named"), "{failed}");

        // A hash given directly still signs, listing or not.
        assert_eq!(choose_identity(Err("x".into()), sha1), Ok(sha1.to_owned()));
        assert_eq!(choose_identity(Ok(String::new()), sha1), Ok(sha1.to_owned()));
    }

    #[test]
    fn the_ad_hoc_description_warns_about_the_permission() {
        let text = describe(&Signature::AdHoc);
        assert!(text.contains("WARNING") && text.contains("Input Monitoring"), "{text}");
        assert!(text.contains("create-signing-identity.sh"), "{text}");
    }
}
