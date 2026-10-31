//! Pestaña Restaurar: backups a la izquierda; progreso, log e historial a la derecha.

use chrono::{DateTime, Local};
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
    odoo::restore::{self, human},
};

pub fn draw(f: &mut Frame, area: Rect, app: &App, hits: &mut HitMap) {
    let t = &app.theme;
    let [bar, main] = Layout::vertical([Constraint::Length(2), Constraint::Fill(1)]).areas(area);
    let bar = Rect { height: 1, ..bar };
    let restoring = app.busy.as_ref().is_some_and(|b| b.sink == Sink::Restore);
    let can = app.busy.is_none() && app.selected_backup().is_some_and(|b| b.dump.is_some());
    widgets::toolbar(
        f,
        bar,
        app,
        &[
            Btn::new("R", "Restaurar…", Action::Cmd(Cmd::Restore)).enabled(can),
            Btn::new("r", "Refrescar", Action::Cmd(Cmd::Refresh)),
            Btn::new("/", "Filtrar", Action::Filter(Pane::Backups)),
            Btn::new("X", "Cancelar", Action::Cmd(Cmd::Cancel))
                .enabled(restoring)
                .danger(),
            Btn::new("C", "Limpiar log", Action::Cmd(Cmd::ClearLog)),
        ],
        hits,
    );

    let [left, right] =
        Layout::horizontal([Constraint::Percentage(45), Constraint::Fill(1)]).areas(main);

    // Backups.
    let rows = app.backup_rows();
    let dir = app.current().map(|p| p.backups_dir.as_str()).unwrap_or("");
    let title = format!("Backups  {}", widgets::truncate(dir, 28));
    let inner = widgets::frame(
        f,
        left,
        app,
        Pane::Backups,
        app.glyphs.backup,
        &title,
        Some(rows.len().to_string()),
        hits,
    );
    let inner = widgets::filter_bar(f, inner, app, Pane::Backups, hits);
    let target_h = u16::from(app.restore_target.is_some());
    let [target, head, body] = Layout::vertical([
        Constraint::Length(target_h),
        Constraint::Length(1),
        Constraint::Fill(1),
    ])
    .areas(inner);
    if let Some(db) = &app.restore_target {
        f.render_widget(
            Line::from(vec![
                Span::styled(" destino: ", t.dim),
                Span::styled(db.as_str(), Style::new().fg(t.warn).bold()),
                Span::styled(" (se sobrescribe)", t.dim),
            ]),
            target,
        );
    }
    let name_w = (inner.width as usize).saturating_sub(45).clamp(10, 60);
    widgets::header(
        f,
        head,
        app,
        format!(
            "    {} {}{} {}   FS  FECHA",
            cell("BACKUP", name_w),
            cell("TIPO", 8),
            cell("ODOO", 6),
            rcell("TAMAÑO", 9),
        ),
    );
    if let Some(e) = &app.backups_err {
        f.render_widget(
            Paragraph::new(format!("✗ {e}"))
                .style(Style::new().fg(t.err))
                .wrap(ratatui::widgets::Wrap { trim: true }),
            body,
        );
    } else {
        if rows.is_empty() {
            f.render_widget(
                Line::styled(
                    " no hay backups (carpetas, .zip, .dump, .sql, .sql.gz)",
                    t.dim,
                ),
                body,
            );
        }
        let lines: Vec<Line> = rows
            .iter()
            .map(|&i| {
                let b = &app.backups[i];
                let (mark, ms) = match b.dump {
                    Some(_) => ("✓ ", t.ok),
                    None => ("✗ ", t.err),
                };
                let size = b.dump.map(human).unwrap_or_else(|| "sin dump".into());
                let version = b
                    .manifest
                    .as_ref()
                    .map(|m| restore::major(&m.version))
                    .unwrap_or_default();
                let date = b
                    .modified
                    .map(|m| DateTime::<Local>::from(m).format("%d/%m %H:%M").to_string())
                    .unwrap_or_default();
                Line::from(vec![
                    Span::styled(mark, ms),
                    Span::styled(cell(&b.name, name_w), Style::new().fg(t.fg).bold()),
                    Span::styled(format!(" {}", cell(b.kind.label(), 8)), t.dim),
                    Span::styled(cell(&version, 6), t.ok),
                    Span::styled(rcell(&size, 10), t.fg),
                    Span::styled(if b.filestore { "  ● " } else { "  · " }, t.accent),
                    Span::styled(date, t.dim),
                ])
            })
            .collect();
        widgets::list(f, body, app, Pane::Backups, lines, &app.backup_list, hits);
    }

    // Derecha: progreso + log + historial.
    let hist_h = if app.history.is_empty() { 0 } else { 7 };
    let progress = app
        .progress
        .filter(|(s, _)| *s == Sink::Restore)
        .map(|(_, p)| p);
    let gauge_h = u16::from(progress.is_some());
    let [gauge, log_area, hist] = Layout::vertical([
        Constraint::Length(gauge_h),
        Constraint::Fill(1),
        Constraint::Length(hist_h),
    ])
    .areas(right);
    if let Some(p) = progress {
        let label = if restoring {
            format!("{:.1}% cargando el dump", p * 100.0)
        } else {
            format!("{:.0}%", p * 100.0)
        };
        widgets::gauge(f, gauge, app, p, label);
    }
    let badge = restoring.then(|| super::spinner(app).to_string());
    let inner = widgets::frame(
        f,
        log_area,
        app,
        Pane::RestoreLog,
        app.glyphs.restore,
        "Restauración",
        badge,
        hits,
    );
    let inner = widgets::filter_bar(f, inner, app, Pane::RestoreLog, hits);
    widgets::log_view(
        f,
        inner,
        app,
        Pane::RestoreLog,
        &app.restore_log,
        " elige un backup y pulsa R o haz doble clic",
        hits,
    );

    if hist_h > 0 {
        let inner = widgets::card(f, hist, app, app.glyphs.history, "Historial");
        let lines: Vec<Line> = app
            .history
            .iter()
            .take(inner.height as usize)
            .map(|e| {
                Line::from(vec![
                    Span::styled(
                        if e.ok { "✓ " } else { "✗ " },
                        if e.ok { t.ok } else { t.err },
                    ),
                    Span::styled(e.at.format("%d/%m %H:%M  ").to_string(), t.dim),
                    Span::styled(format!("{} ", e.db), Style::new().fg(t.fg).bold()),
                    Span::styled(format!("← {} ", e.backup), t.dim),
                    Span::styled(format!("[{}]", e.profile), t.dim),
                ])
            })
            .collect();
        f.render_widget(Paragraph::new(lines), inner);
    }
}
