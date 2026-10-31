//! Paletas: la de Omarchy (sigue el tema del sistema) y algunas fijas.

use std::{collections::HashMap, path::PathBuf, time::SystemTime};

use ratatui::{style::Color, widgets::BorderType};

#[derive(Debug, Clone)]
pub struct Theme {
    pub fg: Color,
    pub dim: Color,
    pub border: Color,
    pub accent: Color,
    pub ok: Color,
    pub warn: Color,
    pub err: Color,
    /// Fondo de popups, botones y pestaña activa; el resto usa el fondo de la terminal.
    pub surface: Color,
    /// Omarchy usa esquinas rectas; los demás temas, redondeadas.
    pub border_type: BorderType,
}

const fn hex(c: u32) -> Color {
    Color::Rgb((c >> 16) as u8, (c >> 8) as u8, c as u8)
}

/// `#rrggbb` → color.
fn parse_hex(s: &str) -> Option<Color> {
    let s = s.trim().strip_prefix('#')?;
    (s.len() == 6).then_some(())?;
    u32::from_str_radix(s, 16).ok().map(hex)
}

fn rgb(c: Color) -> (f64, f64, f64) {
    match c {
        Color::Rgb(r, g, b) => (f64::from(r), f64::from(g), f64::from(b)),
        _ => (0.0, 0.0, 0.0),
    }
}

/// Mezcla `a` hacia `b` en proporción `t` (0 = a, 1 = b).
fn mix(a: Color, b: Color, t: f64) -> Color {
    let ((ar, ag, ab), (br, bg, bb)) = (rgb(a), rgb(b));
    let m = |x: f64, y: f64| (x + (y - x) * t).round().clamp(0.0, 255.0) as u8;
    Color::Rgb(m(ar, br), m(ag, bg), m(ab, bb))
}

/// Paleta del tema activo de Omarchy.
pub fn omarchy_colors_path() -> Option<PathBuf> {
    let p = dirs::home_dir()?.join(".local/state/omarchy/current/theme/colors.toml");
    p.exists().then_some(p)
}

/// Fecha de modificación de la paleta de Omarchy, para notar cambios de tema.
pub fn omarchy_stamp() -> Option<SystemTime> {
    omarchy_colors_path()?.metadata().ok()?.modified().ok()
}

impl Theme {
    pub fn by_name(name: Option<&str>) -> Self {
        let default = if omarchy_colors_path().is_some() {
            "omarchy"
        } else {
            "tokyonight"
        };
        match name.unwrap_or(default) {
            "omarchy" => Self::omarchy().unwrap_or_else(|| Self::by_name(Some("tokyonight"))),
            "catppuccin" => Self {
                fg: hex(0xcdd6f4),
                dim: hex(0x6c7086),
                border: hex(0x45475a),
                accent: hex(0x89b4fa),
                ok: hex(0xa6e3a1),
                warn: hex(0xf9e2af),
                err: hex(0xf38ba8),
                surface: hex(0x313244),
                border_type: BorderType::Rounded,
            },
            "gruvbox" => Self {
                fg: hex(0xebdbb2),
                dim: hex(0x928374),
                border: hex(0x504945),
                accent: hex(0x83a598),
                ok: hex(0xb8bb26),
                warn: hex(0xfabd2f),
                err: hex(0xfb4934),
                surface: hex(0x3c3836),
                border_type: BorderType::Rounded,
            },
            _ => Self {
                fg: hex(0xc0caf5),
                dim: hex(0x565f89),
                border: hex(0x3b4261),
                accent: hex(0x7aa2f7),
                ok: hex(0x9ece6a),
                warn: hex(0xe0af68),
                err: hex(0xf7768e),
                surface: hex(0x292e42),
                border_type: BorderType::Rounded,
            },
        }
    }

    /// Lee la paleta de Omarchy (`colors.toml`) y la reparte en los roles de la interfaz.
    pub fn omarchy() -> Option<Self> {
        let raw = std::fs::read_to_string(omarchy_colors_path()?).ok()?;
        Self::from_omarchy(&raw)
    }

    fn from_omarchy(raw: &str) -> Option<Self> {
        let map: HashMap<String, String> = toml::from_str(raw).ok()?;
        let get = |k: &str| map.get(k).and_then(|v| parse_hex(v));
        let bg = get("background")?;
        let fg = get("foreground")?;
        let accent = get("accent").or_else(|| get("color4"))?;
        let c = |n: u8, fallback: Color| get(&format!("color{n}")).unwrap_or(fallback);
        // Los slots ANSI: 1 rojo, 2 verde, 3 amarillo, 8 gris.
        Some(Self {
            fg,
            dim: c(8, mix(bg, fg, 0.45)),
            border: mix(bg, accent, 0.4),
            accent,
            ok: c(2, hex(0x9ece6a)),
            warn: c(3, hex(0xe0af68)),
            err: c(1, hex(0xf7768e)),
            surface: mix(bg, accent, 0.18),
            border_type: BorderType::Plain,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_omarchy_palette() {
        let t = Theme::from_omarchy(
            r##"
            accent = "#bb9af7"
            background = "#000000"
            foreground = "#ffffff"
            color1 = "#ff0000"
            color2 = "#00ff00"
            color8 = "#808080"
            "##,
        )
        .unwrap();
        assert_eq!(t.accent, Color::Rgb(0xbb, 0x9a, 0xf7));
        assert_eq!(t.err, Color::Rgb(255, 0, 0));
        assert_eq!(t.ok, Color::Rgb(0, 255, 0));
        assert_eq!(t.warn, hex(0xe0af68));
        assert!(Theme::from_omarchy("accent = 1").is_none());
    }
}
