//! Animación de entrada: el cuadro ya dibujado aparece en dos capas.
//!
//! 1. **Líneas**: cada borde se traza desde su esquina superior izquierda,
//!    como una pluma que recorre las dos aristas a la vez, con una cabeza
//!    brillante y una estela que se apaga hasta su color. Todos los paneles se
//!    cierran a la vez, sea cual sea su tamaño.
//! 2. **Contenido**: el texto y los fondos aparecen en cascada desde arriba a la
//!    izquierda, con un destello del color de acento que se asienta.
//!
//! Es un post-proceso del `Buffer`: no toca ningún widget, así que vale igual
//! para todas las pestañas y popups.

use std::{collections::VecDeque, time::Duration};

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier},
};

use crate::theme::{Theme, mix};

/// Las líneas tardan esto en cerrarse.
const LINES: f64 = 0.9;
/// Retraso máximo de una esquina según su posición (las de arriba a la
/// izquierda arrancan antes).
const LINE_STAGGER: f64 = 0.15;
/// Cuándo empieza a aparecer el contenido.
const TEXT_START: f64 = 0.45;
/// De la primera celda de contenido a la última.
const TEXT_SPREAD: f64 = 0.55;
/// Lo que tarda cada celda de contenido en asentarse.
const TEXT_FADE: f64 = 0.35;
/// Celdas de estela tras la cabeza de la pluma.
const TRAIL: f64 = 10.0;

/// Duración total; después el cuadro queda tal cual.
pub const TOTAL: Duration = Duration::from_millis(1400);

const UP: u8 = 1;
const DOWN: u8 = 2;
const LEFT: u8 = 4;
const RIGHT: u8 = 8;

/// Hacia dónde sale cada trazo de un carácter de caja.
fn arms(c: char) -> u8 {
    match c {
        '─' | '━' | '═' | '╌' | '┄' | '┈' => LEFT | RIGHT,
        '│' | '┃' | '║' | '╎' | '┆' | '┊' => UP | DOWN,
        '┌' | '╭' | '┏' | '╔' => RIGHT | DOWN,
        '┐' | '╮' | '┓' | '╗' => LEFT | DOWN,
        '└' | '╰' | '┗' | '╚' => UP | RIGHT,
        '┘' | '╯' | '┛' | '╝' => UP | LEFT,
        '├' | '┣' | '╠' => UP | DOWN | RIGHT,
        '┤' | '┫' | '╣' => UP | DOWN | LEFT,
        '┬' | '┳' | '╦' => LEFT | RIGHT | DOWN,
        '┴' | '┻' | '╩' => LEFT | RIGHT | UP,
        '┼' | '╋' | '╬' => UP | DOWN | LEFT | RIGHT,
        _ => 0,
    }
}

