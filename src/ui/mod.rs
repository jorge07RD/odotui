//! Dibujo: cabecera con pestañas y perfil, cuerpo de la pestaña, pie con
//! ayuda, y encima los popups (ayuda, paleta, formulario, confirmación).

mod databases;
mod instance;
mod modules;
mod profiles;
mod restore;
pub mod widgets;

#[cfg(test)]
mod tests;

use std::time::Duration;

use ratatui::{
    Frame,
    layout::{Constraint, Layout, Margin, Rect},
    style::Style,
    text::{Line, Span},
    widgets::{Block, Clear, Paragraph, Wrap},
};

use crate::{
    app::App,
    form::{FieldKind, Form},
    input::{Action, HitMap, Pane, Tab},
};

const TOAST: Duration = Duration::from_secs(5);
const SPINNER: [&str; 8] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧"];

pub fn spinner(app: &App) -> &'static str {
    SPINNER[app.spin % SPINNER.len()]
}

pub fn draw(f: &mut Frame, app: &App, hits: &mut HitMap) {
    let area = f.area();
    let [header, body, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(1),
    ])
    .areas(area);

    draw_header(f, header, app, hits);
    match app.tab {
        Tab::Dbs => databases::draw(f, body, app, hits),
        Tab::Restore => restore::draw(f, body, app, hits),
        Tab::Instance => instance::draw(f, body, app, hits),
        Tab::Modules => modules::draw(f, body, app, hits),
        Tab::Profiles => profiles::draw(f, body, app, hits),
    }
    draw_footer(f, footer, app, hits);

    if app.help {
        draw_help(f, app, hits);
    }
    if app.palette.is_some() {
        draw_palette(f, app, hits);
    }
    if let Some(form) = &app.form {
        draw_form(f, app, form, hits);
    }
    if let Some(c) = &app.confirm {
        draw_confirm(f, app, &c.message, hits);
    }
    draw_toast(f, body, app);
}

