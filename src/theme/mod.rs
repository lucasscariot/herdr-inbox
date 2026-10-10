//! Colors. The inbox follows the user's Herdr theme: the same built-in
//! palettes, `[theme] name`, `[theme.custom]` overrides and the legacy
//! `[ui] accent`, read from Herdr's `config.toml`.

mod builtin;

use std::path::Path;

use ratatui::style::Color;
use serde::Deserialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    pub accent: Color,
    pub panel_bg: Color,
    pub sidebar_bg: Color,
    pub active_row_bg: Color,
    pub selection_bg: Color,
    pub surface0: Color,
    pub surface1: Color,
    pub surface_dim: Color,
    pub overlay0: Color,
    pub overlay1: Color,
    pub text: Color,
    pub subtext0: Color,
    pub mauve: Color,
    pub green: Color,
    pub yellow: Color,
    pub red: Color,
    pub blue: Color,
    pub teal: Color,
    pub peach: Color,
}

pub const DEFAULT_THEME: &str = "catppuccin";

impl Default for Palette {
    fn default() -> Self {
        builtin::builtin(DEFAULT_THEME).unwrap_or_else(unreachable_palette)
    }
}

/// Only reachable if the generated table lost its default entry, which the
/// tests rule out; keeps `Default` free of `unwrap`.
fn unreachable_palette() -> Palette {
    Palette {
        accent: Color::Blue,
        panel_bg: Color::Reset,
        sidebar_bg: Color::Reset,
        active_row_bg: Color::DarkGray,
        selection_bg: Color::DarkGray,
        surface0: Color::Reset,
        surface1: Color::DarkGray,
        surface_dim: Color::DarkGray,
        overlay0: Color::Gray,
        overlay1: Color::White,
        text: Color::Reset,
        subtext0: Color::Gray,
        mauve: Color::Magenta,
        green: Color::Green,
        yellow: Color::Yellow,
        red: Color::Red,
        blue: Color::Blue,
        teal: Color::Cyan,
        peach: Color::Yellow,
    }
}

/// Herdr's accepted spellings for its theme names.
pub fn canonical_name(name: &str) -> Option<&'static str> {
    let name = name.to_lowercase().replace([' ', '_'], "-");
    let canonical = match name.as_str() {
        "catppuccin" | "catppuccin-mocha" => "catppuccin",
        "catppuccin-latte" | "latte" | "light" => "catppuccin-latte",
        "tokyo-night" | "tokyonight" => "tokyo-night",
        "tokyo-night-day" | "tokyo-day" | "tokyonight-day" => "tokyo-night-day",
        "gruvbox" | "gruvbox-dark" => "gruvbox",
        "one-dark" | "onedark" => "one-dark",
        "one-light" | "onelight" => "one-light",
        "solarized" | "solarized-dark" => "solarized",
        "kanagawa-lotus" | "lotus" => "kanagawa-lotus",
        "rose-pine" | "rosepine" => "rose-pine",
        "rose-pine-dawn" | "rosepine-dawn" | "dawn" => "rose-pine-dawn",
        other => return builtin::NAMES.iter().copied().find(|known| *known == other),
    };
    Some(canonical)
}