fn ease_out(t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

fn ease_in_out(t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    if t < 0.5 {
        4.0 * t * t * t
    } else {
        1.0 - (-2.0 * t + 2.0).powi(3) / 2.0
    }
}

/// Recorrido de las líneas: para cada celda de borde, a cuántos pasos está
/// del inicio de su trazo, cuánto mide ese trazo y cuándo arranca.
struct Strokes {
    /// Por celda (índice del buffer): (distancia, trazo).
    at: Vec<Option<(u32, usize)>>,
    /// Por trazo: (longitud, retraso en segundos).
    strokes: Vec<(u32, f64)>,
}

/// Índices locales a `area` (fila a fila).
fn strokes(buf: &Buffer, area: Rect) -> Strokes {
    let (w, h) = (area.width as usize, area.height as usize);
    let arm: Vec<u8> = (0..w * h)
        .map(|i| {
            let cell = &buf[(area.x + (i % w) as u16, area.y + (i / w) as u16)];
            let mut chars = cell.symbol().chars();
            match (chars.next(), chars.next()) {
                (Some(ch), None) => arms(ch),
                _ => 0,
            }
        })
        .collect();
    let mut at: Vec<Option<(u32, usize)>> = vec![None; w * h];
    let mut strokes = vec![];
    // Unidas solo si los dos trazos se miran (┐ no se une con el ─ de abajo).
    let next = |i: usize, dir: u8| -> Option<usize> {
        let (x, y) = (i % w, i / w);
        let (j, back) = match dir {
            UP if y > 0 => (i - w, DOWN),
            DOWN if y + 1 < h => (i + w, UP),
            LEFT if x > 0 => (i - 1, RIGHT),
            RIGHT if x + 1 < w => (i + 1, LEFT),
            _ => return None,
        };
        (arm[i] & dir != 0 && arm[j] & back != 0).then_some(j)
    };
    // Primero desde las esquinas superiores izquierdas (paneles) y luego desde
    // cualquier punto suelto (separadores), en orden de lectura.
    let corner = |i: usize| arm[i] == RIGHT | DOWN;
    let order = (0..w * h)
        .filter(|&i| corner(i))
        .chain((0..w * h).filter(|&i| arm[i] != 0));
    for seed in order {
        if at[seed].is_some() {
            continue;
        }
        let id = strokes.len();
        let mut longest = 0;
        let mut queue = VecDeque::from([(seed, 0u32)]);
        at[seed] = Some((0, id));
        while let Some((i, d)) = queue.pop_front() {
            longest = longest.max(d);
            for dir in [UP, DOWN, LEFT, RIGHT] {
                if let Some(j) = next(i, dir)
                    && at[j].is_none()
                {
                    at[j] = Some((d + 1, id));
                    queue.push_back((j, d + 1));
                }
            }
        }
        let (x, y) = (seed % w, seed / w);
        let delay =
            LINE_STAGGER * (0.5 * x as f64 / w.max(1) as f64 + 0.5 * y as f64 / h.max(1) as f64);
        strokes.push((longest, delay));
    }
    Strokes { at, strokes }
}

/// El color real de una celda (los `Reset` no se pueden mezclar).
fn solid(c: Color, fallback: Color) -> Color {
    match c {
        Color::Rgb(..) => c,
        _ => fallback,
    }
}

/// Dibuja `area` del cuadro tal como se ve a `elapsed` del inicio (al abrir,
/// la pantalla entera; al entrar por primera vez en una pestaña, su cuerpo).
pub fn apply(buf: &mut Buffer, area: Rect, elapsed: Duration, theme: &Theme) {
    let t = elapsed.as_secs_f64();
    if t >= TOTAL.as_secs_f64() {
        return;
    }
    let area = area.intersection(buf.area);
    let (w, h) = (area.width as usize, area.height as usize);
    if w == 0 || h == 0 {
        return;
    }
    let lines = strokes(buf, area);
    let head = mix(theme.accent, Color::Rgb(255, 255, 255), 0.45);

    for i in 0..w * h {
        let (x, y) = (i % w, i / w);
        let cell = &mut buf[(area.x + x as u16, area.y + y as u16)];
        match lines.at[i] {
            // Borde: visible si la pluma ya pasó por aquí.
            Some((d, id)) => {
                let (len, delay) = lines.strokes[id];
                // Arranca suave, acelera y frena al cerrar el trazo.
                let p = ease_in_out((t - delay) / LINES);
                let pen = p * f64::from(len.max(1)) + 0.5;
                let d = f64::from(d);
                if d > pen || p <= 0.0 {
                    cell.set_symbol(" ");
                    continue;
                }
                let real = solid(cell.fg, theme.border);
                if p >= 1.0 {
                    // Trazo cerrado: un último brillo que se apaga.
                    let after = (t - delay - LINES) / 0.25;
                    if after < 1.0 {
                        cell.fg = mix(theme.accent, real, ease_out(after));
                    }
                    continue;
                }
                let behind = pen - d;
                if behind < 1.5 {
                    cell.fg = head;
                    cell.modifier.insert(Modifier::BOLD);
                } else {
                    let k = ((behind - 1.5) / TRAIL).clamp(0.0, 1.0);
                    cell.fg = mix(theme.accent, real, ease_out(k));
                }
            }
            // Contenido: cascada en diagonal con destello de acento.
            None => {
                let order = 0.65 * y as f64 / h as f64 + 0.35 * x as f64 / w as f64;
                let k = (t - TEXT_START - order * TEXT_SPREAD) / TEXT_FADE;
                if k >= 1.0 {
                    continue;
                }
                let has_bg = matches!(cell.bg, Color::Rgb(..));
                let blank = cell.symbol().trim().is_empty();
                if k <= 0.0 {
                    if !blank {
                        cell.set_symbol(" ");
                    }
                    if has_bg {
                        cell.bg = theme.bg;
                    }
                    continue;
                }
                let e = ease_in_out(k);
                if has_bg {
                    cell.bg = mix(theme.bg, cell.bg, e);
                }
                if !blank {
                    let real = solid(cell.fg, theme.fg);
                    // Sale del fondo, pasa por el acento y se asienta.
                    let base = solid(cell.bg, theme.bg);
                    cell.fg = if e < 0.5 {
                        mix(base, theme.accent, e * 2.0)
                    } else {
                        mix(theme.accent, real, (e - 0.5) * 2.0)
                    };
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn boxed() -> Buffer {
        // ╭────╮
        // │ hi │
        // ╰────╯
        let mut b = Buffer::empty(Rect::new(0, 0, 6, 3));
        for (y, row) in ["╭────╮", "│ hi │", "╰────╯"].iter().enumerate()
        {
            for (x, ch) in row.chars().enumerate() {
                b[(x as u16, y as u16)].set_symbol(&ch.to_string());
            }
        }
        b
    }

    fn sym(b: &Buffer, x: u16, y: u16) -> String {
        b[(x, y)].symbol().to_string()
    }

    #[test]
    fn a_box_is_one_stroke_from_its_corner() {
        let b = boxed();
        let s = strokes(&b, b.area);
        assert_eq!(s.strokes.len(), 1);
        assert_eq!(s.at[0], Some((0, 0)), "╭ empieza");
        // La esquina opuesta es lo último: 5 a la derecha + 2 abajo.
        assert_eq!(s.at[3 * 6 - 1], Some((7, 0)));
        // El texto no es borde.
        assert_eq!(s.at[6 + 2], None);
    }

    #[test]
    fn lines_only_join_when_both_arms_meet() {
        // Un ─ justo debajo de un ┐ no sigue el trazo.
        let mut b = Buffer::empty(Rect::new(0, 0, 2, 2));
        b[(0, 0)].set_symbol("─");
        b[(1, 0)].set_symbol("┐");
        b[(1, 1)].set_symbol("─");
        let s = strokes(&b, b.area);
        assert_ne!(s.at[1].unwrap().1, s.at[3].unwrap().1);
    }

    #[test]
    fn draws_from_the_corner_then_shows_everything() {
        let theme = crate::theme::Theme::by_name(Some("tokyonight"));
        let mut start = boxed();
        apply(&mut start, Rect::new(0, 0, 6, 3), Duration::ZERO, &theme);
        assert_eq!(sym(&start, 0, 0), " ", "al inicio no hay nada");
        assert_eq!(sym(&start, 2, 1), " ");

        let mut mid = boxed();
        apply(
            &mut mid,
            Rect::new(0, 0, 6, 3),
            Duration::from_millis(250),
            &theme,
        );
        assert_eq!(sym(&mid, 0, 0), "╭", "la esquina ya está");
        assert_eq!(sym(&mid, 5, 2), " ", "la opuesta todavía no");

        let mut end = boxed();
        apply(&mut end, Rect::new(0, 0, 6, 3), TOTAL, &theme);
        assert_eq!(end, boxed(), "al final, el cuadro tal cual");
    }

    #[test]
    fn only_touches_its_area() {
        let theme = crate::theme::Theme::by_name(Some("tokyonight"));
        // Solo la fila del medio: los bordes de arriba y abajo quedan intactos.
        let mut b = boxed();
        apply(&mut b, Rect::new(0, 1, 6, 1), Duration::ZERO, &theme);
        assert_eq!(sym(&b, 0, 0), "╭");
        assert_eq!(sym(&b, 5, 2), "╯");
        assert_eq!(sym(&b, 2, 1), " ", "el texto de la zona aún no aparece");
    }
}
