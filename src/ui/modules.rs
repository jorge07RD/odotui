//! Pestaña Módulos: elegir BD, marcar módulos e instalar/actualizar.

use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::Style,
    text::{Line, Span},
};

use super::widgets::{self, Btn, cell};
use crate::{
    app::App,
    input::{Action, Cmd, HitMap, ModView, Pane},
    jobs::Sink,
    odoo::modules::{self, ModuleInfo},
};

/// Barra de búsqueda, siempre visible: clic o `/` para escribir.
fn search_bar(f: &mut Frame, area: Rect, app: &App, shown: usize, hits: &mut HitMap) {
    let t = &app.theme;
    let text = &app.mod_list.filter;
    let editing = app.filtering == Some(Pane::ModList);
    let bg = if editing { t.sel } else { t.surface };
    f.buffer_mut().set_style(area, Style::new().bg(bg));
    let mut spans = vec![Span::styled(
        format!(" {} ", app.glyphs.filter),
        Style::new().fg(t.accent).bg(bg).bold(),
    )];
    if text.is_empty() && !editing {
        spans.push(Span::styled(
            "Buscar módulo: nombre, título o categoría  (/)",
            Style::new().fg(t.dim).bg(bg).italic(),
        ));
    } else {
        spans.push(Span::styled(
            text.clone(),
            Style::new().fg(t.fg).bg(bg).bold(),
        ));
        if editing {
            spans.push(Span::styled("▏", Style::new().fg(t.accent).bg(bg)));
        }
    }
    f.render_widget(Line::from(spans), area);
    let total = app.modules.len();
    let mut right = format!("{shown} de {total} ");
    if editing {
        right = format!("{shown} de {total} · ↑↓ elegir · Enter · Esc borra ");
    } else if !text.is_empty() {
        right = format!("{shown} de {total} · Esc borra ");
    }
    f.render_widget(
        Line::styled(right, Style::new().fg(t.dim).bg(bg)).right_aligned(),
        area,
    );
    hits.push(area, Action::Filter(Pane::ModList));
}

/// Chips de estado (clic o `s`), con cuántos de cada uno coinciden.
fn chips(f: &mut Frame, area: Rect, app: &App, words: &[String], hits: &mut HitMap) {
    let t = &app.theme;
    let counts: Vec<usize> = ModView::ALL
        .iter()
        .map(|&v| {
            app.modules
                .iter()
                .filter(|m| app.in_view(m, v) && m.score(words).is_some())
                .count()
        })
        .collect();
    let labels = |short: bool| -> Vec<String> {
        ModView::ALL
            .iter()
            .zip(&counts)
            .map(|(&v, n)| {
                let name = if short { v.short() } else { v.label() };
                match v {
                    ModView::Changed => format!(" ⟳ {name} {n} "),
                    _ => format!(" {name} {n} "),
                }
            })
            .collect()
    };
    // Si no caben todos con su nombre, con el nombre corto.
    let total = |ls: &[String]| -> u16 {
        ls.iter()
            .map(|l| Line::raw(l.as_str()).width() as u16 + 1)
            .sum()
    };
    let mut shown = labels(false);
    if total(&shown) > area.width {
        shown = labels(true);
    }
    let mut x = area.x;
    for ((v, label), n) in ModView::ALL.into_iter().zip(shown).zip(counts) {
        let w = Line::raw(label.as_str()).width() as u16;
        if x + w > area.right() {
            break;
        }
        let on = app.mod_view == v;
        let r = Rect::new(x, area.y, w, 1);
        let style = if on {
            Style::new().fg(t.bg).bg(t.accent).bold()
        } else if v == ModView::Changed && n > 0 {
            Style::new().fg(t.warn).bg(t.surface)
        } else if app.hovered(r) {
            Style::new().fg(t.fg).bg(t.sel)
        } else {
            Style::new().fg(t.dim).bg(t.surface)
        };
        f.render_widget(Line::styled(label, style), r);
        hits.push(r, Action::ModView(v));
        x += w + 1;
    }
}

/// Nombre técnico y, si cabe, el visible en tenue; lo buscado resaltado.
fn name_cell(
    m: &ModuleInfo,
    width: usize,
    words: &[String],
    style: Style,
    app: &App,
) -> Vec<Span<'static>> {
    let t = &app.theme;
    let hl = Style::new().fg(t.bg).bg(t.accent).bold();
    // Una columna de aire antes del estado.
    let width = width.saturating_sub(1);
    let name = widgets::truncate(&m.name, width);
    let mut used = Line::raw(name.as_str()).width();
    let mut spans = widgets::highlight(&name, words, style, hl);
    let rest = width.saturating_sub(used + 2);
    if rest >= 4 && !m.title.is_empty() && m.title != m.name {
        let title = widgets::truncate(&m.title, rest);
        used += 2 + Line::raw(title.as_str()).width();
        spans.push(Span::raw("  "));
        spans.extend(widgets::highlight(
            &title,
            words,
            Style::new().fg(t.dim),
            hl,
        ));
    }
    spans.push(Span::raw(" ".repeat(width.saturating_sub(used) + 1)));
    spans
}

