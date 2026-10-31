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
    let [dbrow, bar, main] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Fill(1),
    ])
    .areas(area);

    // Selector de BD: ◄ nombre ►
    let name = app.mod_db.as_deref().unwrap_or("(sin BDs de Odoo)");
    let arrow = Style::new().fg(t.accent).bg(t.surface).bold();
    let prev = Rect::new(dbrow.x, dbrow.y, 3, 1);
    let label = format!(" BD: {name} ");
    let lw = Line::raw(label.as_str()).width() as u16;
    let mid = Rect::new(prev.right(), dbrow.y, lw.min(dbrow.width.saturating_sub(6)), 1);
    let next = Rect::new(mid.right(), dbrow.y, 3, 1);
    f.render_widget(Span::styled(" ◄ ", arrow), prev);
    f.render_widget(
        Span::styled(label, Style::new().fg(t.fg).bg(t.surface).bold()),
        mid,
    );
    f.render_widget(Span::styled(" ► ", arrow), next);
    hits.push(prev, Action::Cmd(Cmd::ModDb(-1)));
    hits.push(mid, Action::Cmd(Cmd::ModDb(1)));
    hits.push(next, Action::Cmd(Cmd::ModDb(1)));
    f.render_widget(
        Line::styled(
            format!("  b/B cambia · {} marcado(s) ", app.marks.len()),
            t.dim,
        ),
        Rect::new(next.right(), dbrow.y, dbrow.right().saturating_sub(next.right()), 1),
    );

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
    let title = if app.mods_loading {
        format!(" {} Módulos ", super::spinner(app))
    } else {
        format!(" Módulos ({}) ", rows.len())
    };
    let inner = widgets::frame(f, left, app, Pane::ModList, Line::raw(title), hits);
    let inner = widgets::filter_bar(f, inner, app, Pane::ModList, hits);
    if let Some(e) = &app.mods_err {
        f.render_widget(
            Line::styled(widgets::truncate(&format!("✗ {e}"), inner.width as usize), t.err),
            inner,
        );
    } else {
        let name_w = (inner.width as usize).saturating_sub(30).clamp(10, 50);
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
                    Span::styled(if on { "[✓] " } else { "[ ] " }, if on { t.ok } else { t.dim }),
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
                Rect::new(inner.x, inner.y + k as u16, 5, 1),
                Action::Toggle(Pane::ModList, offset + k),
            );
        }
        if rows.is_empty() && !app.mods_loading {
            f.render_widget(Line::styled(" sin módulos (elige una BD de Odoo)", t.dim), inner);
        }
    }

    let title = if running {
        format!(" {} {} ", super::spinner(app), app.busy.as_ref().map_or("", |b| b.label.as_str()))
    } else {
        " Salida de odoo ".into()
    };
    let inner = widgets::frame(f, right, app, Pane::ModLog, Line::raw(title), hits);
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