fn draw_header(f: &mut Frame, row: Rect, app: &App, hits: &mut HitMap) {
    let t = &app.theme;
    let g = app.glyphs;
    f.buffer_mut().set_style(row, Style::new().bg(t.bar));
    let on_bar = |c| Style::new().fg(c).bg(t.bar);

    let menu = Rect::new(row.x, row.y, 3, 1);
    let menu_bg = if app.hovered(menu) { t.sel } else { t.bar };
    f.render_widget(
        Span::styled(" ≡ ", Style::new().fg(t.fg).bg(menu_bg).bold()),
        menu,
    );
    hits.push(menu, Action::Palette);
    let narrow = row.width < 100;
    let brand = if narrow { " ◆ " } else { " ◆ odotui " };
    f.render_widget(
        Span::styled(brand, on_bar(t.accent).bold()),
        Rect {
            x: row.x + 3,
            width: row.width.saturating_sub(3),
            ..row
        },
    );

    // Derecha: IA, tarea en curso y perfil.
    let pill = match app.current() {
        Some(p) => {
            let icon = if p.is_docker() { g.docker } else { g.local };
            widgets::pill(
                vec![Span::styled(
                    format!("{icon} {} ▾", p.name),
                    Style::new().fg(t.fg).bold(),
                )],
                t.surface,
                Some(t.bar),
                app,
            )
        }
        None => widgets::pill(
            vec![Span::styled("sin perfil ▾", Style::new().fg(t.warn))],
            t.surface,
            Some(t.bar),
            app,
        ),
    };
    let pill_w = widgets::spans_width(&pill);
    let pill_r = Rect::new(row.right().saturating_sub(pill_w + 1), row.y, pill_w, 1);
    let pill = if app.hovered(pill_r) {
        pill.into_iter()
            .map(|s| {
                let st = if s.style.bg == Some(t.surface) {
                    s.style.bg(t.sel)
                } else if s.style.fg == Some(t.surface) {
                    s.style.fg(t.sel)
                } else {
                    s.style
                };
                s.style(st)
            })
            .collect()
    } else {
        pill
    };
    f.render_widget(Line::from(pill), pill_r);
    hits.push(pill_r, Action::NextProfile);

    let mut right: Vec<Span> = vec![];
    if app.ai_active() {
        right.extend(widgets::pill(
            vec![Span::styled(
                format!("{} IA", g.ai),
                Style::new().fg(t.bg).bold(),
            )],
            t.warn,
            Some(t.bar),
            app,
        ));
        right.push(Span::styled(" ", on_bar(t.fg)));
    } else if app.ai_listening {
        right.push(Span::styled(format!("{} IA  ", g.ai), on_bar(t.dim)));
    }
    if let Some(b) = &app.busy {
        right.push(Span::styled(
            format!(
                "{} {} {}s  ",
                spinner(app),
                b.label,
                b.started.elapsed().as_secs()
            ),
            on_bar(t.warn),
        ));
    }
    let busy_w = widgets::spans_width(&right);
    let busy_r = Rect::new(pill_r.x.saturating_sub(busy_w + 1), row.y, busy_w, 1);
    if !narrow {
        f.render_widget(Line::from(right), busy_r);
        if app.busy.is_some() {
            hits.push(busy_r, Action::Cmd(crate::input::Cmd::Cancel));
        }
    }

    // Pestañas: la activa es una pastilla de acento.
    let icons = [g.db, g.restore, g.server, g.modules, g.profiles];
    let mut x = row.x + 3 + Line::raw(brand).width() as u16 + 1;
    let limit = if narrow { pill_r.x } else { busy_r.x };
    for (i, tab) in Tab::ALL.iter().enumerate() {
        let text = if narrow {
            format!("{} {}", i + 1, icons[i])
        } else {
            format!("{} {} {}", i + 1, icons[i], tab.title())
        };
        let spans = if *tab == app.tab {
            widgets::pill(
                vec![Span::styled(text.clone(), Style::new().fg(t.bg).bold())],
                t.accent,
                Some(t.bar),
                app,
            )
        } else {
            vec![Span::styled(format!(" {text} "), on_bar(t.dim))]
        };
        let w = widgets::spans_width(&spans);
        if x + w > limit {
            break;
        }
        let r = Rect::new(x, row.y, w, 1);
        let spans = if *tab != app.tab && app.hovered(r) {
            widgets::pill(
                vec![Span::styled(text, Style::new().fg(t.fg))],
                t.surface,
                Some(t.bar),
                app,
            )
        } else {
            spans
        };
        f.render_widget(Line::from(spans), r);
        hits.push(r, Action::Tab(*tab));
        x += w + 1;
    }
}

fn draw_footer(f: &mut Frame, row: Rect, app: &App, hits: &mut HitMap) {
    let t = &app.theme;
    f.buffer_mut().set_style(row, Style::new().bg(t.bar));
    let buttons: &[(&str, &str, Action)] = &[
        (":", "comandos", Action::Palette),
        ("?", "ayuda", Action::Help),
        ("q", "salir", Action::Quit),
    ];
    let mut x = row.right().saturating_sub(1);
    for (key, label, action) in buttons.iter().rev() {
        let spans = |hover: bool| {
            widgets::pill(
                vec![
                    Span::styled(format!("{key} "), Style::new().fg(t.accent).bold()),
                    Span::styled(*label, Style::new().fg(t.fg)),
                ],
                if hover { t.sel } else { t.surface },
                Some(t.bar),
                app,
            )
        };
        let w = widgets::spans_width(&spans(false));
        x = x.saturating_sub(w);
        let r = Rect::new(x, row.y, w, 1);
        f.render_widget(Line::from(spans(app.hovered(r))), r);
        hits.push(r, *action);
        x = x.saturating_sub(1);
    }
    let hints: &[(&str, &str)] = match app.tab {
        Tab::Dbs => &[
            ("Enter", "módulos"),
            ("/", "filtrar"),
            ("←→", "panel"),
            ("2º clic", "abrir"),
        ],
        Tab::Restore => &[
            ("Enter", "restaurar"),
            ("/", "filtrar"),
            ("X", "cancelar"),
            ("rueda", "scroll"),
        ],
        Tab::Instance => &[
            ("f", "seguir"),
            ("/", "filtrar"),
            ("rueda", "scroll"),
            ("o", "abrir web"),
        ],
        Tab::Modules => &[
            ("Espacio", "marcar"),
            ("i", "instalar"),
            ("u", "actualizar"),
            ("b", "otra BD"),
        ],
        Tab::Profiles => &[
            ("Enter", "usar"),
            ("p", "siguiente perfil"),
            ("t", "probar"),
        ],
    };
    let mut spans = vec![Span::raw(" ")];
    for (k, d) in hints {
        spans.push(Span::styled(*k, Style::new().fg(t.accent).bg(t.bar).bold()));
        spans.push(Span::styled(
            format!(" {d}   "),
            Style::new().fg(t.dim).bg(t.bar),
        ));
    }
    let left = Rect {
        width: x.saturating_sub(row.x),
        ..row
    };
    f.render_widget(Line::from(spans), left);
}