pub fn draw(f: &mut Frame, area: Rect, app: &App, hits: &mut HitMap) {
    let t = &app.theme;
    let [bar, main] = Layout::vertical([Constraint::Length(2), Constraint::Fill(1)]).areas(area);
    let bar = Rect { height: 1, ..bar };

    let idle = app.busy.is_none();
    let running = app.busy.as_ref().is_some_and(|b| b.sink == Sink::Modules);
    let any = !app.marks.is_empty() || app.selected_module().is_some();
    let changed = app.modules.iter().filter(|m| m.changed()).count();
    let changed_label = format!("Cambiados ({changed})");
    widgets::toolbar(
        f,
        bar,
        app,
        &[
            Btn::new("i", "Instalar", Action::Cmd(Cmd::ModInstall)).enabled(idle && any),
            Btn::new("u", "Actualizar", Action::Cmd(Cmd::ModUpdate)).enabled(idle && any),
            Btn::new("U", &changed_label, Action::Cmd(Cmd::ModUpdateChanged))
                .enabled(idle && changed > 0),
            Btn::new("d", "Desinstalar", Action::Cmd(Cmd::ModUninstall))
                .enabled(idle && any)
                .danger(),
            Btn::new("␣", "Marcar", Action::Cmd(Cmd::ModToggle)),
            Btn::new("x", "Desmarcar", Action::Cmd(Cmd::ModClear)).enabled(!app.marks.is_empty()),
            Btn::new("r", "Recargar", Action::Cmd(Cmd::Refresh)),
            Btn::new("X", "Cancelar", Action::Cmd(Cmd::Cancel))
                .enabled(running)
                .danger(),
        ],
        hits,
    );

    let [left, right] =
        Layout::horizontal([Constraint::Percentage(50), Constraint::Fill(1)]).areas(main);
    let rows = app.module_rows();
    let badge = if app.mods_loading {
        super::spinner(app).to_string()
    } else {
        rows.len().to_string()
    };
    let inner = widgets::frame(
        f,
        left,
        app,
        Pane::ModList,
        app.glyphs.modules,
        "Módulos",
        Some(badge),
        hits,
    );
    let [dbrow, inner] =
        Layout::vertical([Constraint::Length(2), Constraint::Fill(1)]).areas(inner);
    db_selector(f, Rect { height: 1, ..dbrow }, app, hits);
    let words = modules::search_words(&app.mod_list.filter);
    let [search, chips_row, inner] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(2),
        Constraint::Fill(1),
    ])
    .areas(inner);
    search_bar(f, search, app, rows.len(), hits);
    chips(
        f,
        Rect {
            height: 1,
            ..chips_row
        },
        app,
        &words,
        hits,
    );
    if let Some(e) = &app.mods_err {
        f.render_widget(
            Line::styled(
                widgets::truncate(&format!("✗ {e}"), inner.width as usize),
                t.err,
            ),
            inner,
        );
    } else {
        let name_w = (inner.width as usize).saturating_sub(32).clamp(10, 70);
        let [head, inner] =
            Layout::vertical([Constraint::Length(1), Constraint::Fill(1)]).areas(inner);
        widgets::header(
            f,
            head,
            app,
            format!(
                "        {}{}{}",
                cell("MÓDULO", name_w),
                cell("ESTADO", 14),
                "VERSIÓN"
            ),
        );
        let lines: Vec<Line> = rows
            .iter()
            .map(|&i| {
                let m = &app.modules[i];
                let on = app.marks.contains(&m.name);
                let mark = if m.changed() {
                    Span::styled("⟳ ", Style::new().fg(t.warn).bold())
                } else if m.disk.is_some() {
                    // Addon propio (está en addons_dir).
                    Span::styled("◆ ", Style::new().fg(t.accent))
                } else {
                    Span::raw("  ")
                };
                let (state, version) = if m.changed() {
                    (
                        "cambió".to_string(),
                        // La de la BD ya no importa: se ve la que se instalará.
                        format!("→ {}", m.disk.as_deref().unwrap_or("")),
                    )
                } else {
                    (m.state.clone(), m.version.clone())
                };
                let state_color = match m.state.as_str() {
                    _ if m.changed() => t.warn,
                    "installed" => t.ok,
                    "to upgrade" | "to install" | "to remove" => t.warn,
                    "uninstallable" => t.err,
                    _ => t.dim,
                };
                let name_style = if m.application {
                    Style::new().fg(t.fg).bold()
                } else {
                    Style::new().fg(t.fg)
                };
                let mut spans = vec![
                    Span::styled(
                        if on { " [✓] " } else { " [ ] " },
                        if on {
                            Style::new().fg(t.ok).bold()
                        } else {
                            Style::new().fg(t.dim)
                        },
                    ),
                    mark,
                ];
                spans.extend(name_cell(m, name_w, &words, name_style, app));
                spans.push(Span::styled(cell(&state, 14), state_color));
                spans.push(Span::styled(version, t.dim));
                Line::from(spans)
            })
            .collect();
        // La casilla es clicable aparte de la fila.
        let h = inner.height as usize;
        let offset = app.mod_list.offset.min(rows.len().saturating_sub(h));
        widgets::list(f, inner, app, Pane::ModList, lines, &app.mod_list, hits);
        for k in 0..rows.len().saturating_sub(offset).min(h) {
            hits.push(
                Rect::new(inner.x, inner.y + k as u16, 6, 1),
                Action::Toggle(Pane::ModList, offset + k),
            );
        }
        if rows.is_empty() && !app.mods_loading {
            let msg = if app.modules.is_empty() {
                " sin módulos (elige una BD de Odoo)".to_string()
            } else if app.mod_list.filter.is_empty() {
                format!(" ninguno en «{}»", app.mod_view.label())
            } else {
                format!(
                    " nada coincide con «{}» en «{}»",
                    app.mod_list.filter,
                    app.mod_view.label()
                )
            };
            f.render_widget(Line::styled(msg, t.dim), inner);
        }
    }

    let title = if running {
        app.busy.as_ref().map_or("", |b| b.label.as_str())
    } else {
        "Salida de odoo"
    };
    let badge = running.then(|| super::spinner(app).to_string());
    let inner = widgets::frame(
        f,
        right,
        app,
        Pane::ModLog,
        app.glyphs.log,
        title,
        badge,
        hits,
    );
    let inner = widgets::filter_bar(f, inner, app, Pane::ModLog, hits);
    widgets::log_view(
        f,
        inner,
        app,
        Pane::ModLog,
        &app.mod_log,
        " marca módulos y pulsa i (instalar) o u (actualizar)",
        hits,
    );
}

