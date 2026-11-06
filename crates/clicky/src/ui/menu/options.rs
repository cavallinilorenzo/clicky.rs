//! The sound options, right after the volume slider: Key Release Sounds and
//! Spatial Stereo, each a checkbox item. (Natural Variation has no item: it is
//! always on.) A click flips
//! the option in the engine at once (the next key uses it) and saves it.
//!
//! Key Release Sounds is greyed out for a soundpack recorded without release
//! sounds, with the reason as the item's subtitle (macOS 14.4+) or as a
//! note row below it (macOS 13–14.3), and in its tooltip. The choice itself is
//! kept: switching to a soundpack with release sounds brings it back. The
//! items are refreshed each time the menu opens, so they follow a soundpack
//! picked since.

use std::rc::Rc;

use objc2::available;
use objc2::rc::Retained;
use objc2_app_kit::NSMenuItem;
use objc2_foundation::NSString;

use super::{MenuBuilder, control_state};
use crate::engine::Shared;
use crate::engine::options::{RELEASE_SOUNDS, SPATIAL_STEREO};
use crate::packs::{self, Entry};
use crate::settings::{Setting, Store};

/// Why Key Release Sounds is unavailable.
pub const NO_RELEASE_SOUNDS: &str = "This soundpack has no release sounds";

/// One of the two options.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SoundOption {
    ReleaseSounds,
    SpatialStereo,
}

impl SoundOption {
    pub const ALL: [SoundOption; 2] = [SoundOption::ReleaseSounds, SoundOption::SpatialStereo];

    pub fn title(self) -> &'static str {
        match self {
            SoundOption::ReleaseSounds => "Key Release Sounds",
            SoundOption::SpatialStereo => "Spatial Stereo",
        }
    }

    fn tooltip(self) -> &'static str {
        match self {
            SoundOption::ReleaseSounds => "Play a key's release sound when it comes back up",
            SoundOption::SpatialStereo => {
                "Place each key's sound left or right by where it sits on the keyboard (best on headphones)"
            }
        }
    }

    fn setting(self) -> &'static Setting<bool> {
        match self {
            SoundOption::ReleaseSounds => &RELEASE_SOUNDS,
            SoundOption::SpatialStereo => &SPATIAL_STEREO,
        }
    }

    /// Whether the option is on in the engine (the user's choice).
    pub fn is_on(self, shared: &Shared) -> bool {
        match self {
            SoundOption::ReleaseSounds => shared.release_sounds(),
            SoundOption::SpatialStereo => shared.spatial(),
        }
    }

    /// Turns the option on or off in the engine and saves it.
    pub fn set(self, on: bool, shared: &Shared, store: &Store) {
        match self {
            SoundOption::ReleaseSounds => shared.set_release_sounds(on),
            SoundOption::SpatialStereo => shared.set_spatial(on),
        }
        store.set(self.setting(), &on);
    }
}

/// How an option's item looks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct View {
    pub checked: bool,
    pub enabled: bool,
    /// Why it is unavailable, if it is.
    pub reason: Option<&'static str>,
}

/// The item for `option` given the engine's state and whether the current
/// soundpack has release sounds. Key Release Sounds is unavailable (and shown
/// unchecked, since it has no effect) while it has none.
pub fn view(option: SoundOption, shared: &Shared, pack_has_release: bool) -> View {
    let on = option.is_on(shared);
    if option == SoundOption::ReleaseSounds && !pack_has_release {
        return View { checked: false, enabled: false, reason: Some(NO_RELEASE_SOUNDS) };
    }
    View { checked: on, enabled: true, reason: None }
}

/// Whether the engine's current soundpack has release sounds.
fn current_has_release(shared: &Shared) -> bool {
    let library = packs::library();
    shared.pack().and_then(|pack| library.get(&pack.meta.id)).is_some_and(Entry::has_release)
}

/// One option's menu rows: its checkbox item and, before macOS 14.4 (no
/// subtitles), a note row for the reason it is unavailable.
struct Row {
    option: SoundOption,
    item: Retained<NSMenuItem>,
    note: Option<Retained<NSMenuItem>>,
}

