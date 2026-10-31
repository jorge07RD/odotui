//! Pestaña BDs: lista de bases con tamaño y versión de Odoo, y acciones.

use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::Style,
    text::{Line, Span},
};

use super::widgets::{self, Btn, cell, rcell};
use crate::{
    app::App,
    input::{Action, Cmd, HitMap, Pane},
    odoo::restore::human,
};

pub fn draw(f: &mut Frame, area: Rect, app: &App, hits: &mut HitMap) {
    let t = &app.theme;
    let log_h = if app.db_log.is_empty() { 0 } else { 9 };
    let [bar, list_area, log_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(log_h),
    ])
    .areas(area);

    let idle = app.busy.is_none();
    let has = app.selected_db().is_some();
    widgets::toolbar(
        f,
        bar,
        app,
        &[
            Btn::new("r", "Refrescar", Action::Cmd(Cmd::Refresh)),
            Btn::new("m", "Módulos", Action::Cmd(Cmd::DbModules)).enabled(has),
            Btn::new("s", "Restaurar aquí", Action::Cmd(Cmd::DbRestoreHere)).enabled(has && idle),
            Btn::new("c", "Duplicar", Action::Cmd(Cmd::DbDuplicate)).enabled(has && idle),
            Btn::new("n", "Renombrar", Action::Cmd(Cmd::DbRename)).enabled(has && idle),
            Btn::new("x", "Neutralizar", Action::Cmd(Cmd::DbNeutralize)).enabled(has && idle),
            Btn::new("d", "Borrar", Action::Cmd(Cmd::DbDrop))
                .enabled(has && idle)
                .danger(),
            Btn::new("X", "Cancelar", Action::Cmd(Cmd::Cancel)).enabled(!idle),
        ],
        hits,
    );

    let rows = app.db_rows();
    let title = if app.dbs_loading {
        format!(" {} Bases de datos ", super::spinner(app))
    } else {
        format!(" Bases de datos ({}) ", rows.len())
    };
    let inner = widgets::frame(f, list_area, app, Pane::DbList, Line::raw(title), hits);
    let inner = widgets::filter_bar(f, inner, app, Pane::DbList, hits);
    let [head, body] =
        Layout::vertical([Constraint::Length(1), Constraint::Fill(1)]).areas(inner);
    let name_w = (inner.width as usize).saturating_sub(40).clamp(12, 48);
    f.render_widget(
        Line::styled(
            format!(
                " {}  {} {} {}",
                cell("nombre", name_w),
                cell("odoo", 12),
                rcell("tamaño", 10),
                cell(" dueño", 12)
            ),
            Style::new().fg(t.dim).bold(),
        ),
        head,
    );

    if let Some(e) = &app.dbs_err {
        f.render_widget(Line::styled(widgets::truncate(&format!("✗ {e}"), body.width as usize), t.err), body);
        return log(f, log_area, app, hits);
    }
    if rows.is_empty() && !app.dbs_loading {
        let msg = if app.current().is_none() {
            "sin perfil: créalo en la pestaña 5"
        } else {
            "no hay bases de datos"
        };
        f.render_widget(Line::styled(format!(" {msg}"), t.dim), body);
    }
    let lines: Vec<Line> = rows
        .iter()
        .map(|&i| {
            let d = &app.dbs[i];
            let (ver, vstyle) = match &d.version {
                Some(v) => (v.as_str(), Style::new().fg(t.ok)),
                None => ("—", Style::new().fg(t.dim)),
            };
            Line::from(vec![
                Span::styled(cell(&d.name, name_w), Style::new().fg(t.fg).bold()),
                Span::raw("  "),
                Span::styled(cell(ver, 12), vstyle),
                Span::styled(rcell(&human(d.size), 10), t.fg),
                Span::styled(format!("  {}", cell(&d.owner, 12)), t.dim),
            ])
        })
        .collect();
    widgets::list(f, body, app, Pane::DbList, lines, &app.db_list, hits);
    log(f, log_area, app, hits);
}

fn log(f: &mut Frame, area: Rect, app: &App, hits: &mut HitMap) {
    if area.height == 0 {
        return;
    }
    let inner = widgets::frame(f, area, app, Pane::DbLog, Line::raw(" Operaciones "), hits);
    let inner = widgets::filter_bar(f, inner, app, Pane::DbLog, hits);
    widgets::log_view(f, inner, app, Pane::DbLog, &app.db_log, "", hits);
}
