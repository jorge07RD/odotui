//! Piezas reutilizables: pastillas, botones, paneles, listas y logs. Todas
//! registran sus áreas en el `HitMap` mientras se dibujan.

use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Style},
    text::{Line, Span},
    widgets::Block,
};

use crate::{
    app::App,
    input::{Action, HitMap, Pane},
    log::{self, Level, ListState, LogView},
};

/// Íconos de Nerd Font, con equivalentes simples para terminales sin ella.
pub struct Glyphs {
    /// Extremos redondeados de las pastillas.
    pub lcap: &'static str,
    pub rcap: &'static str,
    pub db: &'static str,
    pub restore: &'static str,
    pub server: &'static str,
    pub modules: &'static str,
    pub profiles: &'static str,
    pub ai: &'static str,
    pub docker: &'static str,
    pub local: &'static str,
    pub log: &'static str,
    pub history: &'static str,
    pub info: &'static str,
    pub filter: &'static str,
    pub backup: &'static str,
}

pub const NERD: Glyphs = Glyphs {
    lcap: "\u{e0b6}",
    rcap: "\u{e0b4}",
    db: "\u{f1c0}",
    restore: "\u{f006f}",
    server: "\u{f233}",
    modules: "\u{f1b3}",
    profiles: "\u{f1de}",
    ai: "\u{f06a9}",
    docker: "\u{f308}",
    local: "\u{f120}",
    log: "\u{f15c}",
    history: "\u{f1da}",
    info: "\u{f05a}",
    filter: "\u{f0b0}",
    backup: "\u{f187}",
};

pub const PLAIN: Glyphs = Glyphs {
    lcap: " ",
    rcap: " ",
    db: "◆",
    restore: "↺",
    server: "▣",
    modules: "▦",
    profiles: "≡",
    ai: "◉",
    docker: "◆",
    local: "›",
    log: "≣",
    history: "↺",
    info: "i",
    filter: "/",
    backup: "▤",
};

/// Pastilla redondeada: `(contenido)` con fondo `bg` sobre `outer` (o el
/// fondo de la terminal).
pub fn pill<'a>(
    segments: Vec<Span<'a>>,
    bg: Color,
    outer: Option<Color>,
    app: &App,
) -> Vec<Span<'a>> {
    let cap = |s: &'static str| {
        let mut st = Style::new().fg(bg);
        if let Some(o) = outer {
            st = st.bg(o);
        }
        Span::styled(s, st)
    };
    let mut out = vec![cap(app.glyphs.lcap)];
    out.extend(segments.into_iter().map(|s| {
        let st = s.style.bg(bg);
        s.style(st)
    }));
    out.push(cap(app.glyphs.rcap));
    out
}

pub fn spans_width(spans: &[Span]) -> u16 {
    spans.iter().map(|s| s.width() as u16).sum()
}

pub struct Btn<'a> {
    pub key: &'a str,
    pub label: &'a str,
    pub action: Action,
    pub enabled: bool,
    pub danger: bool,
}

impl<'a> Btn<'a> {
    pub fn new(key: &'a str, label: &'a str, action: Action) -> Self {
        Self {
            key,
            label,
            action,
            enabled: true,
            danger: false,
        }
    }

    pub fn danger(mut self) -> Self {
        self.danger = true;
        self
    }

    pub fn enabled(mut self, on: bool) -> Self {
        self.enabled = on;
        self
    }
}

/// Botón con tecla: `( r  Refrescar )`, la tecla como tapa de color.
fn button<'a>(b: &Btn<'a>, hovered: bool, app: &App) -> Vec<Span<'a>> {
    let t = &app.theme;
    let (keycap, label_fg) = match (b.enabled, b.danger) {
        (false, _) => (t.border, t.dim),
        (true, true) => (t.err, t.err),
        (true, false) => (t.accent, t.fg),
    };
    let body = if hovered && b.enabled {
        t.sel
    } else {
        t.surface
    };
    vec![
        Span::styled(app.glyphs.lcap, Style::new().fg(keycap)),
        Span::styled(
            format!("{} ", b.key),
            Style::new().fg(t.bg).bg(keycap).bold(),
        ),
        Span::styled(format!(" {} ", b.label), Style::new().fg(label_fg).bg(body)),
        Span::styled(app.glyphs.rcap, Style::new().fg(body)),
    ]
}

/// Fila de botones; los que no caben se omiten.
pub fn toolbar(f: &mut Frame, row: Rect, app: &App, btns: &[Btn], hits: &mut HitMap) {
    let mut x = row.x + 1;
    for b in btns {
        let w = spans_width(&button(b, false, app));
        if x + w > row.right() {
            break;
        }
        let r = Rect::new(x, row.y, w, 1);
        f.render_widget(Line::from(button(b, app.hovered(r), app)), r);
        if b.enabled {
            hits.push(r, b.action);
        }
        x += w + 1;
    }
}