impl Row {
    fn show(&self, shared: &Shared, pack_has_release: bool) {
        let view = view(self.option, shared, pack_has_release);
        self.item.setState(control_state(view.checked));
        self.item.setEnabled(view.enabled);
        let tooltip = view.reason.unwrap_or(self.option.tooltip());
        self.item.setToolTip(Some(&NSString::from_str(tooltip)));
        let reason = view.reason.map(NSString::from_str);
        match &self.note {
            Some(note) => {
                note.setHidden(reason.is_none());
                if let Some(reason) = &reason {
                    note.setTitle(reason);
                }
            }
            None => {
                if available!(macos = 14.4) {
                    self.item.setSubtitle(reason.as_deref());
                }
            }
        }
    }
}

pub fn add(menu: &mut MenuBuilder) {
    let mtm = menu.mtm();
    let subtitles = available!(macos = 14.4);
    menu.separator();
    let mut rows = Vec::new();
    for option in SoundOption::ALL {
        let item = NSMenuItem::new(mtm);
        item.setTitle(&NSString::from_str(option.title()));
        menu.add(&item);
        let note = (option == SoundOption::ReleaseSounds && !subtitles).then(|| {
            let note = NSMenuItem::new(mtm);
            note.setEnabled(false);
            note.setIndentationLevel(1);
            note.setHidden(true);
            menu.add(&note);
            note
        });
        rows.push(Row { option, item, note });
    }
    let rows = Rc::new(rows);
    let shared = menu.ui().shared;
    let refresh = {
        let rows = rows.clone();
        Rc::new(move || {
            let has_release = current_has_release(shared);
            rows.iter().for_each(|row| row.show(shared, has_release));
        })
    };
    for row in rows.iter() {
        let (ui, refresh, option) = (menu.ui().clone(), refresh.clone(), row.option);
        menu.action(move |_| {
            option.set(!option.is_on(ui.shared), ui.shared, &ui.store);
            refresh();
        })
        .attach_item(&row.item);
    }
    refresh();
    menu.on_open(move || refresh());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::options;

    #[test]
    fn key_release_sounds_is_greyed_out_with_a_reason_for_packs_without_them() {
        let shared = Shared::new(true);
        let release = view(SoundOption::ReleaseSounds, &shared, false);
        assert_eq!(
            release,
            View { checked: false, enabled: false, reason: Some(NO_RELEASE_SOUNDS) }
        );
        let spatial = view(SoundOption::SpatialStereo, &shared, false);
        assert!(spatial.enabled && spatial.checked, "the other stays available");
    }

    #[test]
    fn key_release_sounds_comes_back_with_a_pack_that_has_them() {
        let shared = Shared::new(true);
        shared.set_release_sounds(true);
        assert!(!view(SoundOption::ReleaseSounds, &shared, false).enabled);
        assert_eq!(
            view(SoundOption::ReleaseSounds, &shared, true),
            View { checked: true, enabled: true, reason: None },
            "the choice was kept"
        );
        shared.set_release_sounds(false);
        let off = view(SoundOption::ReleaseSounds, &shared, true);
        assert!(off.enabled && !off.checked);
    }

    #[test]
    fn toggling_an_option_takes_effect_and_is_saved() {
        let shared = Shared::new(true);
        let store = Store::in_memory();
        options::restore(&shared, &store);
        for option in SoundOption::ALL {
            assert!(option.is_on(&shared), "{option:?} is on by default");
            option.set(false, &shared, &store);
            assert!(!option.is_on(&shared));
            assert!(!view(option, &shared, true).checked);
        }
        let relaunched = Shared::new(true);
        options::restore(&relaunched, &store);
        assert!(SoundOption::ALL.iter().all(|o| !o.is_on(&relaunched)), "saved");
        SoundOption::SpatialStereo.set(true, &shared, &store);
        let relaunched = Shared::new(true);
        options::restore(&relaunched, &store);
        assert!(relaunched.spatial() && !relaunched.release_sounds());
    }
}
