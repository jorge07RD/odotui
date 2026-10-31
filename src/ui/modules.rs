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
    input::{Action, Cmd, HitMap, Pane},
    jobs::Sink,
};

pub fn draw(f: &mut Frame, area: Rect, app: &App, hits: &mut HitMap) {
    let t = &app.theme;
    let [bar, main] = Layout::vertical([Constraint::Length(2), Constraint::Fill(1)]).areas(area);
    let bar = Rect { height: 1, ..bar };

    let idle = app.busy.is_none();
    let running = app.busy.as_ref().is_some_and(|b| b.sink == Sink::Modules);
    let any = !app.marks.is_empty() || app.selected_module().is_some();
    widgets::toolbar(
        f,
        bar,
        app,
        &[
            Btn::new("i", "Instalar", Action::Cmd(Cmd::ModInstall)).enabled(idle && any),
            Btn::new("u", "Actualizar", Action::Cmd(Cmd::ModUpdate)).enabled(idle && any),
            Btn::new("␣", "Marcar", Action::Cmd(Cmd::ModToggle)),
            Btn::new("x", "Desmarcar", Action::Cmd(Cmd::ModClear)).enabled(!app.marks.is_empty()),
            Btn::new("r", "Recargar", Action::Cmd(Cmd::Refresh)),
            Btn::new("/", "Filtrar", Action::Filter(Pane::ModList)),
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
    let inner = widgets::filter_bar(f, inner, app, Pane::ModList, hits);
    if let Some(e) = &app.mods_err {
        f.render_widget(
            Line::styled(
                widgets::truncate(&format!("✗ {e}"), inner.width as usize),
                t.err,
            ),
            inner,
        );
    } else {
        let name_w = (inner.width as usize).saturating_sub(30).clamp(10, 50);
        let [head, inner] =
            Layout::vertical([Constraint::Length(1), Constraint::Fill(1)]).areas(inner);
        widgets::header(
            f,
            head,
            app,
            format!(
                "      {}{}{}",
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
                let state_color = match m.state.as_str() {
                    "installed" => t.ok,
                    "to upgrade" | "to install" | "to remove" => t.warn,
                    "uninstallable" => t.err,
                    _ => t.dim,
                };
                Line::from(vec![
                    Span::styled(
                        if on { " [✓] " } else { " [ ] " },
                        if on {
                            Style::new().fg(t.ok).bold()
                        } else {
                            Style::new().fg(t.dim)
                        },
                    ),
                    Span::styled(
                        cell(&m.name, name_w),
                        if m.application {
                            Style::new().fg(t.fg).bold()
                        } else {
                            Style::new().fg(t.fg)
                        },
                    ),
                    Span::styled(cell(&m.state, 14), state_color),
                    Span::styled(m.version.clone(), t.dim),
                ])
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
            f.render_widget(
                Line::styled(" sin módulos (elige una BD de Odoo)", t.dim),
                inner,
            );
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
    let info = if marks > 0 {
        Span::styled(
            format!("   {marks} marcado(s)"),
            Style::new().fg(t.ok).bold(),
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
