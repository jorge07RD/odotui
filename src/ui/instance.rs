//! Pestaña Instancia: estado, controles y log en vivo del servidor de Odoo.

use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::Style,
    text::{Line, Span},
};

use super::widgets::{self, Btn};
use crate::{
    app::App,
    input::{Action, Cmd, HitMap, Pane},
};

pub fn draw(f: &mut Frame, area: Rect, app: &App, hits: &mut HitMap) {
    let t = &app.theme;
    let [bar, status, log_area] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Length(2),
        Constraint::Fill(1),
    ])
    .areas(area);
    let bar = Rect { height: 1, ..bar };
    let status = Rect {
        x: status.x + 1,
        width: status.width.saturating_sub(1),
        height: 1,
        ..status
    };

    let Some(p) = app.current() else {
        f.render_widget(Line::styled(" sin perfil", t.dim), status);
        return;
    };
    let running = app.inst_status == "running";
    let (dot, color) = if running {
        ("●", t.ok)
    } else if app.inst_status.starts_with("error") {
        ("●", t.err)
    } else {
        ("○", t.border)
    };
    let (icon, what) = if p.is_docker() {
        (
            app.glyphs.docker,
            if p.odoo_container.is_empty() {
                "—"
            } else {
                &p.odoo_container
            },
        )
    } else {
        (
            app.glyphs.local,
            if p.odoo_bin.is_empty() {
                "—"
            } else {
                &p.odoo_bin
            },
        )
    };
    // Estado como pastilla de color; luego la URL y, al final, lo largo.
    let fg_on = if running || color == t.err {
        t.bg
    } else {
        t.fg
    };
    let mut spans = widgets::pill(
        vec![Span::styled(
            format!("{dot} {}", app.inst_status),
            Style::new().fg(fg_on).bold(),
        )],
        color,
        None,
        app,
    );
    spans.push(Span::raw(" "));
    spans.extend(widgets::pill(
        vec![Span::styled(p.url().to_string(), Style::new().fg(t.accent))],
        t.surface,
        None,
        app,
    ));
    if !p.is_docker() && app.dev {
        spans.push(Span::raw(" "));
        spans.extend(widgets::pill(
            vec![Span::styled("--dev=all", Style::new().fg(t.warn))],
            t.surface,
            None,
            app,
        ));
    }
    spans.push(Span::styled(
        format!("  {icon} {what}"),
        Style::new().fg(t.dim),
    ));
    f.render_widget(Line::from(spans), status);

    let idle = app.busy.is_none();
    let local = !p.is_docker();
    widgets::toolbar(
        f,
        bar,
        app,
        &[
            Btn::new("s", "Iniciar", Action::Cmd(Cmd::InstStart)).enabled(idle && !running),
            Btn::new("t", "Parar", Action::Cmd(Cmd::InstStop))
                .enabled(idle && running)
                .danger(),
            Btn::new("R", "Reiniciar", Action::Cmd(Cmd::InstRestart)).enabled(idle),
            Btn::new("L", "Logs on/off", Action::Cmd(Cmd::InstLogs)),
            Btn::new("v", "--dev", Action::Cmd(Cmd::InstDev)).enabled(local),
            Btn::new("o", "Abrir", Action::Cmd(Cmd::InstOpen)),
            Btn::new("f", "Seguir", Action::Cmd(Cmd::Follow)),
            Btn::new("/", "Filtrar", Action::Filter(Pane::InstanceLog)),
            Btn::new("C", "Limpiar", Action::Cmd(Cmd::ClearLog)),
        ],
        hits,
    );

    let badge = if app.inst_log.is_empty() {
        None
    } else {
        Some(app.inst_log.visible().len().to_string())
    };
    let inner = widgets::frame(
        f,
        log_area,
        app,
        Pane::InstanceLog,
        app.glyphs.log,
        "Log de Odoo",
        badge,
        hits,
    );
    let inner = widgets::filter_bar(f, inner, app, Pane::InstanceLog, hits);
    let empty = if local {
        " pulsa s para lanzar odoo-bin en segundo plano; su log aparece aquí"
    } else {
        " sin logs (¿contenedor detenido? L para reconectar)"
    };
    widgets::log_view(f, inner, app, Pane::InstanceLog, &app.inst_log, empty, hits);
}
