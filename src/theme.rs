//! Paletas: la de Omarchy (sigue el tema del sistema) y algunas fijas. De cada
//! una se derivan los tonos de barras, selección y franjas mezclando el fondo
//! con el acento, para que todo combine.

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
    /// Fondo de la terminal (para texto sobre pastillas de acento).
    pub bg: Color,
    /// Cabecera y pie.
    pub bar: Color,
    /// Popups, botones y tarjetas.
    pub surface: Color,
    /// Fila seleccionada.
    pub sel: Color,
    /// Filas alternas.
    pub stripe: Color,
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
pub fn mix(a: Color, b: Color, t: f64) -> Color {
    let ((ar, ag, ab), (br, bg, bb)) = (rgb(a), rgb(b));
    let m = |x: f64, y: f64| (x + (y - x) * t).round().clamp(0.0, 255.0) as u8;
    Color::Rgb(m(ar, br), m(ag, bg), m(ab, bb))
}

/// Tono (0‥360) y saturación (0‥1) en HSL.
fn hue_sat(c: Color) -> (f64, f64) {
    let (r, g, b) = rgb(c);
    let (r, g, b) = (r / 255.0, g / 255.0, b / 255.0);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    let l = (max + min) / 2.0;
    if d == 0.0 {
        return (0.0, 0.0);
    }
    let s = d / (1.0 - (2.0 * l - 1.0).abs());
    let h = if max == r {
        60.0 * ((g - b) / d).rem_euclid(6.0)
    } else if max == g {
        60.0 * ((b - r) / d + 2.0)
    } else {
        60.0 * ((r - g) / d + 4.0)
    };
    (h, s)
}

/// El color del slot si de verdad es del tono esperado; si no (p. ej. un tema
/// monocromo con el «verde» lavanda), uno que sí lo es, teñido del acento.
fn semantic(slot: Option<Color>, hues: &[(f64, f64)], fallback: Color, accent: Color) -> Color {
    if let Some(c) = slot {
        let (h, s) = hue_sat(c);
        if s > 0.3 && hues.iter().any(|&(lo, hi)| h >= lo && h <= hi) {
            return c;
        }
    }
    mix(fallback, accent, 0.12)
}

const GREEN: [(f64, f64); 1] = [(75.0, 170.0)];
const YELLOW: [(f64, f64); 1] = [(25.0, 65.0)];
const RED: [(f64, f64); 2] = [(0.0, 20.0), (330.0, 360.0)];

/// Paleta del tema activo de Omarchy.
pub fn omarchy_colors_path() -> Option<PathBuf> {
    let p = dirs::home_dir()?.join(".local/state/omarchy/current/theme/colors.toml");
    p.exists().then_some(p)
}

/// Fecha de modificación de la paleta de Omarchy, para notar cambios de tema.
pub fn omarchy_stamp() -> Option<SystemTime> {
    omarchy_colors_path()?.metadata().ok()?.modified().ok()
}

struct Base {
    bg: Color,
    fg: Color,
    accent: Color,
    dim: Option<Color>,
    ok: Option<Color>,
    warn: Option<Color>,
    err: Option<Color>,
}

impl Theme {
    fn derive(b: Base) -> Self {
        Self {
            fg: b.fg,
            dim: b.dim.unwrap_or_else(|| mix(b.bg, b.fg, 0.45)),
            border: mix(b.bg, b.accent, 0.35),
            accent: b.accent,
            ok: semantic(b.ok, &GREEN, hex(0x8fd6a4), b.accent),
            warn: semantic(b.warn, &YELLOW, hex(0xe8b86d), b.accent),
            err: semantic(b.err, &RED, hex(0xf07178), b.accent),
            bg: b.bg,
            bar: mix(b.bg, b.accent, 0.10),
            surface: mix(b.bg, b.accent, 0.18),
            sel: mix(b.bg, b.accent, 0.30),
            stripe: mix(b.bg, b.fg, 0.035),
            border_type: BorderType::Rounded,
        }
    }

    pub fn by_name(name: Option<&str>) -> Self {
        let default = if omarchy_colors_path().is_some() {
            "omarchy"
        } else {
            "tokyonight"
        };
        let fixed = |bg, fg, accent, dim, ok, warn, err| {
            Self::derive(Base {
                bg: hex(bg),
                fg: hex(fg),
                accent: hex(accent),
                dim: Some(hex(dim)),
                ok: Some(hex(ok)),
                warn: Some(hex(warn)),
                err: Some(hex(err)),
            })
        };
        match name.unwrap_or(default) {
            "omarchy" => Self::omarchy().unwrap_or_else(|| Self::by_name(Some("tokyonight"))),
            "catppuccin" => fixed(
                0x1e1e2e, 0xcdd6f4, 0xcba6f7, 0x6c7086, 0xa6e3a1, 0xf9e2af, 0xf38ba8,
            ),
            "gruvbox" => fixed(
                0x282828, 0xebdbb2, 0x83a598, 0x928374, 0xb8bb26, 0xfabd2f, 0xfb4934,
            ),
            _ => fixed(
                0x1a1b26, 0xc0caf5, 0x7aa2f7, 0x565f89, 0x9ece6a, 0xe0af68, 0xf7768e,
            ),
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
        // Los slots ANSI: 1 rojo, 2 verde, 3 amarillo, 8 gris.
        Some(Self::derive(Base {
            bg: get("background")?,
            fg: get("foreground")?,
            accent: get("accent").or_else(|| get("color4"))?,
            dim: get("color8"),
            ok: get("color2"),
            warn: get("color3"),
            err: get("color1"),
        }))
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
        assert_eq!(t.dim, Color::Rgb(128, 128, 128));
        assert!(Theme::from_omarchy("accent = 1").is_none());
    }

    #[test]
    fn monochrome_themes_keep_status_colors_apart() {
        // Como «void»: el verde y el amarillo del tema son lavanda.
        let t = Theme::from_omarchy(
            r##"
            accent = "#bb9af7"
            background = "#05010c"
            foreground = "#ffffff"
            color1 = "#f07178"
            color2 = "#c2b8ff"
            color3 = "#ddccff"
            "##,
        )
        .unwrap();
        let (ok, _) = hue_sat(t.ok);
        let (warn, _) = hue_sat(t.warn);
        assert!((75.0..170.0).contains(&ok), "verde: {ok}");
        assert!((25.0..65.0).contains(&warn), "amarillo: {warn}");
        assert_eq!(t.err, Color::Rgb(0xf0, 0x71, 0x78));
    }
}
