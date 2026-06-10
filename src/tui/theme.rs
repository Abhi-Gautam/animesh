//! Theme registry and semantic color roles for the TUI.
//!
//! Render code should consume [`ThemeRoles`] / [`ThemeStyles`] instead of
//! hardcoding raw colors. Built-in palettes can then be swapped globally and
//! previewed live by the theme picker.

use ratatui::style::{Color, Modifier, Style};

pub(crate) const DEFAULT_THEME_ID: &str = "catppuccin-mocha";
pub(crate) const KV_UI_THEME: &str = "ui.theme";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Appearance {
    Light,
    Dark,
}

impl Appearance {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }
}

#[allow(dead_code)]
#[derive(Clone, Copy, Debug)]
pub(crate) struct ThemePalette {
    pub rosewater: Color,
    pub flamingo: Color,
    pub pink: Color,
    pub mauve: Color,
    pub red: Color,
    pub maroon: Color,
    pub peach: Color,
    pub yellow: Color,
    pub green: Color,
    pub teal: Color,
    pub sky: Color,
    pub sapphire: Color,
    pub blue: Color,
    pub lavender: Color,
    pub text: Color,
    pub subtext1: Color,
    pub subtext0: Color,
    pub overlay2: Color,
    pub overlay1: Color,
    pub overlay0: Color,
    pub surface2: Color,
    pub surface1: Color,
    pub surface0: Color,
    pub base: Color,
    pub mantle: Color,
    pub crust: Color,
}

#[allow(dead_code)]
#[derive(Clone, Copy, Debug)]
pub(crate) struct ThemeRoles {
    pub bg: Color,
    pub panel_bg: Color,
    pub popup_bg: Color,
    pub fg: Color,
    pub fg_muted: Color,
    pub fg_dim: Color,
    pub subtle: Color,
    pub accent: Color,
    pub accent_alt: Color,
    pub border: Color,
    pub border_focused: Color,
    pub selected_fg: Color,
    pub selected_bg: Color,
    pub success: Color,
    pub warning: Color,
    pub danger: Color,
    pub info: Color,
    pub playable: Color,
    pub upcoming: Color,
    pub late: Color,
    pub watched: Color,
    pub episode: Color,
}

#[allow(dead_code)]
#[derive(Clone, Copy, Debug)]
pub(crate) struct ThemeStyles {
    pub normal: Style,
    pub muted: Style,
    pub dim: Style,
    pub subtle: Style,
    pub title: Style,
    pub title_focused: Style,
    pub border: Style,
    pub border_focused: Style,
    pub selected: Style,
    pub key: Style,
    pub success: Style,
    pub warning: Style,
    pub danger: Style,
    pub info: Style,
    pub popup: Style,
    pub mode_badge: Style,
}

#[allow(dead_code)]
#[derive(Clone, Debug)]
pub(crate) struct Theme {
    pub id: &'static str,
    pub name: &'static str,
    pub family: &'static str,
    pub appearance: Appearance,
    pub palette: ThemePalette,
    pub roles: ThemeRoles,
    pub styles: ThemeStyles,
}

impl Theme {
    pub(crate) fn from_catppuccin(
        id: &'static str,
        name: &'static str,
        appearance: Appearance,
        palette: ThemePalette,
    ) -> Self {
        let roles = catppuccin_roles(palette, appearance);
        Self::from_roles(id, name, "Catppuccin", appearance, palette, roles)
    }

