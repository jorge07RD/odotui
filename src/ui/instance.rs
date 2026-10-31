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
    let [status, bar, log_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Fill(1),
    ])
    .areas(area);

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
        ("○", t.dim)
    };
    let what = if p.is_docker() {
        format!("contenedor {}", if p.odoo_container.is_empty() { "—" } else { &p.odoo_container })
    } else {
        format!("odoo-bin {}", if p.odoo_bin.is_empty() { "—" } else { &p.odoo_bin })
    };
    // Lo corto primero: la ruta de odoo-bin puede ser larga y se recorta.
    let mut spans = vec![
        Span::styled(format!(" {dot} {} ", app.inst_status), Style::new().fg(color).bold()),
        Span::styled(format!(" {} ", p.url()), t.accent),
    ];
    if !p.is_docker() && app.dev {
        spans.push(Span::styled(" --dev=all ", Style::new().fg(t.warn)));
    }
    spans.push(Span::styled(format!(" {what}"), t.dim));
    f.render_widget(Line::from(spans), status);

    let idle = app.busy.is_none();
    let local = !p.is_docker();
    let logs_label = if local { "Logs" } else { "Logs on/off" };
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
            Btn::new("L", logs_label, Action::Cmd(Cmd::InstLogs)).enabled(!local),
            Btn::new("v", "--dev", Action::Cmd(Cmd::InstDev)).enabled(local),
            Btn::new("o", "Abrir", Action::Cmd(Cmd::InstOpen)),
            Btn::new("f", "Seguir", Action::Cmd(Cmd::Follow)),
            Btn::new("/", "Filtrar", Action::Filter(Pane::InstanceLog)),
            Btn::new("C", "Limpiar", Action::Cmd(Cmd::ClearLog)),
        ],
        hits,
    );

    let inner = widgets::frame(f, log_area, app, Pane::InstanceLog, Line::raw(" Log "), hits);
    let inner = widgets::filter_bar(f, inner, app, Pane::InstanceLog, hits);
    let empty = if local {
        " pulsa s para lanzar odoo-bin; su salida aparece aquí"
    } else {
        " sin logs (¿contenedor detenido? L para reconectar)"
    };
    widgets::log_view(f, inner, app, Pane::InstanceLog, &app.inst_log, empty, hits);
}
