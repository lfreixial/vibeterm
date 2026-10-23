use anyhow::{Result, bail};
use serde::Deserialize;
use std::collections::BTreeMap;
use winit::keyboard::{Key, ModifiersState, NamedKey};

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    NewTab,
    SplitColumns,
    SplitRows,
    ClosePane,
    NextTab,
    PreviousTab,
    NextPane,
    PreviousPane,
    Search,
    Palette,
    Copy,
    Paste,
    Reload,
    ZoomIn,
    ZoomOut,
    ZoomReset,
}
pub const ACTIONS: [(Action, &str, &str); 16] = [
    (Action::NewTab, "New tab", "Ctrl+Shift+t"),
    (Action::SplitColumns, "Split side by side", "Ctrl+Shift+e"),
    (Action::SplitRows, "Split above and below", "Ctrl+Shift+o"),
    (Action::ClosePane, "Close active pane", "Ctrl+Shift+w"),
    (Action::NextTab, "Next tab", "Ctrl+Tab"),
    (Action::PreviousTab, "Previous tab", "Ctrl+Shift+Tab"),
    (Action::NextPane, "Next pane", "Ctrl+Shift+ArrowRight"),
    (
        Action::PreviousPane,
        "Previous pane",
        "Ctrl+Shift+ArrowLeft",
    ),
    (Action::Search, "Search scrollback", "Ctrl+Shift+f"),
    (Action::Palette, "Command palette", "Ctrl+Shift+p"),
    (Action::Copy, "Copy selection", "Ctrl+Shift+c"),
    (Action::Paste, "Paste clipboard", "Ctrl+Shift+v"),
    (Action::Reload, "Reload configuration", "Ctrl+Shift+r"),
    (Action::ZoomIn, "Increase font size", "Ctrl+="),
    (Action::ZoomOut, "Decrease font size", "Ctrl+-"),
    (Action::ZoomReset, "Reset font size", "Ctrl+0"),
];
#[derive(Clone, Debug, PartialEq, Eq)]
struct Chord {
    modifiers: ModifiersState,
    key: String,
}
fn parse(value: &str) -> Result<Chord> {
    let parts: Vec<_> = value.split('+').collect();
    let mut modifiers = ModifiersState::empty();
    for part in &parts[..parts.len() - 1] {
        modifiers |= match part.to_ascii_lowercase().as_str() {
            "ctrl" => ModifiersState::CONTROL,
            "shift" => ModifiersState::SHIFT,
            "alt" => ModifiersState::ALT,
            "super" => ModifiersState::SUPER,
            _ => bail!("Unknown shortcut modifier: {part}"),
        };
    }
    let key = parts.last().unwrap().to_ascii_lowercase();
    if key.chars().count() != 1
        && ![
            "tab",
            "arrowleft",
            "arrowright",
            "arrowup",
            "arrowdown",
            "enter",
            "escape",
            "home",
            "end",
            "pageup",
            "pagedown",
            "insert",
            "delete",
            "f1",
            "f2",
            "f3",
            "f4",
            "f5",
            "f6",
            "f7",
            "f8",
            "f9",
            "f10",
            "f11",
            "f12",
        ]
        .contains(&key.as_str())
    {
        bail!("Invalid shortcut key: {value}");
    }
    if modifiers.is_empty() {
        bail!("Shortcuts must include a modifier: {value}");
    }
    Ok(Chord { modifiers, key })
}
pub fn label(action: Action, overrides: &BTreeMap<Action, String>) -> &str {
    overrides
        .get(&action)
        .map(String::as_str)
        .unwrap_or_else(|| ACTIONS.iter().find(|(a, _, _)| *a == action).unwrap().2)
}
pub fn validate(overrides: &BTreeMap<Action, String>) -> Result<()> {
    let mut seen = Vec::new();
    for (action, _, _) in ACTIONS {
        let value = label(action, overrides);
        if value.is_empty() {
            continue;
        }
        let chord = parse(value)?;
        if seen.contains(&chord) {
            bail!("Duplicate shortcut: {value}");
        }
        seen.push(chord);
    }
    for (action, alias) in [(Action::Paste, "Shift+Insert")] {
        if !overrides.contains_key(&action) && seen.contains(&parse(alias)?) {
            bail!(
                "Shortcut conflicts with default alias: {alias}; override or disable {action:?} first"
            );
        }
    }
    Ok(())
}
pub fn resolve(
    key: &Key,
    modifiers: ModifiersState,
    overrides: &BTreeMap<Action, String>,
) -> Option<Action> {
    let name = match key {
        Key::Character(s) => s.to_ascii_lowercase(),
        Key::Named(k) => format!("{k:?}").to_ascii_lowercase(),
        _ => return None,
    };
    // Preserve the familiar shifted plus key as a zoom alias when defaults are in use.
    if name == "+"
        && modifiers == ModifiersState::CONTROL | ModifiersState::SHIFT
        && !overrides.contains_key(&Action::ZoomIn)
    {
        return Some(Action::ZoomIn);
    }
    if *key == Key::Named(NamedKey::Insert)
        && modifiers == ModifiersState::SHIFT
        && !overrides.contains_key(&Action::Paste)
    {
        return Some(Action::Paste);
    }
    ACTIONS.iter().find_map(|(action, _, _)| {
        let chord = parse(label(*action, overrides)).ok()?;
        (chord.key == name && chord.modifiers == modifiers).then_some(*action)
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn custom_bindings_disable_defaults_and_reject_conflicts() {
        let mut map = BTreeMap::new();
        map.insert(Action::NewTab, "Alt+t".into());
        validate(&map).unwrap();
        let key = Key::Character("t".into());
        assert_eq!(
            resolve(&key, ModifiersState::ALT, &map),
            Some(Action::NewTab)
        );
        assert_eq!(
            resolve(&key, ModifiersState::CONTROL | ModifiersState::SHIFT, &map),
            None
        );
        map.insert(Action::Paste, "Alt+t".into());
        assert!(validate(&map).is_err());
        map.insert(Action::Paste, String::new());
        validate(&map).unwrap();
    }
}
