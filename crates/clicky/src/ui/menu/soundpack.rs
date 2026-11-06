//! The soundpack picker: a "Soundpack" header, then one item named after the
//! current soundpack whose submenu lists every bundled soundpack under the
//! headers Linear, Tactile and Clicky, with a checkmark on the current one.
//!
//! Picking a soundpack switches the engine to it (the next key press uses it;
//! voices already playing finish from the old one, so there is no pop), saves
//! it, and plays a short preview.
//!
//! Derived from Rustyvibes 2 (MIT, © 2021–2026 Kunal Bagaria,
//! https://github.com/KunalBagaria/rustyvibes-v2), `crates/rustyvibes/src/ui/menu.rs`.

use std::rc::Rc;

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{AnyThread, MainThreadMarker, available};
use objc2_app_kit::{NSFont, NSFontAttributeName, NSMenu, NSMenuItem};
use objc2_foundation::{NSAttributedString, NSDictionary, NSString};

use super::{MenuBuilder, control_state};
use crate::diagnostics::{self, Event};
use crate::engine::{Shared, preview};
use crate::packs::{self, Entry, SELECTED_PACK};
use crate::settings::Store;

pub fn add(menu: &mut MenuBuilder) {
    let mtm = menu.mtm();
    let library = packs::library();
    menu.separator();
    menu.add(&section_header(mtm, "Soundpack"));

    let submenu = NSMenu::new(mtm);
    submenu.setAutoenablesItems(false);
    let mut rows: Vec<(&'static Entry, Retained<NSMenuItem>)> = Vec::new();
    for (index, (switch_type, entries)) in library.groups().into_iter().enumerate() {
        if index > 0 {
            submenu.addItem(&NSMenuItem::separatorItem(mtm));
        }
        submenu.addItem(&section_header(mtm, switch_type.label()));
        for entry in entries {
            let row = NSMenuItem::new(mtm);
            row.setTitle(&NSString::from_str(&entry.title()));
            submenu.addItem(&row);
            rows.push((entry, row));
        }
    }
    if rows.is_empty() {
        let none = NSMenuItem::new(mtm);
        none.setTitle(&NSString::from_str("No soundpacks found"));
        none.setEnabled(false);
        submenu.addItem(&none);
    }

    let current = NSMenuItem::new(mtm);
    current.setSubmenu(Some(&submenu));
    menu.add(&current);

    let rows = Rc::new(rows);
    let shared = menu.ui().shared;
    let refresh = {
        let rows = rows.clone();
        move || show_current(shared, &current, &rows)
    };
    let refresh = Rc::new(refresh);
    for (entry, row) in rows.iter() {
        let ui = menu.ui().clone();
        let refresh = refresh.clone();
        let entry: &'static Entry = entry;
        menu.action(move |_| {
            if choose(entry, ui.shared, &ui.store).is_ok() {
                preview::play();
            }
            refresh();
        })
        .attach_item(row);
    }
    refresh();
    menu.on_open(move || refresh());
}

/// Switches to `entry` and saves it as the chosen soundpack. On failure (its
/// file is unreadable) the current soundpack stays and the reason is logged.
fn choose(entry: &Entry, shared: &Shared, store: &Store) -> Result<(), String> {
    let first_use = !entry.is_loaded();
    match entry.load() {
        Ok(pack) => {
            if first_use {
                diagnostics::log(Event::SoundpackLoaded {
                    id: pack.meta.id.clone(),
                    clips: pack.clip_count(),
                    sample_rate: pack.sample_rate,
                });
            }
            shared.set_pack(pack);
            store.set(&SELECTED_PACK, &entry.id().to_owned());
            Ok(())
        }
        Err(detail) => {
            diagnostics::log(Event::Error { context: "soundpack", detail: detail.clone() });
            Err(detail)
        }
    }
}

/// Names the current soundpack and checks its row.
fn show_current(shared: &Shared, current: &NSMenuItem, rows: &[(&Entry, Retained<NSMenuItem>)]) {
    let id = shared.pack().map(|pack| pack.meta.id.as_str());
    let mut title = "No soundpacks found".to_owned();
    for (entry, row) in rows {
        let on = Some(entry.id()) == id;
        row.setState(control_state(on));
        if on {
            title = entry.title();
        }
    }
    current.setTitle(&NSString::from_str(&title));
}

/// A section title: native on macOS 14+, a disabled bold item on macOS 13.
fn section_header(mtm: MainThreadMarker, title: &str) -> Retained<NSMenuItem> {
    if available!(macos = 14.0) {
        return NSMenuItem::sectionHeaderWithTitle(&NSString::from_str(title), mtm);
    }
    let font = NSFont::boldSystemFontOfSize(11.0);
    // SAFETY: an immutable framework constant.
    let key: &NSString = unsafe { NSFontAttributeName };
    let value: &AnyObject = &font;
    let attributes = NSDictionary::from_slices(&[key], &[value]);
    // SAFETY: the attributes map a valid key to an NSFont.
    let text = unsafe {
        NSAttributedString::initWithString_attributes(
            NSAttributedString::alloc(),
            &NSString::from_str(title),
            Some(&attributes),
        )
    };
    let item = NSMenuItem::new(mtm);
    item.setAttributedTitle(Some(&text));
    item.setEnabled(false);
    item
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::packs::Library;
    use crate::packs::tests::{temp_dir, write_pack};

    #[test]
    fn choosing_a_soundpack_switches_the_engine_and_is_saved() {
        let dir = temp_dir("choose");
        write_pack(&dir, "red", "linear", 0, false);
        write_pack(&dir, "blue", "clicky", 1, false);
        let library = Library::scan(std::slice::from_ref(&dir));
        let shared = Shared::new(true);
        let store = Store::in_memory();

        choose(library.get("blue").unwrap(), &shared, &store).unwrap();
        assert_eq!(shared.pack().map(|p| p.meta.id.as_str()), Some("blue"));
        assert_eq!(store.get(&SELECTED_PACK), "blue");
        assert_eq!(
            library.load_saved(&store.get(&SELECTED_PACK)).0.map(|(e, _)| e.id()),
            Some("blue")
        );
    }

    #[test]
    fn a_soundpack_that_fails_to_load_leaves_the_current_one() {
        let dir = temp_dir("choose-broken");
        write_pack(&dir, "red", "linear", 0, false);
        write_pack(&dir, "blue", "clicky", 1, false);
        let library = Library::scan(std::slice::from_ref(&dir));
        let shared = Shared::new(true);
        let store = Store::in_memory();
        choose(library.get("red").unwrap(), &shared, &store).unwrap();
        std::fs::write(dir.join("blue.clickypack"), b"gone").unwrap();

        assert!(choose(library.get("blue").unwrap(), &shared, &store).is_err());
        assert_eq!(shared.pack().map(|p| p.meta.id.as_str()), Some("red"));
        assert_eq!(store.get(&SELECTED_PACK), "red");
    }
}