/// Parses a Herdr color: `#rrggbb`, `#rgb`, `rgb(r, g, b)`, a named ANSI
/// color, or `reset`. Unknown values are `None` (Herdr warns and uses cyan;
/// the inbox keeps the palette's color instead).
pub fn parse_color(value: &str) -> Option<Color> {
    let value = value.trim().to_lowercase();
    if let Some(hex) = value.strip_prefix('#') {
        let digit = |s: &str| u8::from_str_radix(s, 16).ok();
        return match hex.len() {
            6 if hex.is_ascii() => Some(Color::Rgb(digit(&hex[0..2])?, digit(&hex[2..4])?, digit(&hex[4..6])?)),
            3 if hex.is_ascii() => {
                Some(Color::Rgb(digit(&hex[0..1])? * 17, digit(&hex[1..2])? * 17, digit(&hex[2..3])? * 17))
            }
            _ => None,
        };
    }
    if let Some(inner) = value.strip_prefix("rgb(").and_then(|v| v.strip_suffix(')')) {
        let parts: Vec<u8> = inner.split(',').map(|p| p.trim().parse::<u8>()).collect::<Result<_, _>>().ok()?;
        return match parts.as_slice() {
            [r, g, b] => Some(Color::Rgb(*r, *g, *b)),
            _ => None,
        };
    }
    let named = match value.as_str() {
        "reset" | "default" | "none" | "transparent" => Color::Reset,
        "black" => Color::Black,
        "red" => Color::Red,
        "green" => Color::Green,
        "yellow" => Color::Yellow,
        "blue" => Color::Blue,
        "magenta" | "purple" => Color::Magenta,
        "cyan" => Color::Cyan,
        "white" => Color::White,
        "gray" | "grey" => Color::Gray,
        "darkgray" | "darkgrey" => Color::DarkGray,
        "lightred" => Color::LightRed,
        "lightgreen" => Color::LightGreen,
        "lightyellow" => Color::LightYellow,
        "lightblue" => Color::LightBlue,
        "lightmagenta" => Color::LightMagenta,
        "lightcyan" => Color::LightCyan,
        _ => return None,
    };
    Some(named)
}

#[derive(Debug, Default, Deserialize)]
struct HerdrConfig {
    #[serde(default)]
    theme: ThemeSection,
    #[serde(default)]
    ui: UiSection,
}

#[derive(Debug, Default, Deserialize)]
struct ThemeSection {
    name: Option<String>,
    #[serde(default)]
    auto_switch: bool,
    dark_name: Option<String>,
    custom: Option<toml::Table>,
}

#[derive(Debug, Default, Deserialize)]
struct UiSection {
    accent: Option<String>,
}

/// The palette described by a Herdr `config.toml`, or the default when the
/// file is missing or unreadable.
pub fn from_herdr_config(text: &str) -> Palette {
    let Ok(config) = toml::from_str::<HerdrConfig>(text) else {
        return Palette::default();
    };
    // Without knowing the terminal's appearance, auto-switch follows the dark
    // theme, which is what most terminals use.
    let name = if config.theme.auto_switch {
        config.theme.dark_name.as_deref().or(config.theme.name.as_deref())
    } else {
        config.theme.name.as_deref()
    };
    let mut palette = name.and_then(canonical_name).and_then(builtin::builtin).unwrap_or_default();
    if let Some(accent) = config.ui.accent.as_deref().and_then(parse_color) {
        palette.accent = accent;
    }
    if let Some(custom) = &config.theme.custom {
        palette.apply(custom);
    }
    palette
}

pub fn load(herdr_config: &Path) -> Palette {
    std::fs::read_to_string(herdr_config).map(|text| from_herdr_config(&text)).unwrap_or_default()
}

impl Palette {
    /// The background of a card that stands a little off the panel: the
    /// dim surface when the theme defines it in RGB, else the panel's own
    /// surface, since an ANSI grey fill reads as a bright block in many
    /// terminal palettes.
    pub fn raised(&self) -> Color {
        match self.surface_dim {
            Color::Rgb(..) => self.surface_dim,
            _ => self.surface0,
        }
    }

