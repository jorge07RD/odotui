//! Dibujo: cabecera con pestañas y perfil, cuerpo de la pestaña, pie con
//! ayuda, y encima los popups (ayuda, paleta, formulario, confirmación).

mod databases;
mod instance;
mod modules;
mod profiles;
mod restore;
pub mod widgets;

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
    let menu = Rect::new(row.x, row.y, 3, 1);
    let menu_bg = if app.hovered(menu) { t.border } else { t.surface };
    f.render_widget(
        Span::styled(" ≡ ", Style::new().fg(t.fg).bg(menu_bg).bold()),
        menu,
    );
    hits.push(menu, Action::Palette);
    let narrow = row.width < 90;
    let brand = if narrow { " ◆ " } else { " ◆ odotui " };
    f.render_widget(
        Span::styled(brand, Style::new().fg(t.accent).bold()),
        Rect {
            x: row.x + 3,
            width: row.width.saturating_sub(3),
            ..row
        },
    );

    // Derecha: tarea en curso y selector de perfil.
    let mut right = vec![];
    if let Some(b) = &app.busy {
        right.push(Span::styled(
            format!("{} {} {}s  ", spinner(app), b.label, b.started.elapsed().as_secs()),
            t.warn,
        ));
    }
    let pill = match app.current() {
        Some(p) => format!(" ◆ {} · {} ▾ ", p.name, p.mode.as_str()),
        None => " sin perfil ▾ ".into(),
    };
    let pill_w = Line::raw(pill.as_str()).width() as u16;
    let pill_r = Rect::new(row.right().saturating_sub(pill_w), row.y, pill_w, 1);
    let busy_w = Line::from(right.clone()).width() as u16;
    let pill_bg = if app.hovered(pill_r) { t.border } else { t.surface };
    f.render_widget(
        Span::styled(pill, Style::new().fg(t.accent).bg(pill_bg).bold()),
        pill_r,
    );
    hits.push(pill_r, Action::NextProfile);
    let busy_r = Rect::new(pill_r.x.saturating_sub(busy_w), row.y, busy_w, 1);
    if !narrow {
        f.render_widget(Line::from(right), busy_r);
        if app.busy.is_some() {
            hits.push(busy_r, Action::Cmd(crate::input::Cmd::Cancel));
        }
    }

    let mut x = row.x + 3 + Line::raw(brand).width() as u16;
    let limit = if narrow { pill_r.x } else { busy_r.x };
    for (i, tab) in Tab::ALL.iter().enumerate() {
        let label = if narrow {
            format!(" {} ", i + 1)
        } else {
            format!(" {} {} ", i + 1, tab.title())
        };
        let w = Line::raw(label.as_str()).width() as u16;
        if x + w > limit {
            break;
        }
        let r = Rect::new(x, row.y, w, 1);
        let style = if *tab == app.tab {
            Style::new().fg(t.surface).bg(t.accent).bold()
        } else if app.hovered(r) {
            Style::new().fg(t.fg).bg(t.border)
        } else {
            Style::new().fg(t.dim)
        };
        f.render_widget(Span::styled(label, style), r);
        hits.push(r, Action::Tab(*tab));
        x += w + 1;
    }
}

fn draw_footer(f: &mut Frame, row: Rect, app: &App, hits: &mut HitMap) {
    let t = &app.theme;
    let buttons: &[(&str, Action)] = &[
        (" : comandos ", Action::Palette),
        (" ? ayuda ", Action::Help),
        (" q salir ", Action::Quit),
    ];
    let mut x = row.right();
    for (label, action) in buttons.iter().rev() {
        let w = Line::raw(*label).width() as u16;
        x = x.saturating_sub(w);
        let r = Rect::new(x, row.y, w, 1);
        let bg = if app.hovered(r) { t.border } else { t.surface };
        f.render_widget(Span::styled(*label, Style::new().fg(t.fg).bg(bg)), r);
        hits.push(r, *action);
        x = x.saturating_sub(1);
    }
    let hint = match app.tab {
        Tab::Dbs => "clic: seleccionar · 2º clic/Enter: módulos · ←→ panel · / filtrar",
        Tab::Restore => "2º clic/Enter en un backup: restaurar · / filtrar · rueda: scroll",
        Tab::Instance => "rueda/arrastre: scroll · f seguir · / filtrar",
        Tab::Modules => "clic en [ ] o Espacio: marcar · i instalar · u actualizar",
        Tab::Profiles => "2º clic/Enter: usar perfil · p o clic en el perfil arriba: cambiar",
    };
    let left = Rect {
        width: x.saturating_sub(row.x),
        ..row
    };
    f.render_widget(
        Span::styled(widgets::truncate(&format!(" {hint}"), left.width as usize), t.dim),
        left,
    );
}

