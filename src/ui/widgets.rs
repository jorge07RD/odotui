//! Piezas reutilizables: botones, paneles con borde, listas y logs. Todas
//! registran sus áreas en el `HitMap` mientras se dibujan.

use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::Style,
    text::{Line, Span},
    widgets::Block,
};

use crate::{
    app::App,
    input::{Action, HitMap, Pane},
    log::{self, Level, ListState, LogView},
};

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

/// Fila de botones `[tecla] etiqueta`; los que no caben se omiten.
pub fn toolbar(f: &mut Frame, row: Rect, app: &App, btns: &[Btn], hits: &mut HitMap) {
    let t = &app.theme;
    let mut x = row.x;
    for b in btns {
        let w = (Line::raw(b.key).width() + Line::raw(b.label).width() + 3) as u16;
        if x + w > row.right() {
            break;
        }
        let r = Rect::new(x, row.y, w, 1);
        let bg = if b.enabled && app.hovered(r) {
            t.border
        } else {
            t.surface
        };
        let (kc, lc) = match (b.enabled, b.danger) {
            (false, _) => (t.dim, t.dim),
            (true, true) => (t.err, t.err),
            (true, false) => (t.accent, t.fg),
        };
        f.render_widget(
            Line::from(vec![
                Span::styled(format!(" {}", b.key), Style::new().fg(kc).bg(bg).bold()),
                Span::styled(format!(" {} ", b.label), Style::new().fg(lc).bg(bg)),
            ]),
            r,
        );
        if b.enabled {
            hits.push(r, b.action);
        }
        x += w + 1;
    }
}

/// Panel con borde; tocarlo lo enfoca. Devuelve el interior.
pub fn frame(
    f: &mut Frame,
    area: Rect,
    app: &App,
    pane: Pane,
    title: Line,
    hits: &mut HitMap,
) -> Rect {
    let t = &app.theme;
    let focused = app.focus == pane;
    let border = if focused { t.accent } else { t.border };
    let block = Block::bordered()
        .border_type(t.border_type)
        .border_style(border)
        .title(title.style(if focused {
            Style::new().fg(t.accent).bold()
        } else {
            Style::new().fg(t.dim)
        }));
    let inner = block.inner(area);
    f.render_widget(block, area);
    hits.push(area, Action::Focus(pane));
    inner
}

/// Línea de filtro `/ texto` arriba de una lista o log; se reserva solo si
/// hay filtro o se está escribiendo. Devuelve el área restante.
pub fn filter_bar(f: &mut Frame, area: Rect, app: &App, pane: Pane, hits: &mut HitMap) -> Rect {
    let text = app.filter_text(pane);
    let editing = app.filtering == Some(pane);
    if text.is_empty() && !editing {
        return area;
    }
    let t = &app.theme;
    let [bar, rest] = Layout::vertical([Constraint::Length(1), Constraint::Fill(1)]).areas(area);
    let style = Style::new().fg(t.fg).bg(t.border);
    f.buffer_mut().set_style(bar, style);
    let cursor = if editing { "▏" } else { "" };
    f.render_widget(
        Line::from(vec![
            Span::styled(" / ", Style::new().fg(t.accent).bg(t.border).bold()),
            Span::styled(format!("{text}{cursor}"), style),
        ]),
        bar,
    );
    let hint = if editing { "Enter listo · Esc borrar " } else { "clic para editar " };
    f.render_widget(Line::styled(hint, Style::new().fg(t.dim).bg(t.border)).right_aligned(), bar);
    hits.push(bar, Action::Filter(pane));
    rest
}

/// Lista de filas de una línea con selección, hover, scroll y ▲▼.
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
            Some(if focused { t.surface } else { t.border })
        } else if app.hovered(r) {
            Some(t.border)
        } else {
            None
        };
        if let Some(bg) = bg {
            f.buffer_mut().set_style(r, Style::new().bg(bg));
        }
        let mark = if selected && focused {
            Span::styled("▍", Style::new().fg(t.accent))
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
        f.render_widget(Span::styled("▼", style), Rect::new(x, area.bottom() - 1, 1, 1));
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
        f.render_widget(Line::styled(empty, t.dim), area);
        return;
    }
    for (k, line) in lines.iter().skip(offset).take(h).enumerate() {
        let r = Rect::new(area.x, area.y + k as u16, area.width, 1);
        let style = level_style(app, log::level(line));
        f.render_widget(Line::styled(truncate(line, area.width as usize), style), r);
    }
    // Indicador de si se sigue el final; tocarlo alterna.
    let (label, color) = if log.follow {
        (" ⇣ siguiendo ".to_string(), t.ok)
    } else {
        (format!(" ⏸ {}/{} ", offset + h.min(lines.len()), lines.len()), t.warn)
    };
    let w = Line::raw(label.as_str()).width() as u16;
    if area.width > w + 2 && area.height > 0 {
        let r = Rect::new(area.right() - w, area.bottom() - 1, w, 1);
        f.render_widget(Span::styled(label, Style::new().fg(color).bg(t.surface)), r);
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
