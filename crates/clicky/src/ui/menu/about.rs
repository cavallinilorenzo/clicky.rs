//! "About Clicky": the standard About panel with the app version and the
//! credits and licences of the soundpack recordings (Mechvibes, kbsim) and of
//! the code Clicky derives from (Rustyvibes 2).
//!
//! Derived from Rustyvibes 2 (MIT, © 2021–2026 Kunal Bagaria,
//! https://github.com/KunalBagaria/rustyvibes-v2), `crates/rustyvibes/src/ui/about.rs`.

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{AnyThread, MainThreadMarker, available};
use objc2_app_kit::{
    NSAboutPanelOptionApplicationVersion, NSAboutPanelOptionCredits, NSApplication, NSColor,
    NSFont, NSFontAttributeName, NSForegroundColorAttributeName, NSMutableParagraphStyle,
    NSParagraphStyleAttributeName, NSTextAlignment,
};
use objc2_foundation::{NSAttributedString, NSDictionary, NSString};

use super::MenuBuilder;
use crate::packs;

/// Where soundpack recordings come from.
struct Source {
    /// What the `credit` of its soundpacks starts with.
    name: &'static str,
    url: &'static str,
    commit: &'static str,
    copyright: &'static str,
}

const MECHVIBES: Source = Source {
    name: "Mechvibes",
    url: "https://github.com/hainguyents13/mechvibes-electron",
    commit: "326252a13e7bef4f1c35d08ef0189b5af6f8ba02",
    copyright: "Copyright (c) 2021 Hai Nguyen",
};

const KBSIM: Source = Source {
    name: "kbsim",
    url: "https://github.com/tplai/kbsim",
    commit: "ba103f3b0afa9dab80447aa2e7e2ed80b6bd80e4",
    copyright: "Copyright (c) Thomas Lai",
};

/// The project Clicky's code derives from.
const RUSTYVIBES_URL: &str = "https://github.com/KunalBagaria/rustyvibes-v2";
const RUSTYVIBES_COPYRIGHT: &str = "Copyright (c) 2021–2026 Kunal Bagaria";

/// The MIT permission notice, identical for all three sources.
const MIT_NOTICE: &str = "Permission is hereby granted, free of charge, to any person obtaining a copy of this software and associated documentation files (the \"Software\"), to deal in the Software without restriction, including without limitation the rights to use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of the Software, and to permit persons to whom the Software is furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED \"AS IS\", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.";

pub fn add(menu: &mut MenuBuilder) {
    menu.separator();
    let mtm = menu.mtm();
    menu.item("About Clicky", move |_| show(mtm));
}

fn show(mtm: MainThreadMarker) {
    let packs: Vec<(String, String)> = packs::library()
        .entries()
        .iter()
        .map(|e| (e.info.meta.credit.clone(), e.title()))
        .collect();
    let credits = attributed(&credits_text(&packs));
    let version = NSString::from_str(env!("CARGO_PKG_VERSION"));
    // SAFETY: framework constants.
    let keys: [&NSString; 2] =
        unsafe { [NSAboutPanelOptionCredits, NSAboutPanelOptionApplicationVersion] };
    let values: [&AnyObject; 2] = [&credits, &version];
    let options = NSDictionary::from_slices(&keys, &values);
    let app = NSApplication::sharedApplication(mtm);
    // A menu bar app is never active on its own: bring the panel to the front.
    if available!(macos = 14.0) {
        app.activate();
    } else {
        #[allow(deprecated)]
        app.activateIgnoringOtherApps(true);
    }
    // SAFETY: the credits option takes an NSAttributedString and the version an NSString.
    unsafe { app.orderFrontStandardAboutPanelWithOptions(&options) };
}

/// Small, secondary, centred text, as About panels use.
fn attributed(text: &str) -> Retained<NSAttributedString> {
    let font = NSFont::systemFontOfSize(11.0);
    let color = NSColor::secondaryLabelColor();
    let paragraph = NSMutableParagraphStyle::new();
    paragraph.setAlignment(NSTextAlignment::Center);
    // SAFETY: framework constants.
    let keys: [&NSString; 3] = unsafe {
        [NSFontAttributeName, NSForegroundColorAttributeName, NSParagraphStyleAttributeName]
    };
    let values: [&AnyObject; 3] = [&font, &color, &paragraph];
    let attributes = NSDictionary::from_slices(&keys, &values);
    // SAFETY: the attribute values have the types AppKit expects for these keys.
    unsafe {
        NSAttributedString::initWithString_attributes(
            NSAttributedString::alloc(),
            &NSString::from_str(text),
            Some(&attributes),
        )
    }
}