fn draw_toast(f: &mut Frame, body: Rect, app: &App) {
    let Some((msg, at)) = &app.toast else { return };
    if at.elapsed() > TOAST || body.height < 3 {
        return;
    }
    let t = &app.theme;
    let text = widgets::truncate(&format!("  {msg}  "), body.width as usize);
    let w = Line::raw(text.as_str()).width() as u16;
    let r = Rect::new(body.x + (body.width - w) / 2, body.bottom() - 2, w, 1);
    let color = if msg.starts_with('✗') { t.err } else { t.fg };
    f.render_widget(Clear, r);
    f.render_widget(
        Paragraph::new(text).style(Style::new().fg(color).bg(t.surface).bold()),
        r,
    );
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

pub fn card<'a>(app: &App, title: Line<'a>) -> Block<'a> {
    Block::bordered()
        .border_type(app.theme.border_type)
        .border_style(app.theme.accent)
        .style(Style::new().bg(app.theme.surface))
        .title(title)
}

/// Botones grandes de los popups.
fn button_row(f: &mut Frame, row: Rect, app: &App, buttons: &[(&str, Action)], hits: &mut HitMap) {
    let t = &app.theme;
    let mut x = row.x;
    for (label, action) in buttons {
        let w = Line::raw(*label).width() as u16;
        if x + w > row.right() {
            break;
        }
        let r = Rect::new(x, row.y, w, 1);
        let style = if app.hovered(r) {
            Style::new().fg(t.surface).bg(t.fg).bold()
        } else {
            Style::new().fg(t.surface).bg(t.accent).bold()
        };
        f.render_widget(Span::styled(*label, style), r);
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
    f.render_widget(Clear, area);
    let block = card(app, Line::styled(" Ayuda ", Style::new().fg(t.fg).bold()));
    let inner = block.inner(area).inner(Margin::new(1, 0));
    f.render_widget(block, area);
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
    let area = Rect { y: screen.y + 2, ..area };
    hits.modal(screen, area);
    f.render_widget(Clear, area);
    let block = card(app, Line::styled(" Comandos ", Style::new().fg(t.fg).bold()));
    let inner = block.inner(area);
    f.render_widget(block, area);
    let [query, list] =
        Layout::vertical([Constraint::Length(1), Constraint::Fill(1)]).areas(inner);
    f.buffer_mut().set_style(query, Style::new().bg(t.border));
    f.render_widget(
        Line::from(vec![
            Span::styled(" › ", Style::new().fg(t.accent).bg(t.border).bold()),
            Span::styled(format!("{}▏", pal.query), Style::new().fg(t.fg).bg(t.border)),
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
    // Alto justo para el contenido (bordes + campos + error + botones).
    let area = popup(screen, 72, total + err_h + 3);
    hits.modal_strict(screen);
    f.render_widget(Clear, area);
    let block = card(
        app,
        Line::styled(format!(" ✎ {} ", form.title), Style::new().fg(t.fg).bold()),
    );
    let inner = block.inner(area).inner(Margin::new(1, 0));
    f.render_widget(block, area);

    let [body, error, buttons] = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(err_h),
        Constraint::Length(1),
    ])
    .areas(inner);
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
    f.render_widget(Clear, area);
    let block = card(app, Line::styled(" Confirmar ", Style::new().fg(t.warn).bold()));
    let inner = block.inner(area).inner(Margin::new(1, 0));
    f.render_widget(block, area);
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