/// Título de panel: ícono, texto y, si hay, una insignia (contador, spinner).
fn title<'a>(app: &App, icon: &str, text: &str, badge: Option<String>, focused: bool) -> Line<'a> {
    let t = &app.theme;
    let style = if focused {
        Style::new().fg(t.accent).bold()
    } else {
        Style::new().fg(t.dim).bold()
    };
    let mut spans = vec![Span::styled(format!(" {icon} {text} "), style)];
    if let Some(b) = badge {
        let bg = if focused { t.accent } else { t.border };
        let fg = if focused { t.bg } else { t.fg };
        spans.extend(pill(
            vec![Span::styled(b, Style::new().fg(fg).bold())],
            bg,
            None,
            app,
        ));
        spans.push(Span::raw(" "));
    }
    Line::from(spans)
}

/// Panel con borde redondeado; tocarlo lo enfoca. Devuelve el interior.
#[allow(clippy::too_many_arguments)]
pub fn frame(
    f: &mut Frame,
    area: Rect,
    app: &App,
    pane: Pane,
    icon: &str,
    text: &str,
    badge: Option<String>,
    hits: &mut HitMap,
) -> Rect {
    let t = &app.theme;
    let focused = app.focus == pane;
    let block = Block::bordered()
        .border_type(t.border_type)
        .border_style(if focused { t.accent } else { t.border })
        .title(title(app, icon, text, badge, focused));
    let inner = block.inner(area);
    f.render_widget(block, area);
    hits.push(area, Action::Focus(pane));
    inner
}

/// Tarjeta informativa (no enfocable).
pub fn card(f: &mut Frame, area: Rect, app: &App, icon: &str, text: &str) -> Rect {
    let t = &app.theme;
    let block = Block::bordered()
        .border_type(t.border_type)
        .border_style(t.border)
        .title(title(app, icon, text, None, false));
    let inner = block.inner(area);
    f.render_widget(block, area);
    inner
}

/// Cabecera de columnas de una lista.
pub fn header(f: &mut Frame, row: Rect, app: &App, text: String) {
    let t = &app.theme;
    f.buffer_mut().set_style(row, Style::new().bg(t.bar));
    f.render_widget(
        Line::styled(text, Style::new().fg(t.dim).bg(t.bar).bold()),
        row,
    );
}

/// Línea de filtro arriba de una lista o log; se reserva solo si hay filtro o
/// se está escribiendo. Devuelve el área restante.
pub fn filter_bar(f: &mut Frame, area: Rect, app: &App, pane: Pane, hits: &mut HitMap) -> Rect {
    let text = app.filter_text(pane);
    let editing = app.filtering == Some(pane);
    if text.is_empty() && !editing {
        return area;
    }
    let t = &app.theme;
    let [bar, rest] = Layout::vertical([Constraint::Length(1), Constraint::Fill(1)]).areas(area);
    let bg = if editing { t.sel } else { t.surface };
    let style = Style::new().fg(t.fg).bg(bg);
    f.buffer_mut().set_style(bar, style);
    let cursor = if editing { "▏" } else { "" };
    f.render_widget(
        Line::from(vec![
            Span::styled(
                format!(" {} ", app.glyphs.filter),
                Style::new().fg(t.accent).bg(bg).bold(),
            ),
            Span::styled(format!("{text}{cursor}"), style),
        ]),
        bar,
    );
    let hint = if editing {
        "Enter listo · Esc borrar "
    } else {
        "clic para editar "
    };
    f.render_widget(
        Line::styled(hint, Style::new().fg(t.dim).bg(bg)).right_aligned(),
        bar,
    );
    hits.push(bar, Action::Filter(pane));
    rest
}

/// Lista de filas de una línea: franjas alternas, selección, hover, scroll y ▲▼.
pub fn list(
    f: &mut Frame,
    area: Rect,
    app: &App,
    pane: Pane,
    rows: Vec<Line>,
    state: &ListState,
    hits: &mut HitMap,
) {
    let t = &app.theme;
    let h = area.height as usize;
    let max = rows.len().saturating_sub(h);
    let offset = state.offset.min(max);
    hits.scroll(area, pane, max, h);
    let focused = app.focus == pane;
    for (k, line) in rows.into_iter().enumerate().skip(offset).take(h) {
        let r = Rect::new(area.x, area.y + (k - offset) as u16, area.width, 1);
        let selected = k == state.sel;
        let bg = if selected {
            Some(if focused { t.sel } else { t.surface })
        } else if app.hovered(r) {
            Some(t.surface)
        } else if k % 2 == 1 {
            Some(t.stripe)
        } else {
            None
        };
        if let Some(bg) = bg {
            f.buffer_mut().set_style(r, Style::new().bg(bg));
        }
        let mark = if selected {
            Span::styled("▌", Style::new().fg(if focused { t.accent } else { t.dim }))
        } else {
            Span::raw(" ")
        };
        let mut spans = vec![mark];
        spans.extend(line.spans);
        f.render_widget(Line::from(spans), r);
        hits.push(r, Action::Row(pane, k));
    }
    scroll_marks(f, area, app, offset, max);
}

