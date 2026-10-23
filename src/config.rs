use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::{
    io::Read,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub font_family: String,
    pub font_size: f32,
    pub line_spacing: f32,
    pub padding: f32,
    pub scrollback: usize,
    pub copy_on_select: bool,
    pub cursor_shape: CursorShape,
    pub cursor_blink: bool,
    pub foreground: String,
    pub background: String,
    pub accent: String,
    pub shell: Option<String>,
    pub keybindings: std::collections::BTreeMap<crate::actions::Action, String>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CursorShape {
    #[default]
    Block,
    Beam,
    Underline,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            font_family: "monospace".into(),
            font_size: 15.0,
            line_spacing: 1.3,
            padding: 14.0,
            scrollback: 10_000,
            copy_on_select: false,
            cursor_shape: CursorShape::Block,
            cursor_blink: false,
            foreground: "#d8dee9".into(),
            background: "#171b24".into(),
            accent: "#80cbc4".into(),
            shell: None,
            keybindings: Default::default(),
        }
    }
}

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        let file = match std::fs::File::open(path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default());
            }
            Err(error) => return Err(error).context("Cannot read configuration"),
        };
        let mut source = String::new();
        file.take(65537)
            .read_to_string(&mut source)
            .context("Cannot read configuration")?;
        Self::parse(&source).with_context(|| format!("Invalid configuration: {}", path.display()))
    }

    pub fn parse(source: &str) -> Result<Self> {
        anyhow::ensure!(source.len() <= 65536, "Configuration exceeds 64 KiB");
        let config: Self = toml::from_str(source)?;
        if !(6.0..=72.0).contains(&config.font_size)
            || !(1.0..=3.0).contains(&config.line_spacing)
            || !(0.0..=100.0).contains(&config.padding)
            || config.scrollback > 100_000
            || config.font_family.is_empty()
        {
            bail!(
                "Expected font_size 6–72, line_spacing 1–3, padding 0–100, scrollback ≤100000, and a font family"
            );
        }
        for color in [&config.foreground, &config.background, &config.accent] {
            parse_color(color)?;
        }
        crate::actions::validate(&config.keybindings)?;
        Ok(config)
    }
}

pub fn path() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config")
        })
        .join("vibeterm/config.toml")
}

pub fn parse_color(value: &str) -> Result<[u8; 3]> {
    let hex = value.strip_prefix('#').unwrap_or_default();
    if hex.len() != 6 || !hex.is_ascii() {
        bail!("Color must be #RRGGBB: {value}");
    }
    Ok([
        u8::from_str_radix(&hex[0..2], 16)?,
        u8::from_str_radix(&hex[2..4], 16)?,
        u8::from_str_radix(&hex[4..6], 16)?,
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_bad_settings_and_accepts_partial_config() {
        assert_eq!(Config::parse("font_size = 18").unwrap().font_size, 18.0);
        for input in [
            "font_size = nan",
            "line_spacing = 0",
            "scrollback = 100001",
            "background = 'red'",
            "unknown = 1",
        ] {
            assert!(Config::parse(input).is_err(), "{input}");
        }
    }
}