/// The credits: each source of recordings with its soundpacks, the code
/// Clicky derives from, then the licence they all share. `packs` are
/// `(credit, title)` pairs in menu order.
fn credits_text(packs: &[(String, String)]) -> String {
    let mut text = String::from(
        "Mechanical keyboard sounds on every key press.\n\
         Never records what you type and never goes online.\n",
    );
    for source in [MECHVIBES, KBSIM] {
        let titles: Vec<&str> = packs
            .iter()
            .filter(|(credit, _)| credit.starts_with(source.name))
            .map(|(_, title)| title.as_str())
            .collect();
        text.push_str(&format!(
            "\nSounds from {}, MIT License\n{}\n{}\ncommit {}\n{}\n",
            source.name,
            source.copyright,
            source.url,
            source.commit,
            titles.join(", "),
        ));
    }
    text.push_str(&format!(
        "\nCode derived from Rustyvibes 2, MIT License\n{RUSTYVIBES_COPYRIGHT}\n{RUSTYVIBES_URL}\n"
    ));
    text.push_str(&format!(
        "\nAll three are licensed under the MIT License:\n\n{MIT_NOTICE}\n\n\
         The licence files are in Clicky.app/Contents/Resources/Licenses.\n"
    ));
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pack(credit: &str, title: &str) -> (String, String) {
        (credit.to_owned(), title.to_owned())
    }

    #[test]
    fn credits_name_each_source_with_its_soundpacks_commit_and_licence() {
        let packs = [
            pack("Mechvibes by Hai Nguyen · MIT License", "Cherry MX Brown · PBT"),
            pack("kbsim by Thomas Lai · MIT License", "Holy Panda"),
            pack("Mechvibes by Hai Nguyen · MIT License", "Everglide Oreo"),
            pack("kbsim by Thomas Lai · MIT License", "Topre"),
        ];
        let text = credits_text(&packs);
        let expected = [
            "Sounds from Mechvibes, MIT License\nCopyright (c) 2021 Hai Nguyen\n\
             https://github.com/hainguyents13/mechvibes-electron\n\
             commit 326252a13e7bef4f1c35d08ef0189b5af6f8ba02\n\
             Cherry MX Brown · PBT, Everglide Oreo\n",
            "Sounds from kbsim, MIT License\nCopyright (c) Thomas Lai\n\
             https://github.com/tplai/kbsim\ncommit ba103f3b0afa9dab80447aa2e7e2ed80b6bd80e4\n\
             Holy Panda, Topre\n",
            "Code derived from Rustyvibes 2, MIT License\n\
             Copyright (c) 2021–2026 Kunal Bagaria\nhttps://github.com/KunalBagaria/rustyvibes-v2\n",
            "Permission is hereby granted, free of charge",
            "Never records what you type",
        ];
        for part in expected {
            assert!(text.contains(part), "missing {part:?} in:\n{text}");
        }
    }

    /// The notice quoted in the panel is the one in the shipped licence files.
    #[test]
    fn the_quoted_licence_matches_the_bundled_licence_files() {
        let flatten = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
        let notice = flatten(MIT_NOTICE);
        for file in [
            include_str!("../../../../../assets/soundpacks/mechvibes/LICENSE"),
            include_str!("../../../../../assets/soundpacks/kbsim/LICENSE.md"),
        ] {
            assert!(flatten(file).contains(&notice), "{file}");
        }
        assert!(
            include_str!("../../../../../assets/soundpacks/mechvibes/LICENSE")
                .contains(MECHVIBES.copyright)
        );
        assert!(
            include_str!("../../../../../assets/soundpacks/kbsim/LICENSE.md")
                .contains(KBSIM.copyright)
        );
        assert!(
            include_str!("../../../../../THIRD_PARTY_NOTICES.md").contains(RUSTYVIBES_COPYRIGHT)
        );
    }
}