fn scroll_marks(f: &mut Frame, area: Rect, app: &App, offset: usize, max: usize) {
    if max == 0 || area.height == 0 || area.width == 0 {
        return;
    }
    let x = area.right() - 1;
    let style = Style::new().fg(app.theme.accent);
    if offset > 0 {
        f.render_widget(Span::styled("▲", style), Rect::new(x, area.y, 1, 1));
    }
    if offset < max {
        f.render_widget(
            Span::styled("▼", style),
            Rect::new(x, area.bottom() - 1, 1, 1),
        );
    }
}

pub fn level_style(app: &App, level: Level) -> Style {
    let t = &app.theme;
    match level {
        Level::Error => Style::new().fg(t.err),
        Level::Warn => Style::new().fg(t.warn),
        Level::Info => Style::new().fg(t.fg),
        Level::Debug => Style::new().fg(t.dim),
        Level::Cmd => Style::new().fg(t.dim).italic(),
        Level::Step => Style::new().fg(t.accent).bold(),
        Level::Plain => Style::new().fg(t.fg),
    }
}

/// Log con colores por nivel; sigue el final salvo que se haga scroll arriba.
pub fn log_view(
    f: &mut Frame,
    area: Rect,
    app: &App,
    pane: Pane,
    log: &LogView,
    empty: &str,
    hits: &mut HitMap,
) {
    let t = &app.theme;
    let lines = log.visible();
    let h = area.height as usize;
    let max = lines.len().saturating_sub(h);
    let offset = if log.follow { max } else { log.offset.min(max) };
    hits.scroll(area, pane, max, h);
    if lines.is_empty() {
        f.render_widget(Line::styled(empty, Style::new().fg(t.dim).italic()), area);
        return;
    }
    for (k, line) in lines.iter().skip(offset).take(h).enumerate() {
        let r = Rect::new(area.x, area.y + k as u16, area.width, 1);
        let level = log::level(line);
        if level == Level::Error {
            f.buffer_mut()
                .set_style(r, Style::new().bg(crate::theme::mix(t.bg, t.err, 0.10)));
        }
        let style = level_style(app, level);
        f.render_widget(Line::styled(truncate(line, area.width as usize), style), r);
    }
    // Indicador de si se sigue el final; tocarlo alterna.
    let (label, color) = if log.follow {
        ("⇣ siguiendo".to_string(), t.ok)
    } else {
        (
            format!("⏸ {}/{}", offset + h.min(lines.len()), lines.len()),
            t.warn,
        )
    };
    let spans = pill(
        vec![Span::styled(label, Style::new().fg(color).bold())],
        t.surface,
        None,
        app,
    );
    let w = spans_width(&spans);
    if area.width > w + 2 && area.height > 0 {
        let r = Rect::new(area.right() - w, area.bottom() - 1, w, 1);
        f.render_widget(Line::from(spans), r);
        hits.push(r, Action::Cmd(crate::input::Cmd::Follow));
    }
}

/// Recorta a `max` columnas con `…` al final.
pub fn truncate(s: &str, max: usize) -> String {
    if Line::raw(s).width() <= max {
        return s.into();
    }
    let mut out = String::new();
    let mut w = 0;
    for c in s.chars() {
        let cw = Line::raw(c.to_string()).width();
        if w + cw + 1 > max {
            break;
        }
        out.push(c);
        w += cw;
    }
    out.push('…');
    out
}

/// Celda alineada a la izquierda con ancho fijo.
pub fn cell(s: &str, w: usize) -> String {
    let s = truncate(s, w);
    let pad = w.saturating_sub(Line::raw(s.as_str()).width());
    format!("{s}{}", " ".repeat(pad))
}

/// Celda alineada a la derecha con ancho fijo.
pub fn rcell(s: &str, w: usize) -> String {
    let s = truncate(s, w);
    let pad = w.saturating_sub(Line::raw(s.as_str()).width());
    format!("{}{s}", " ".repeat(pad))
}