fn draw_toast(f: &mut Frame, body: Rect, app: &App) {
    let Some((msg, at)) = &app.toast else { return };
    if at.elapsed() > TOAST || body.height < 3 {
        return;
    }
    let t = &app.theme;
    let (color, msg) = if let Some(m) = msg.strip_prefix("✗ ") {
        (t.err, format!("✗ {m}"))
    } else if let Some(m) = msg.strip_prefix("✓ ") {
        (t.ok, format!("✓ {m}"))
    } else {
        (t.accent, format!("● {msg}"))
    };
    let text = widgets::truncate(&msg, body.width.saturating_sub(6) as usize);
    // Pastilla con el tipo marcado en color a la izquierda.
    let spans = widgets::pill(
        vec![Span::styled(text, Style::new().fg(color).bold())],
        t.sel,
        None,
        app,
    );
    let w = widgets::spans_width(&spans);
    let r = Rect::new(
        body.x + (body.width.saturating_sub(w)) / 2,
        body.bottom() - 2,
        w,
        1,
    );
    f.render_widget(Clear, r);
    f.render_widget(Line::from(spans), r);
}

pub fn popup(screen: Rect, max_w: u16, max_h: u16) -> Rect {
    let w = screen.width.saturating_sub(2).min(max_w);
    let h = screen.height.saturating_sub(2).min(max_h);
    Rect::new(
        screen.x + (screen.width - w) / 2,
        screen.y + (screen.height - h) / 2,
        w,
        h,
    )
}

/// Marco de popup: borde redondeado de acento sobre el fondo de la terminal
/// y el interior relleno, así las esquinas quedan limpias. Devuelve el interior.
pub fn popup_frame(f: &mut Frame, area: Rect, app: &App, title: Line) -> Rect {
    let t = &app.theme;
    f.render_widget(Clear, area);
    let block = Block::bordered()
        .border_type(t.border_type)
        .border_style(t.accent)
        .title(title);
    let inner = block.inner(area);
    f.render_widget(block, area);
    f.buffer_mut().set_style(inner, Style::new().bg(t.surface));
    inner
}

/// Botones de los popups: el primero es el principal (relleno de acento).
fn button_row(f: &mut Frame, row: Rect, app: &App, buttons: &[(&str, Action)], hits: &mut HitMap) {
    let t = &app.theme;
    let mut x = row.x;
    for (i, (label, action)) in buttons.iter().enumerate() {
        let make = |hover: bool| {
            let (fg, bg) = match (i, hover) {
                (0, false) => (t.bg, t.accent),
                (0, true) => (t.bg, t.fg),
                (_, false) => (t.fg, t.sel),
                (_, true) => (t.bg, t.fg),
            };
            widgets::pill(
                vec![Span::styled(
                    label.trim().to_string(),
                    Style::new().fg(fg).bold(),
                )],
                bg,
                Some(t.surface),
                app,
            )
        };
        let w = widgets::spans_width(&make(false));
        if x + w > row.right() {
            break;
        }
        let r = Rect::new(x, row.y, w, 1);
        f.render_widget(Line::from(make(app.hovered(r))), r);
        hits.push(r, *action);
        x += w + 2;
    }
}