    fn apply(&mut self, custom: &toml::Table) {
        let tokens: [(&str, &mut Color); 19] = [
            ("accent", &mut self.accent),
            ("panel_bg", &mut self.panel_bg),
            ("sidebar_bg", &mut self.sidebar_bg),
            ("active_row_bg", &mut self.active_row_bg),
            ("selection_bg", &mut self.selection_bg),
            ("surface0", &mut self.surface0),
            ("surface1", &mut self.surface1),
            ("surface_dim", &mut self.surface_dim),
            ("overlay0", &mut self.overlay0),
            ("overlay1", &mut self.overlay1),
            ("text", &mut self.text),
            ("subtext0", &mut self.subtext0),
            ("mauve", &mut self.mauve),
            ("green", &mut self.green),
            ("yellow", &mut self.yellow),
            ("red", &mut self.red),
            ("blue", &mut self.blue),
            ("teal", &mut self.teal),
            ("peach", &mut self.peach),
        ];
        for (key, slot) in tokens {
            if let Some(color) = custom.get(key).and_then(toml::Value::as_str).and_then(parse_color) {
                *slot = color;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_builtin_theme_resolves_and_the_default_exists() {
        for name in builtin::NAMES {
            assert_eq!(canonical_name(name), Some(*name));
            assert!(builtin::builtin(name).is_some(), "{name}");
        }
        assert_eq!(builtin::NAMES.len(), 18, "Herdr 0.9.3 ships 18 themes");
        assert_ne!(Palette::default(), unreachable_palette());
    }

    #[test]
    fn theme_aliases_match_herdr() {
        assert_eq!(canonical_name("Catppuccin Mocha"), Some("catppuccin"));
        assert_eq!(canonical_name("latte"), Some("catppuccin-latte"));
        assert_eq!(canonical_name("tokyo_night"), Some("tokyo-night"));
        assert_eq!(canonical_name("ROSEPINE-DAWN"), Some("rose-pine-dawn"));
        assert_eq!(canonical_name("nope"), None);
    }

    #[test]
    fn colors_parse_like_herdr_and_unknown_ones_are_rejected() {
        assert_eq!(parse_color("#FF9F0A"), Some(Color::Rgb(255, 159, 10)));
        assert_eq!(parse_color(" #fa0 "), Some(Color::Rgb(255, 170, 0)));
        assert_eq!(parse_color("rgb(1, 2, 3)"), Some(Color::Rgb(1, 2, 3)));
        assert_eq!(parse_color("Transparent"), Some(Color::Reset));
        assert_eq!(parse_color("grey"), Some(Color::Gray));
        assert_eq!(parse_color("#12345"), None);
        assert_eq!(parse_color("#gggggg"), None);
        assert_eq!(parse_color("rgb(1,2)"), None);
        assert_eq!(parse_color("rgb(1,2,300)"), None);
        assert_eq!(parse_color("#é12"), None);
        assert_eq!(parse_color("chartreuse"), None);
    }

    #[test]
    fn a_missing_or_broken_config_uses_the_default_theme() {
        assert_eq!(from_herdr_config(""), Palette::default());
        assert_eq!(from_herdr_config("this is not toml ["), Palette::default());
        assert_eq!(from_herdr_config("[theme]\nname = \"unknown\""), Palette::default());
        assert_eq!(load(Path::new("/nonexistent/herdr/config.toml")), Palette::default());
    }

    #[test]
    fn the_named_theme_and_custom_overrides_apply_in_order() {
        let config = r##"
[ui]
accent = "#000001"

[theme]
name = "terminal"

[theme.custom]
accent = "#FF9F0A"
sidebar_bg = "#0D0D0F"
red = "not a color"
unknown_token = "#ffffff"
"##;
        let palette = from_herdr_config(config);
        let terminal = builtin::builtin("terminal").unwrap();
        assert_eq!(palette.accent, Color::Rgb(255, 159, 10), "custom beats [ui] accent");
        assert_eq!(palette.sidebar_bg, Color::Rgb(13, 13, 15));
        assert_eq!(palette.red, terminal.red, "an invalid override keeps the theme's color");
        assert_eq!(palette.text, terminal.text);
    }

    #[test]
    fn the_legacy_ui_accent_applies_without_custom_colors() {
        let palette = from_herdr_config("[ui]\naccent = \"magenta\"");
        assert_eq!(palette.accent, Color::Magenta);
    }

    #[test]
    fn auto_switch_follows_the_dark_theme() {
        let palette =
            from_herdr_config("[theme]\nname = \"catppuccin-latte\"\nauto_switch = true\ndark_name = \"dracula\"");
        assert_eq!(palette, builtin::builtin("dracula").unwrap());
    }
}