/// `BD  ◄ nombre ►   2 marcados`: las flechas y el nombre cambian de BD.
fn db_selector(f: &mut Frame, row: Rect, app: &App, hits: &mut HitMap) {
    let t = &app.theme;
    let name = app.mod_db.as_deref().unwrap_or("sin BDs de Odoo");
    let label = Span::styled(" BD ", Style::new().fg(t.dim).bold());
    let lw = label.width() as u16;
    f.render_widget(label, row);
    let x = row.x + lw;
    let arrow = |s: &'static str, r: Rect| {
        let bg = if app.hovered(r) { t.sel } else { t.surface };
        widgets::pill(
            vec![Span::styled(s, Style::new().fg(t.accent).bold())],
            bg,
            None,
            app,
        )
    };
    let prev = Rect::new(x, row.y, 3, 1);
    f.render_widget(Line::from(arrow("◄", prev)), prev);
    hits.push(prev, Action::Cmd(Cmd::ModDb(-1)));
    let name_spans = widgets::pill(
        vec![Span::styled(
            format!("{} {name}", app.glyphs.db),
            Style::new().fg(t.bg).bold(),
        )],
        t.accent,
        None,
        app,
    );
    let nw = widgets::spans_width(&name_spans).min(row.width.saturating_sub(lw + 8));
    let mid = Rect::new(prev.right() + 1, row.y, nw, 1);
    f.render_widget(Line::from(name_spans), mid);
    hits.push(mid, Action::Cmd(Cmd::ModDb(1)));
    let next = Rect::new(mid.right() + 1, row.y, 3, 1);
    f.render_widget(Line::from(arrow("►", next)), next);
    hits.push(next, Action::Cmd(Cmd::ModDb(1)));
    let marks = app.marks.len();
    let changed = app.modules.iter().filter(|m| m.changed()).count();
    let info = if marks > 0 {
        Span::styled(
            format!("   {marks} marcado(s)"),
            Style::new().fg(t.ok).bold(),
        )
    } else if changed > 0 {
        Span::styled(
            format!("   ⟳ {changed} con código nuevo · U actualiza"),
            Style::new().fg(t.warn),
        )
    } else {
        Span::styled("   b / B cambia de BD", Style::new().fg(t.dim))
    };
    f.render_widget(
        info,
        Rect::new(
            next.right(),
            row.y,
            row.right().saturating_sub(next.right()),
            1,
        ),
    );
}