fn draw_help(f: &mut Frame, app: &App, hits: &mut HitMap) {
    let t = &app.theme;
    let screen = f.area();
    let item = |k: &'static str, d: &'static str| {
        Line::from(vec![
            Span::styled(format!("{k:<16}"), t.accent),
            Span::styled(d, t.fg),
        ])
    };
    let head = |s: &'static str| Line::styled(s, Style::new().fg(t.dim).bold());
    let lines = vec![
        head("Mouse"),
        item("pestañas", "cambian de herramienta"),
        item("◆ perfil ▾", "pasa al siguiente perfil"),
        item("fila", "selecciona; 2º clic la abre"),
        item("botones", "cada uno muestra su tecla"),
        item("rueda/arrastre", "scroll de listas y logs"),
        item("⇣ siguiendo", "pausa o sigue el final del log"),
        item("≡", "paleta de comandos"),
        item("◉ IA", "una IA está trabajando (odotui mcp)"),
        Line::raw(""),
        head("Teclado"),
        item("Tab  1-5", "pestañas · p perfil siguiente"),
        item("↑↓ jk  PgUp/Dn", "mover · Home/End extremos"),
        item("←→ hl", "cambiar de panel"),
        item("Enter", "abrir la fila seleccionada"),
        item("/  Esc", "filtrar · borrar filtro / cerrar"),
        item(":", "paleta de comandos (escribe para buscar)"),
        item("f  C", "seguir log · limpiar log"),
        item("X", "cancelar la tarea en curso"),
        item("q  Ctrl+C", "salir"),
        Line::raw(""),
        head("Formularios"),
        item("Tab ↑↓", "campo · ←→/Espacio cambia opción o casilla"),
        item("Enter  Esc", "guardar · cancelar"),
    ];
    // Alto justo: bordes + líneas + hueco + botón.
    let area = popup(screen, 72, lines.len() as u16 + 4);
    hits.modal(screen, area);
    let inner = popup_frame(
        f,
        area,
        app,
        Line::styled(" Ayuda ", Style::new().fg(t.fg).bold()),
    )
    .inner(Margin::new(1, 0));
    let [text, buttons] =
        Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(inner);
    f.render_widget(Paragraph::new(lines), text);
    button_row(f, buttons, app, &[(" ✕ Cerrar ", Action::Close)], hits);
}

fn draw_palette(f: &mut Frame, app: &App, hits: &mut HitMap) {
    let Some(pal) = &app.palette else { return };
    let t = &app.theme;
    let screen = f.area();
    let area = popup(screen, 64, 20);
    let area = Rect {
        y: screen.y + 2,
        ..area
    };
    hits.modal(screen, area);
    let inner = popup_frame(
        f,
        area,
        app,
        Line::styled(" Comandos ", Style::new().fg(t.fg).bold()),
    );
    let [query, list] = Layout::vertical([Constraint::Length(1), Constraint::Fill(1)]).areas(inner);
    f.buffer_mut().set_style(query, Style::new().bg(t.border));
    f.render_widget(
        Line::from(vec![
            Span::styled(" › ", Style::new().fg(t.accent).bg(t.border).bold()),
            Span::styled(
                format!("{}▏", pal.query),
                Style::new().fg(t.fg).bg(t.border),
            ),
        ]),
        query,
    );
    let items = app.palette_items();
    let h = list.height as usize;
    let max = items.len().saturating_sub(h);
    let offset = pal.offset.min(max);
    hits.scroll(list, Pane::Palette, max, h);
    for (k, (label, _)) in items.iter().enumerate().skip(offset).take(h) {
        let r = Rect::new(list.x, list.y + (k - offset) as u16, list.width, 1);
        let style = if k == pal.sel {
            Style::new().fg(t.surface).bg(t.accent).bold()
        } else if app.hovered(r) {
            Style::new().fg(t.fg).bg(t.border)
        } else {
            Style::new().fg(t.fg)
        };
        f.buffer_mut().set_style(r, style);
        f.render_widget(Span::styled(format!(" {label}"), style), r);
        hits.push(r, Action::Row(Pane::Palette, k));
    }
    if items.is_empty() {
        f.render_widget(Line::styled(" sin resultados", t.dim), list);
    }
}

