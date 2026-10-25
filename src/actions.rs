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
    PaneZoomIn,
    PaneZoomOut,
    PaneZoomReset,
}
pub const ACTIONS: [(Action, &str, &str); 19] = [
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
    (
        Action::PaneZoomIn,
        "Increase pane font size",
        "Ctrl+Shift+=",
    ),
    (
        Action::PaneZoomOut,
        "Decrease pane font size",
        "Ctrl+Shift+-",
    ),
    (
        Action::PaneZoomReset,
        "Reset pane font size",
        "Ctrl+Shift+0",
    ),
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
    let alias = "Shift+Insert";
    if !overrides.contains_key(&Action::Paste) && seen.contains(&parse(alias)?) {
        bail!("Shortcut conflicts with default alias: {alias}; override or disable Paste first");
    }
    Ok(())
}
fn name(key: &Key) -> Option<String> {
    match key {
        Key::Character(s) => Some(s.to_ascii_lowercase()),
        Key::Named(k) => Some(format!("{k:?}").to_ascii_lowercase()),
        _ => None,
    }
}
/// `unshifted` is the key without modifiers, so "Ctrl+Shift+=" matches although
/// Shift turns `=` into `+` on the logical key.
pub fn resolve(
    key: &Key,
    unshifted: &Key,
    modifiers: ModifiersState,
    overrides: &BTreeMap<Action, String>,
) -> Option<Action> {
    let names = [name(key)?, name(unshifted).unwrap_or_default()];
    if *key == Key::Named(NamedKey::Insert)
        && modifiers == ModifiersState::SHIFT
        && !overrides.contains_key(&Action::Paste)
    {
        return Some(Action::Paste);
    }
    ACTIONS.iter().find_map(|(action, _, _)| {
        let chord = parse(label(*action, overrides)).ok()?;
        (names.contains(&chord.key) && chord.modifiers == modifiers).then_some(*action)
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
            resolve(&key, &key, ModifiersState::ALT, &map),
            Some(Action::NewTab)
        );
        assert_eq!(
            resolve(
                &key,
                &key,
                ModifiersState::CONTROL | ModifiersState::SHIFT,
                &map
            ),
            None
        );
        map.insert(Action::Paste, "Alt+t".into());
        assert!(validate(&map).is_err());
        map.insert(Action::Paste, String::new());
        validate(&map).unwrap();
    }
    #[test]
    fn shift_selects_pane_zoom_over_window_zoom() {
        let none = BTreeMap::new();
        let ctrl = ModifiersState::CONTROL;
        let equals = Key::Character("=".into());
        let plus = Key::Character("+".into());
        assert_eq!(resolve(&equals, &equals, ctrl, &none), Some(Action::ZoomIn));
        // Shift turns `=` into `+`; the unshifted key still identifies the chord.
        assert_eq!(
            resolve(&plus, &equals, ctrl | ModifiersState::SHIFT, &none),
            Some(Action::PaneZoomIn)
        );
        let paren = Key::Character(")".into());
        let zero = Key::Character("0".into());
        assert_eq!(
            resolve(&paren, &zero, ctrl | ModifiersState::SHIFT, &none),
            Some(Action::PaneZoomReset)
        );
        assert_eq!(resolve(&zero, &zero, ctrl, &none), Some(Action::ZoomReset));
    }
}
