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
    menu_with(&[])
}

/// The menu as `--menu <args>` prints it.
fn menu_with(args: &[&str]) -> Vec<Row> {
    let output =
        Command::new(env!("CARGO_BIN_EXE_clicky")).arg("--menu").args(args).output().unwrap();
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

/// The permission rows, shown above the switch only while sounds are unavailable.
const PERMISSION_ROWS: [&str; 3] = [
    "Sounds unavailable",
    "Clicky needs Input Monitoring to notice key presses",
    "Set Up Input Monitoring…",
];

/// Rows shown only sometimes (the permission rows, the Option-held
/// Diagnostics Log, the notes under Key Release Sounds and Launch at Login)
/// stay outside the nine: only the permission rows (and their separator)
/// come before the switch, nothing after Quit, and the Diagnostics Log
/// between About and Quit.
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
    let above: Vec<&Row> = rows[..anchors[0]].iter().collect();
    assert!(
        !above.is_empty() && above.iter().all(|row| row.hidden),
        "only the hidden permission rows come before the switch: {:?}",
        above.iter().map(|r| &r.text).collect::<Vec<_>>()
    );
    assert_eq!(anchors[8], rows.len() - 1, "Quit comes last");
    let diagnostics = rows.iter().position(|row| row.text == "Diagnostics Log").unwrap();
    assert!(rows[diagnostics].hidden, "only shown with Option held");
    assert!(at("About") < diagnostics && diagnostics < at("Quit"));
}

/// Without Input Monitoring, the permission rows lead the menu, above the
/// switch, and the nine sections keep their order with nothing between them.
/// (`--needs-permission` only pretends the permission is missing; it never
/// reads or changes the real permission.)
#[test]
fn without_permission_the_fix_comes_first_and_the_nine_keep_their_order() {
    let rows = menu_with(&["--needs-permission"]);
    let visible: Vec<&str> = rows
        .iter()
        .filter(|row| !row.hidden && row.text != "---")
        .map(|row| row.text.as_str())
        .collect();
    let switch = visible.iter().position(|t| *t == "<switch>").expect("the switch is shown");
    assert_eq!(visible[..switch], PERMISSION_ROWS, "visible rows: {visible:?}");
    let below = &visible[switch..];
    let sections: Vec<&str> = below.iter().filter_map(|text| section(text)).collect();
    assert_eq!(sections, ORDER, "visible rows: {visible:?}");
    let unexpected: Vec<&&str> =
        below.iter().filter(|t| section(t).is_none() && **t != "Soundpack").collect();
    assert!(unexpected.is_empty(), "rows among the nine sections: {unexpected:?}");
    let first_separator = rows.iter().position(|row| row.text == "---").unwrap();
    assert!(
        first_separator < rows.iter().position(|row| row.text == "<switch>").unwrap(),
        "a separator divides the permission rows from the switch"
    );
}
