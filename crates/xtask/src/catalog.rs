//! `assets/soundpacks/catalog.json`: which soundpacks ship, in which order,
//! and where their original recordings live.

use std::path::{Path, PathBuf};

use serde_json::Value;

/// Switch types, in menu order.
pub const SWITCH_TYPES: [&str; 3] = ["linear", "tactile", "clicky"];

/// The layout of a soundpack's original recordings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceKind {
    /// A Mechvibes `config.json` plus one sprite file.
    Mechvibes,
}

#[derive(Clone, Debug)]
pub struct Entry {
    pub id: String,
    pub name: String,
    pub variant: String,
    pub switch_type: String,
    pub credit: String,
    pub kind: SourceKind,
    pub dir: PathBuf,
}

#[derive(Clone, Debug)]
pub struct Catalog {
    pub entries: Vec<Entry>,
}

pub fn load(path: &Path) -> crate::Result<Catalog> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    parse(&text, path.parent().unwrap_or(Path::new(".")))
}

/// Parses catalog JSON; source directories are resolved against `base`.
pub fn parse(text: &str, base: &Path) -> crate::Result<Catalog> {
    let root: Value = serde_json::from_str(text).map_err(|e| format!("catalog: {e}"))?;
    let packs =
        root.get("packs").and_then(Value::as_array).ok_or("catalog: missing \"packs\" array")?;
    let mut entries: Vec<Entry> = Vec::new();
    for pack in packs {
        let id = string(pack, "id", "catalog pack")?;
        let ctx = format!("pack {id:?}");
        if entries.iter().any(|e| e.id == id) {
            return Err(format!("{ctx}: duplicate id"));
        }
        if id.is_empty()
            || !id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        {
            return Err(format!("{ctx}: ids may only contain a-z, 0-9 and '-'"));
        }
        let switch_type = string(pack, "switch_type", &ctx)?;
        if !SWITCH_TYPES.contains(&switch_type.as_str()) {
            return Err(format!("{ctx}: unknown switch type {switch_type:?}"));
        }
        let source = pack.get("source").ok_or_else(|| format!("{ctx}: missing source"))?;
        let kind = match string(source, "kind", &ctx)?.as_str() {
            "mechvibes" => SourceKind::Mechvibes,
            other => return Err(format!("{ctx}: unknown source kind {other:?}")),
        };
        entries.push(Entry {
            name: string(pack, "name", &ctx)?,
            variant: pack.get("variant").and_then(Value::as_str).unwrap_or_default().to_owned(),
            credit: string(pack, "credit", &ctx)?,
            dir: base.join(string(source, "dir", &ctx)?),
            id,
            switch_type,
            kind,
        });
    }
    if entries.is_empty() {
        return Err("catalog: no packs".to_owned());
    }
    Ok(Catalog { entries })
}

fn string(value: &Value, key: &str, ctx: &str) -> crate::Result<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| format!("{ctx}: missing string field {key:?}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r##"{ "packs": [
      {"id":"a","name":"A","switch_type":"linear","credit":"X","source":{"kind":"mechvibes","dir":"mechvibes/a"}},
      {"id":"b","name":"B","variant":"PBT","switch_type":"clicky","credit":"Y","source":{"kind":"mechvibes","dir":"mechvibes/b"}}
    ] }"##;

    #[test]
    fn parses_entries_in_order() {
        let catalog = parse(SAMPLE, Path::new("/assets")).unwrap();
        assert_eq!(catalog.entries.len(), 2);
        assert_eq!(catalog.entries[0].dir, Path::new("/assets/mechvibes/a"));
        assert_eq!(catalog.entries[0].variant, "");
        assert_eq!(catalog.entries[1].variant, "PBT");
        assert_eq!(catalog.entries[1].switch_type, "clicky");
        assert_eq!(catalog.entries[1].kind, SourceKind::Mechvibes);
    }

    #[test]
    fn an_empty_catalog_is_an_error() {
        let err = parse(r#"{ "packs": [] }"#, Path::new("/")).unwrap_err();
        assert!(err.contains("no packs"), "{err}");
    }

    #[test]
    fn rejects_bad_catalogs() {
        let cases = [
            ("\"switch_type\":\"linear\"", "\"switch_type\":\"mushy\"", "unknown switch type"),
            ("\"id\":\"b\"", "\"id\":\"a\"", "duplicate"),
            ("\"id\":\"b\"", "\"id\":\"B!\"", "a-z"),
            ("\"kind\":\"mechvibes\"", "\"kind\":\"other\"", "unknown source kind"),
        ];
        for (needle, replacement, expected) in cases {
            let text = SAMPLE.replacen(needle, replacement, 1);
            let err = parse(&text, Path::new("/")).unwrap_err();
            assert!(err.contains(expected), "{needle} → {err}");
        }
    }

    #[test]
    fn shipped_catalog_points_at_committed_sources() {
        let path = crate::workspace_root().join("assets/soundpacks/catalog.json");
        let catalog = load(&path).unwrap();
        assert!(catalog.entries.iter().any(|e| e.id == "cherry-mx-brown-pbt"));
        for entry in &catalog.entries {
            assert!(entry.dir.is_dir(), "{} is missing {}", entry.id, entry.dir.display());
        }
    }
}
