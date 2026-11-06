//! The real menu, as `Clicky --menu` builds it, is in the order of issue #2.

use std::process::Command;

/// The menu's sections in the order issue #2 asks for.
const ORDER: [&str; 9] = [
    "on/off switch",
    "soundpack picker",
    "volume slider",
    "Key Release Sounds",
    "Natural Variation",
    "Spatial Stereo",
    "Launch at Login",
    "About",
    "Quit",
];

/// One menu item as `--menu` prints it.
struct Row {
    hidden: bool,
    text: String,
}

fn menu() -> Vec<Row> {
    let output = Command::new(env!("CARGO_BIN_EXE_clicky")).arg("--menu").output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| match line.strip_prefix("[hidden] ") {
            Some(text) => Row { hidden: true, text: text.to_owned() },
            None => Row { hidden: false, text: line.to_owned() },
        })
        .collect()
}

/// The section a row belongs to, if it is one of the nine.
fn section(text: &str) -> Option<&'static str> {
    Some(match text {
        "<switch>" => "on/off switch",
        "<submenu>" => "soundpack picker",
        "<slider>" => "volume slider",
        "Key Release Sounds" => "Key Release Sounds",
        "Natural Variation" => "Natural Variation",
        "Spatial Stereo" => "Spatial Stereo",
        "Launch at Login" => "Launch at Login",
        "About Clicky" => "About",
        "Quit Clicky" => "Quit",
        _ => return None,
    })
}

#[test]
fn the_visible_menu_is_in_the_order_of_the_spec() {
    let rows = menu();
    let visible: Vec<&str> = rows
        .iter()
        .filter(|row| !row.hidden && row.text != "---")
        .map(|row| row.text.as_str())
        .collect();
    let sections: Vec<&str> = visible.iter().filter_map(|text| section(text)).collect();
    assert_eq!(sections, ORDER, "visible rows: {visible:?}");
    // The picker is the "Soundpack" heading and the submenu right under it.
    let picker = visible.iter().position(|t| *t == "<submenu>").unwrap();
    assert_eq!(visible[picker - 1], "Soundpack");
    let unexpected: Vec<&&str> =
        visible.iter().filter(|t| section(t).is_none() && **t != "Soundpack").collect();
    assert!(unexpected.is_empty(), "rows outside the nine sections: {unexpected:?}");
}

/// Rows shown only sometimes (the permission rows, the Option-held
/// Diagnostics Log, the notes under Key Release Sounds and Launch at Login)
/// stay inside the order: nothing before the switch or after Quit, and the
/// Diagnostics Log between About and Quit.
#[test]
fn rows_shown_only_sometimes_keep_the_order() {
    let rows = menu();
    let at = |name: &str| {
        rows.iter().position(|row| section(&row.text) == Some(name)).unwrap_or_else(|| {
            panic!("{name} missing: {:?}", rows.iter().map(|r| &r.text).collect::<Vec<_>>())
        })
    };
    let anchors: Vec<usize> = ORDER.iter().map(|name| at(name)).collect();
    assert!(anchors.windows(2).all(|w| w[0] < w[1]), "{anchors:?}");
    assert_eq!(anchors[0], 0, "the switch comes first");
    assert_eq!(anchors[8], rows.len() - 1, "Quit comes last");
    let diagnostics = rows.iter().position(|row| row.text == "Diagnostics Log").unwrap();
    assert!(rows[diagnostics].hidden, "only shown with Option held");
    assert!(at("About") < diagnostics && diagnostics < at("Quit"));
}