/// Formulario: una etiqueta y un control por campo, con botones fijos abajo.
fn draw_form(f: &mut Frame, app: &App, form: &Form, hits: &mut HitMap) {
    let t = &app.theme;
    let screen = f.area();
    // Texto: etiqueta + control + hueco; casilla: una fila + hueco.
    let heights: Vec<u16> = form
        .fields
        .iter()
        .map(|fl| if fl.kind == FieldKind::Check { 2 } else { 3 })
        .collect();
    let total: u16 = heights.iter().sum();
    let err_h = u16::from(form.error.is_some()) * 2;
    let note_h = form
        .note
        .as_ref()
        .map_or(0, |n| n.lines().count() as u16 + 1);
    // Alto justo para el contenido (bordes + aviso + campos + error + botones).
    let area = popup(screen, 72, note_h + total + err_h + 3);
    hits.modal_strict(screen);
    let inner = popup_frame(
        f,
        area,
        app,
        Line::styled(format!(" ✎ {} ", form.title), Style::new().fg(t.fg).bold()),
    )
    .inner(Margin::new(1, 0));

    let [note, body, error, buttons] = Layout::vertical([
        Constraint::Length(note_h),
        Constraint::Fill(1),
        Constraint::Length(err_h),
        Constraint::Length(1),
    ])
    .areas(inner);
    if let Some(n) = &form.note {
        let lines: Vec<Line> = n
            .lines()
            .map(|l| {
                let c = if l.starts_with('⚠') { t.warn } else { t.dim };
                Line::styled(l.to_string(), Style::new().fg(c))
            })
            .collect();
        f.render_widget(Paragraph::new(lines), note);
    }
    let max = total.saturating_sub(body.height) as usize;
    // Sin scroll manual, el campo enfocado queda a la vista.
    let focus_top: u16 = heights[..form.focus.min(heights.len())].iter().sum();
    let focus_bottom = focus_top + heights.get(form.focus).copied().unwrap_or(0);
    let mut scroll = form.scroll.min(max) as u16;
    if focus_bottom > scroll + body.height {
        scroll = focus_bottom.saturating_sub(body.height);
    }
    if focus_top < scroll {
        scroll = focus_top;
    }

    let mut y = body.y as i32 - i32::from(scroll);
    for (i, fl) in form.fields.iter().enumerate() {
        let focused = i == form.focus;
        let row = |dy: i32| -> Option<Rect> {
            let yy = y + dy;
            (yy >= body.y as i32 && yy < body.bottom() as i32)
                .then(|| Rect::new(body.x, yy as u16, body.width, 1))
        };
        let label_style = if focused {
            Style::new().fg(t.accent).bold()
        } else {
            Style::new().fg(t.dim)
        };
        let field_bg = Style::new().fg(t.fg).bg(t.border);
        let bar = |f: &mut Frame, r: Rect| {
            if focused {
                f.render_widget(
                    Span::styled("▍", Style::new().fg(t.accent).bg(t.border)),
                    Rect { width: 1, ..r },
                );
            }
        };
        match &fl.kind {
            FieldKind::Check => {
                if let Some(r) = row(0) {
                    let on = fl.checked();
                    if focused || app.hovered(r) {
                        f.buffer_mut().set_style(r, Style::new().bg(t.border));
                    }
                    f.render_widget(
                        Line::from(vec![
                            Span::styled(
                                if on { " [✓] " } else { " [ ] " },
                                if on { t.ok } else { t.dim },
                            ),
                            Span::styled(fl.label, label_style),
                        ]),
                        r,
                    );
                    f.render_widget(Line::styled(fl.help, t.dim).right_aligned(), r);
                    bar(f, r);
                    hits.push(r, Action::FormNext(i));
                }
            }
            FieldKind::Text | FieldKind::Password | FieldKind::Number => {
                if let Some(r) = row(0) {
                    f.render_widget(Span::styled(fl.label, label_style), r);
                    f.render_widget(Line::styled(fl.help, t.dim).right_aligned(), r);
                    hits.push(r, Action::FormFocus(i));
                }
                if let Some(r) = row(1) {
                    f.buffer_mut().set_style(r, field_bg);
                    let mut text = if fl.kind == FieldKind::Password {
                        "•".repeat(fl.value.chars().count())
                    } else {
                        fl.value.clone()
                    };
                    if focused {
                        let at = text
                            .char_indices()
                            .nth(fl.cursor)
                            .map_or(text.len(), |(b, _)| b);
                        text.insert(at, '▏');
                    }
                    // Si no cabe, se muestra el final (donde suele estar el cursor).
                    let w = r.width.saturating_sub(2) as usize;
                    let chars: Vec<char> = text.chars().collect();
                    let shown: String = chars[chars.len().saturating_sub(w)..].iter().collect();
                    f.render_widget(Span::styled(format!(" {shown}"), field_bg), r);
                    bar(f, r);
                    hits.push(r, Action::FormFocus(i));
                }
            }
            FieldKind::Select(_) => {
                if let Some(r) = row(0) {
                    f.render_widget(Span::styled(fl.label, label_style), r);
                    hits.push(r, Action::FormFocus(i));
                }
                if let Some(r) = row(1) {
                    f.buffer_mut().set_style(r, field_bg);
                    f.render_widget(Line::styled(fl.value.as_str(), field_bg).centered(), r);
                    let arrow = Style::new().fg(t.accent).bg(t.border).bold();
                    f.render_widget(Span::styled(" ◄ ", arrow), r);
                    f.render_widget(Line::styled(" ► ", arrow).right_aligned(), r);
                    bar(f, r);
                    hits.push(r, Action::FormNext(i));
                    hits.push(Rect { width: 5, ..r }, Action::FormPrev(i));
                    hits.push(
                        Rect::new(r.right().saturating_sub(5), r.y, 5, 1),
                        Action::FormNext(i),
                    );
                }
            }
        }
        y += i32::from(heights[i]);
    }
    hits.scroll(body, Pane::Form, max, body.height as usize);

    if let Some(err) = &form.error {
        f.render_widget(
            Paragraph::new(format!("✗ {err}"))
                .style(Style::new().fg(t.err))
                .wrap(Wrap { trim: true }),
            error,
        );
    }
    button_row(
        f,
        buttons,
        app,
        &[
            (" ✓ Guardar (Enter) ", Action::FormSave),
            (" ✕ Cancelar (Esc) ", Action::FormCancel),
        ],
        hits,
    );
}

fn draw_confirm(f: &mut Frame, app: &App, message: &str, hits: &mut HitMap) {
    let t = &app.theme;
    let screen = f.area();
    let area = popup(screen, 60, 8);
    hits.modal(screen, area);
    let inner = popup_frame(
        f,
        area,
        app,
        Line::styled(" Confirmar ", Style::new().fg(t.warn).bold()),
    )
    .inner(Margin::new(1, 0));
    let [text, buttons] =
        Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(inner);
    f.render_widget(
        Paragraph::new(message)
            .style(Style::new().fg(t.fg))
            .wrap(Wrap { trim: true }),
        text,
    );
    button_row(
        f,
        buttons,
        app,
        &[
            (" ✓ Sí (s) ", Action::ConfirmYes),
            (" ✕ No (n) ", Action::ConfirmNo),
        ],
        hits,
    );
}
