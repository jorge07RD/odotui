//! Pestaña BDs: lista de bases con tamaño y versión de Odoo, detalle de la
//! seleccionada y acciones.

use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::Style,
    text::{Line, Span},
    widgets::Paragraph,
};

use super::widgets::{self, Btn, cell, rcell};
use crate::{
    app::App,
    input::{Action, Cmd, HitMap, Pane},
    jobs::Sink,
    odoo::restore::human,
};

pub fn draw(f: &mut Frame, area: Rect, app: &App, hits: &mut HitMap) {
    let t = &app.theme;
    let log_h = if app.db_log.is_empty() { 0 } else { 9 };
    // Un backup en curso: barra al comprimir, MB escritos mientras corre pg_dump.
    let backing_up = app.busy.as_ref().is_some_and(|b| b.sink == Sink::Dbs);
    let progress = app
        .progress
        .filter(|(s, _)| *s == Sink::Dbs && backing_up)
        .map(|(_, p)| p);
    let gauge_h = u16::from(progress.is_some() || backing_up && app.written.is_some());
    let [bar, main, gauge, log_area] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Fill(1),
        Constraint::Length(gauge_h),
        Constraint::Length(log_h),
    ])
    .areas(area);
    if let Some(p) = progress {
        widgets::gauge(f, gauge, app, p, format!("{:.1}% comprimiendo", p * 100.0));
    } else if let Some(n) = app.written.filter(|_| backing_up) {
        f.render_widget(
            Line::from(vec![
                Span::styled(format!(" {} ", super::spinner(app)), t.accent),
                Span::styled(
                    format!("{} escritos", human(n)),
                    Style::new().fg(t.fg).bold(),
                ),
                Span::styled("  (pg_dump no sabe el total)", t.dim),
            ]),
            gauge,
        );
    }

    let idle = app.busy.is_none();
    let has = app.selected_db().is_some();
    widgets::toolbar(
        f,
        Rect { height: 1, ..bar },
        app,
        &[
            Btn::new("r", "Refrescar", Action::Cmd(Cmd::Refresh)),
            Btn::new("m", "Módulos", Action::Cmd(Cmd::DbModules)).enabled(has),
            Btn::new("b", "Backup", Action::Cmd(Cmd::DbBackup)).enabled(has && idle),
            Btn::new("s", "Restaurar", Action::Cmd(Cmd::DbRestoreHere)).enabled(has && idle),
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

    // Con espacio, el detalle de la BD seleccionada a la derecha.
    let wide = main.width >= 110;
    let [list_area, detail_area] = Layout::horizontal([
        Constraint::Fill(1),
        Constraint::Length(if wide { 38 } else { 0 }),
    ])
    .areas(main);

    let rows = app.db_rows();
    let badge = if app.dbs_loading {
        super::spinner(app).to_string()
    } else {
        rows.len().to_string()
    };
    let inner = widgets::frame(
        f,
        list_area,
        app,
        Pane::DbList,
        app.glyphs.db,
        "Bases de datos",
        Some(badge),
        hits,
    );
    let inner = widgets::filter_bar(f, inner, app, Pane::DbList, hits);
    let [head, body] = Layout::vertical([Constraint::Length(1), Constraint::Fill(1)]).areas(inner);
    let name_w = (inner.width as usize).saturating_sub(42).clamp(12, 48);
    widgets::header(
        f,
        head,
        app,
        format!(
            "  {}  {} {}   {}",
            cell("NOMBRE", name_w),
            cell("ODOO", 12),
            rcell("TAMAÑO", 10),
            cell("DUEÑO", 12)
        ),
    );

    if let Some(e) = &app.dbs_err {
        f.render_widget(
            Paragraph::new(format!("✗ {e}"))
                .style(Style::new().fg(t.err))
                .wrap(ratatui::widgets::Wrap { trim: true }),
            body,
        );
        detail(f, detail_area, app);
        return log(f, log_area, app, hits);
    }
    if rows.is_empty() && !app.dbs_loading {
        let msg = if app.current().is_none() {
            "sin perfil: créalo en la pestaña 5"
        } else {
            "no hay bases de datos"
        };
        f.render_widget(
            Line::styled(format!("  {msg}"), Style::new().fg(t.dim).italic()),
            body,
        );
    }
    let lines: Vec<Line> = rows
        .iter()
        .map(|&i| {
            let d = &app.dbs[i];
            let (ver, vstyle) = match &d.version {
                Some(v) => (format!("● {v}"), Style::new().fg(t.ok)),
                None => ("○ —".to_string(), Style::new().fg(t.dim)),
            };
            Line::from(vec![
                Span::styled(
                    format!(" {}", cell(&d.name, name_w)),
                    Style::new().fg(t.fg).bold(),
                ),
                Span::raw("  "),
                Span::styled(cell(&ver, 12), vstyle),
                Span::styled(rcell(&human(d.size), 10), Style::new().fg(t.fg)),
                Span::styled(format!("   {}", cell(&d.owner, 12)), Style::new().fg(t.dim)),
            ])
        })
        .collect();
    widgets::list(f, body, app, Pane::DbList, lines, &app.db_list, hits);
    detail(f, detail_area, app);
    log(f, log_area, app, hits);
}

/// Tarjeta con la BD seleccionada y un resumen de todas.
fn detail(f: &mut Frame, area: Rect, app: &App) {
    if area.width == 0 {
        return;
    }
    let t = &app.theme;
    let inner = widgets::card(f, area, app, app.glyphs.info, "Detalle");
    let row = |k: &str, v: String| {
        Line::from(vec![
            Span::styled(format!(" {k:<10}"), Style::new().fg(t.dim)),
            Span::styled(v, Style::new().fg(t.fg)),
        ])
    };
    let mut lines = vec![Line::raw("")];
    match app.selected_db() {
        Some(d) => {
            lines.push(Line::styled(
                format!(" {}", widgets::truncate(&d.name, inner.width as usize - 2)),
                Style::new().fg(t.accent).bold(),
            ));
            lines.push(Line::raw(""));
            let badge = match &d.version {
                Some(v) => widgets::pill(
                    vec![Span::styled(
                        format!("Odoo {v}"),
                        Style::new().fg(t.bg).bold(),
                    )],
                    t.ok,
                    None,
                    app,
                ),
                None => widgets::pill(
                    vec![Span::styled("no es de Odoo", Style::new().fg(t.fg))],
                    t.border,
                    None,
                    app,
                ),
            };
            let mut b = vec![Span::raw(" ")];
            b.extend(badge);
            lines.push(Line::from(b));
            lines.push(Line::raw(""));
            lines.push(row("tamaño", human(d.size)));
            lines.push(row("dueño", d.owner.clone()));
            if let Some(p) = app.current() {
                lines.push(row(
                    "filestore",
                    widgets::truncate(
                        &format!("{}/{}", p.filestore().trim_end_matches('/'), d.name),
                        (inner.width as usize).saturating_sub(12),
                    ),
                ));
            }
        }
        None => lines.push(Line::styled(
            " nada seleccionado",
            Style::new().fg(t.dim).italic(),
        )),
    }
    // Resumen abajo.
    let total: u64 = app.dbs.iter().map(|d| d.size).sum();
    let odoo = app.dbs.iter().filter(|d| d.version.is_some()).count();
    let summary = vec![
        Line::styled(
            format!(" {}", "─".repeat(inner.width.saturating_sub(2) as usize)),
            Style::new().fg(t.border),
        ),
        row("bases", format!("{} ({odoo} de Odoo)", app.dbs.len())),
        row("en total", human(total)),
    ];
    let [top, bottom] = Layout::vertical([Constraint::Fill(1), Constraint::Length(3)]).areas(inner);
    f.render_widget(Paragraph::new(lines), top);
    f.render_widget(Paragraph::new(summary), bottom);
}

fn log(f: &mut Frame, area: Rect, app: &App, hits: &mut HitMap) {
    if area.height == 0 {
        return;
    }
    let inner = widgets::frame(
        f,
        area,
        app,
        Pane::DbLog,
        app.glyphs.log,
        "Operaciones",
        None,
        hits,
    );
    let inner = widgets::filter_bar(f, inner, app, Pane::DbLog, hits);
    widgets::log_view(f, inner, app, Pane::DbLog, &app.db_log, "", hits);
}
