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

/// Todos los Odoo de la máquina: de cada perfil y los lanzados fuera.
fn running_list(f: &mut Frame, area: Rect, app: &App, hits: &mut HitMap) {
    let t = &app.theme;
    let title = match app.running.len() {
        0 => "Ningún Odoo corriendo".to_string(),
        1 => "1 Odoo corriendo".to_string(),
        n => format!("{n} Odoo corriendo"),
    };
    let inner = widgets::card(f, area, app, app.glyphs.server, &title);
    if app.running.is_empty() {
        f.render_widget(
            Line::styled(
                " en esta máquina no hay ningún odoo-bin ni contenedor de tus perfiles · s para iniciar",
                t.dim,
            ),
            inner,
        );
        return;
    }
    let current = app.current().map(|p| p.name.as_str());
    for (i, r) in app.running.iter().enumerate().take(inner.height as usize) {
        let row = Rect::new(inner.x, inner.y + i as u16, inner.width, 1);
        let mine = r.managed && r.profile.as_deref() == current;
        let dot = if r.managed {
            Span::styled(" ● ", Style::new().fg(t.ok).bold())
        } else {
            Span::styled(" ● ", Style::new().fg(t.warn).bold())
        };
        let who = Span::styled(
            widgets::cell(&r.who(), 22),
            if mine {
                Style::new().fg(t.accent).bold()
            } else {
                Style::new().fg(t.fg).bold()
            },
        );
        let pid = match (r.pid, &r.container) {
            (Some(p), _) => format!("pid {p}"),
            (None, Some(_)) => "docker".to_string(),
            _ => String::new(),
        };
        let dbs = match &r.db {
            Some(d) => format!("solo {d}"),
            None => "todas las BDs".into(),
        };
        let mut spans = vec![
            dot,
            who,
            Span::styled(widgets::cell(&pid, 12), t.dim),
            Span::styled(widgets::cell(&format!(":{}", r.port), 7), t.accent),
            Span::styled(
                widgets::cell(&dbs, 24),
                if r.db.is_some() {
                    Style::new().fg(t.warn)
                } else {
                    Style::new().fg(t.fg)
                },
            ),
        ];
        if r.dev {
            spans.push(Span::styled("--dev  ", t.dim));
        }
        if mine {
            spans.push(Span::styled("este perfil", t.dim));
        } else if !r.managed {
            spans.push(Span::styled("iniciado fuera de odotui", t.dim));
        }
        f.render_widget(Line::from(spans), row);
        // «abrir», a la derecha y clicable.
        let open = " abrir ";
        let w = open.len() as u16;
        if row.width > w + 2 {
            let r_open = Rect::new(row.right() - w - 1, row.y, w, 1);
            let style = if app.hovered(r_open) {
                Style::new().fg(t.bg).bg(t.accent).bold()
            } else {
                Style::new().fg(t.accent).bg(t.surface)
            };
            f.render_widget(Line::styled(open, style), r_open);
            hits.push(r_open, Action::OpenRunning(i));
        }
    }
}

pub fn draw(f: &mut Frame, area: Rect, app: &App, hits: &mut HitMap) {
    let t = &app.theme;
    let list_h = app.running.len().clamp(1, 6) as u16 + 2;
    let [bar, status, list_area, log_area] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Length(2),
        Constraint::Length(list_h),
        Constraint::Fill(1),
    ])
    .areas(area);
    running_list(f, list_area, app, hits);
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
    if let Some(db) = &app.inst_db {
        spans.push(Span::raw(" "));
        spans.extend(widgets::pill(
            vec![Span::styled(
                format!("solo {db}"),
                Style::new().fg(t.bg).bold(),
            )],
            t.warn,
            None,
            app,
        ));
    }
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
    let mut btns = vec![
        Btn::new("s", "Iniciar", Action::Cmd(Cmd::InstStart)).enabled(idle && !running),
        Btn::new("t", "Parar", Action::Cmd(Cmd::InstStop))
            .enabled(idle && running)
            .danger(),
        Btn::new("R", "Reiniciar", Action::Cmd(Cmd::InstRestart)).enabled(idle),
        // Como en producción: Odoo con una sola BD (elige cuál).
        Btn::new("i", "Una BD…", Action::Cmd(Cmd::InstRunOnly)).enabled(idle && local),
    ];
    // Solo cuando corre con una BD: volver a todas.
    if app.inst_db.is_some() {
        btns.push(Btn::new("a", "Todas", Action::Cmd(Cmd::InstAllDbs)).enabled(idle));
    }
    btns.extend([
        Btn::new("L", "Logs on/off", Action::Cmd(Cmd::InstLogs)),
        Btn::new("v", "--dev", Action::Cmd(Cmd::InstDev)).enabled(local),
        Btn::new("o", "Abrir", Action::Cmd(Cmd::InstOpen)),
        Btn::new("f", "Seguir", Action::Cmd(Cmd::Follow)),
        Btn::new("/", "Filtrar", Action::Filter(Pane::InstanceLog)),
        Btn::new("C", "Limpiar", Action::Cmd(Cmd::ClearLog)),
    ]);
    widgets::toolbar(f, bar, app, &btns, hits);

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