    pub(crate) fn from_roles(
        id: &'static str,
        name: &'static str,
        family: &'static str,
        appearance: Appearance,
        palette: ThemePalette,
        roles: ThemeRoles,
    ) -> Self {
        let styles = ThemeStyles {
            normal: Style::default().fg(roles.fg).bg(roles.bg),
            muted: Style::default().fg(roles.fg_muted).bg(roles.bg),
            dim: Style::default().fg(roles.fg_dim).bg(roles.bg),
            subtle: Style::default().fg(roles.subtle).bg(roles.bg),
            title: Style::default()
                .fg(roles.fg)
                .bg(roles.bg)
                .add_modifier(Modifier::BOLD),
            title_focused: Style::default()
                .fg(roles.accent)
                .bg(roles.bg)
                .add_modifier(Modifier::BOLD),
            border: Style::default().fg(roles.border).bg(roles.bg),
            border_focused: Style::default().fg(roles.border_focused).bg(roles.bg),
            selected: Style::default()
                .fg(roles.selected_fg)
                .bg(roles.selected_bg)
                .add_modifier(Modifier::BOLD),
            key: Style::default()
                .fg(roles.accent)
                .bg(roles.bg)
                .add_modifier(Modifier::BOLD),
            success: Style::default().fg(roles.success).bg(roles.bg),
            warning: Style::default().fg(roles.warning).bg(roles.bg),
            danger: Style::default().fg(roles.danger).bg(roles.bg),
            info: Style::default().fg(roles.info).bg(roles.bg),
            popup: Style::default().fg(roles.fg).bg(roles.popup_bg),
            mode_badge: Style::default()
                .fg(roles.selected_fg)
                .bg(roles.selected_bg)
                .add_modifier(Modifier::BOLD),
        };
        Self {
            id,
            name,
            family,
            appearance,
            palette,
            roles,
            styles,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct ThemeRegistry {
    themes: Vec<Theme>,
}

impl ThemeRegistry {
    pub(crate) fn builtin() -> Self {
        Self {
            themes: vec![
                catppuccin_mocha(),
                catppuccin_macchiato(),
                catppuccin_frappe(),
                catppuccin_latte(),
            ],
        }
    }

    pub(crate) fn all(&self) -> &[Theme] {
        &self.themes
    }

    pub(crate) fn get(&self, id: &str) -> Option<&Theme> {
        let id = normalize_theme_id(id);
        self.themes.iter().find(|theme| theme.id == id)
    }

    pub(crate) fn default_theme(&self) -> &Theme {
        self.get(DEFAULT_THEME_ID).unwrap_or(&self.themes[0])
    }

    pub(crate) fn index_of(&self, id: &str) -> usize {
        let id = normalize_theme_id(id);
        self.themes
            .iter()
            .position(|theme| theme.id == id)
            .unwrap_or_else(|| {
                self.themes
                    .iter()
                    .position(|theme| theme.id == DEFAULT_THEME_ID)
                    .unwrap_or(0)
            })
    }
}

#[derive(Clone, Debug)]
pub(crate) struct ThemePickerState {
    pub selected: usize,
    pub original_theme_id: String,
    pub preview_theme_id: Option<String>,
}

impl Default for ThemePickerState {
    fn default() -> Self {
        Self {
            selected: 0,
            original_theme_id: DEFAULT_THEME_ID.to_string(),
            preview_theme_id: None,
        }
    }
}

impl ThemePickerState {
    pub(crate) fn open(&mut self, current_theme_id: &str, selected: usize) {
        self.selected = selected;
        self.original_theme_id = current_theme_id.to_string();
        self.preview_theme_id = Some(current_theme_id.to_string());
    }

    pub(crate) fn close(&mut self) {
        self.preview_theme_id = None;
    }

    pub(crate) fn move_selection(&mut self, delta: i32, len: usize) {
        if len == 0 {
            self.selected = 0;
            self.preview_theme_id = None;
            return;
        }
        let n = len as i32;
        let cur = self.selected as i32;
        self.selected = (cur + delta).rem_euclid(n) as usize;
    }
}

pub(crate) fn normalize_theme_id(id: &str) -> &str {
    match id.trim().to_ascii_lowercase().as_str() {
        "catppuccin" | "catppuccin-dark" | "mocha" | "dark" => "catppuccin-mocha",
        "catppuccin-light" | "latte" | "light" => "catppuccin-latte",
        "frappe" | "frappé" => "catppuccin-frappe",
        "macchiato" => "catppuccin-macchiato",
        _ => id.trim(),
    }
}

fn rgb(hex: u32) -> Color {
    Color::Rgb(
        ((hex >> 16) & 0xff) as u8,
        ((hex >> 8) & 0xff) as u8,
        (hex & 0xff) as u8,
    )
}

fn catppuccin_roles(p: ThemePalette, appearance: Appearance) -> ThemeRoles {
    let selected_fg = match appearance {
        Appearance::Light => p.base,
        Appearance::Dark => p.crust,
    };
    ThemeRoles {
        bg: p.base,
        panel_bg: p.mantle,
        popup_bg: p.mantle,
        fg: p.text,
        fg_muted: p.subtext1,
        fg_dim: p.overlay1,
        subtle: p.surface0,
        accent: p.peach,
        accent_alt: p.mauve,
        border: p.surface0,
        border_focused: p.peach,
        selected_fg,
        selected_bg: p.peach,
        success: p.green,
        warning: p.yellow,
        danger: p.red,
        info: p.sky,
        playable: p.peach,
        upcoming: p.sky,
        late: p.red,
        watched: p.green,
        episode: p.yellow,
    }
}

pub(crate) fn catppuccin_latte() -> Theme {
    Theme::from_catppuccin(
        "catppuccin-latte",
        "Catppuccin Latte",
        Appearance::Light,
        ThemePalette {
            rosewater: rgb(0xdc8a78),
            flamingo: rgb(0xdd7878),
            pink: rgb(0xea76cb),
            mauve: rgb(0x8839ef),
            red: rgb(0xd20f39),
            maroon: rgb(0xe64553),
            peach: rgb(0xfe640b),
            yellow: rgb(0xdf8e1d),
            green: rgb(0x40a02b),
            teal: rgb(0x179299),
            sky: rgb(0x04a5e5),
            sapphire: rgb(0x209fb5),
            blue: rgb(0x1e66f5),
            lavender: rgb(0x7287fd),
            text: rgb(0x4c4f69),
            subtext1: rgb(0x5c5f77),
            subtext0: rgb(0x6c6f85),
            overlay2: rgb(0x7c7f93),
            overlay1: rgb(0x8c8fa1),
            overlay0: rgb(0x9ca0b0),
            surface2: rgb(0xacb0be),
            surface1: rgb(0xbcc0cc),
            surface0: rgb(0xccd0da),
            base: rgb(0xeff1f5),
            mantle: rgb(0xe6e9ef),
            crust: rgb(0xdce0e8),
        },
    )
}

pub(crate) fn catppuccin_frappe() -> Theme {
    Theme::from_catppuccin(
        "catppuccin-frappe",
        "Catppuccin Frappé",
        Appearance::Dark,
        ThemePalette {
            rosewater: rgb(0xf2d5cf),
            flamingo: rgb(0xeebebe),
            pink: rgb(0xf4b8e4),
            mauve: rgb(0xca9ee6),
            red: rgb(0xe78284),
            maroon: rgb(0xea999c),
            peach: rgb(0xef9f76),
            yellow: rgb(0xe5c890),
            green: rgb(0xa6d189),
            teal: rgb(0x81c8be),
            sky: rgb(0x99d1db),
            sapphire: rgb(0x85c1dc),
            blue: rgb(0x8caaee),
            lavender: rgb(0xbabbf1),
            text: rgb(0xc6d0f5),
            subtext1: rgb(0xb5bfe2),
            subtext0: rgb(0xa5adce),
            overlay2: rgb(0x949cbb),
            overlay1: rgb(0x838ba7),
            overlay0: rgb(0x737994),
            surface2: rgb(0x626880),
            surface1: rgb(0x51576d),
            surface0: rgb(0x414559),
            base: rgb(0x303446),
            mantle: rgb(0x292c3c),
            crust: rgb(0x232634),
        },
    )
}

pub(crate) fn catppuccin_macchiato() -> Theme {
    Theme::from_catppuccin(
        "catppuccin-macchiato",
        "Catppuccin Macchiato",
        Appearance::Dark,
        ThemePalette {
            rosewater: rgb(0xf4dbd6),
            flamingo: rgb(0xf0c6c6),
            pink: rgb(0xf5bde6),
            mauve: rgb(0xc6a0f6),
            red: rgb(0xed8796),
            maroon: rgb(0xee99a0),
            peach: rgb(0xf5a97f),
            yellow: rgb(0xeed49f),
            green: rgb(0xa6da95),
            teal: rgb(0x8bd5ca),
            sky: rgb(0x91d7e3),
            sapphire: rgb(0x7dc4e4),
            blue: rgb(0x8aadf4),
            lavender: rgb(0xb7bdf8),
            text: rgb(0xcad3f5),
            subtext1: rgb(0xb8c0e0),
            subtext0: rgb(0xa5adcb),
            overlay2: rgb(0x939ab7),
            overlay1: rgb(0x8087a2),
            overlay0: rgb(0x6e738d),
            surface2: rgb(0x5b6078),
            surface1: rgb(0x494d64),
            surface0: rgb(0x363a4f),
            base: rgb(0x24273a),
            mantle: rgb(0x1e2030),
            crust: rgb(0x181926),
        },
    )
}

pub(crate) fn catppuccin_mocha() -> Theme {
    Theme::from_catppuccin(
        "catppuccin-mocha",
        "Catppuccin Mocha",
        Appearance::Dark,
        ThemePalette {
            rosewater: rgb(0xf5e0dc),
            flamingo: rgb(0xf2cdcd),
            pink: rgb(0xf5c2e7),
            mauve: rgb(0xcba6f7),
            red: rgb(0xf38ba8),
            maroon: rgb(0xeba0ac),
            peach: rgb(0xfab387),
            yellow: rgb(0xf9e2af),
            green: rgb(0xa6e3a1),
            teal: rgb(0x94e2d5),
            sky: rgb(0x89dceb),
            sapphire: rgb(0x74c7ec),
            blue: rgb(0x89b4fa),
            lavender: rgb(0xb4befe),
            text: rgb(0xcdd6f4),
            subtext1: rgb(0xbac2de),
            subtext0: rgb(0xa6adc8),
            overlay2: rgb(0x9399b2),
            overlay1: rgb(0x7f849c),
            overlay0: rgb(0x6c7086),
            surface2: rgb(0x585b70),
            surface1: rgb(0x45475a),
            surface0: rgb(0x313244),
            base: rgb(0x1e1e2e),
            mantle: rgb(0x181825),
            crust: rgb(0x11111b),
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aliases_resolve_to_builtin_themes() {
        let registry = ThemeRegistry::builtin();
        assert_eq!(registry.get("catppuccin").unwrap().id, "catppuccin-mocha");
        assert_eq!(registry.get("light").unwrap().id, "catppuccin-latte");
        assert_eq!(registry.get("frappé").unwrap().id, "catppuccin-frappe");
    }

    #[test]
    fn builtin_registry_has_all_catppuccin_flavors() {
        let ids: Vec<_> = ThemeRegistry::builtin()
            .all()
            .iter()
            .map(|theme| theme.id)
            .collect();
        assert_eq!(
            ids,
            vec![
                "catppuccin-mocha",
                "catppuccin-macchiato",
                "catppuccin-frappe",
                "catppuccin-latte",
            ]
        );
    }
}
