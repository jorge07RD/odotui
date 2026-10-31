//! Pestaña Perfiles: lista, detalle y prueba de conexión.

use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::Style,
    text::{Line, Span},
    widgets::Paragraph,
};

use super::widgets::{self, Btn};
use crate::{
    app::App,
    config::Profile,
    input::{Action, Cmd, HitMap, Pane},
};

pub fn draw(f: &mut Frame, area: Rect, app: &App, hits: &mut HitMap) {
    let t = &app.theme;
    let [bar, main] = Layout::vertical([Constraint::Length(2), Constraint::Fill(1)]).areas(area);
    let bar = Rect { height: 1, ..bar };
    let has = !app.cfg.profiles.is_empty();
    let idle = app.busy.is_none();
    widgets::toolbar(
        f,
        bar,
        app,
        &[
            Btn::new("a", "Nuevo", Action::Cmd(Cmd::ProfNew)),
            Btn::new("e", "Editar", Action::Cmd(Cmd::ProfEdit)).enabled(has),
            Btn::new("⏎", "Usar", Action::Cmd(Cmd::ProfUse)).enabled(has),
            Btn::new("t", "Probar", Action::Cmd(Cmd::ProfTest)).enabled(has && idle),
            Btn::new("*", "Por defecto", Action::Cmd(Cmd::ProfDefault)).enabled(has),
            Btn::new("d", "Borrar", Action::Cmd(Cmd::ProfDelete))
                .enabled(has)
                .danger(),
        ],
        hits,
    );

    let [left, right] =
        Layout::horizontal([Constraint::Length(34), Constraint::Fill(1)]).areas(main);
    let inner = widgets::frame(
        f,
        left,
        app,
        Pane::Profiles,
        app.glyphs.profiles,
        "Perfiles",
        Some(app.cfg.profiles.len().to_string()),
        hits,
    );
    let default = app.cfg.default_profile.as_deref();
    let lines: Vec<Line> = app
        .cfg
        .profiles
        .iter()
        .enumerate()
        .map(|(i, p)| {
            Line::from(vec![
                Span::styled(
                    if i == app.profile { "● " } else { "  " },
                    Style::new().fg(t.ok),
                ),
                Span::styled(
                    format!(
                        "{} ",
                        if p.is_docker() {
                            app.glyphs.docker
                        } else {
                            app.glyphs.local
                        }
                    ),
                    Style::new().fg(t.accent),
                ),
                Span::styled(p.name.clone(), Style::new().fg(t.fg).bold()),
                Span::styled(format!(" {}", p.mode.as_str()), t.dim),
                Span::styled(
                    if default == Some(p.name.as_str()) {
                        " ★"
                    } else {
                        ""
                    },
                    t.warn,
                ),
            ])
        })
        .collect();
    if lines.is_empty() {
        f.render_widget(Line::styled(" pulsa a para crear uno", t.dim), inner);
    }
    widgets::list(f, inner, app, Pane::Profiles, lines, &app.prof_list, hits);

    let inner = widgets::card(f, right, app, app.glyphs.info, "Detalle");
    let Some(p) = app.cfg.profiles.get(app.prof_list.sel) else {
        return;
    };
    let test_h = if app.prof_log.is_empty() { 0 } else { 8 };
    let [detail, test] =
        Layout::vertical([Constraint::Fill(1), Constraint::Length(test_h)]).areas(inner);
    f.render_widget(Paragraph::new(detail_lines(app, p, detail.width)), detail);
    if test_h > 0 {
        let lines: Vec<Line> = app
            .prof_log
            .visible()
            .into_iter()
            .map(|l| {
                Line::styled(
                    l.to_string(),
                    widgets::level_style(app, crate::log::level(l)),
                )
            })
            .collect();
        let skip = lines.len().saturating_sub(test.height as usize);
        f.render_widget(Paragraph::new(lines[skip..].to_vec()), test);
    }
}

fn detail_lines<'a>(app: &App, p: &'a Profile, width: u16) -> Vec<Line<'a>> {
    let t = &app.theme;
    let room = (width as usize).saturating_sub(20);
    let row = |k: &'static str, v: String| {
        // Las rutas largas se recortan por el principio: lo útil es el final.
        let v = if v.is_empty() {
            "—".to_string()
        } else if v.chars().count() > room {
            let tail: String = v.chars().skip(v.chars().count() + 1 - room).collect();
            format!("…{tail}")
        } else {
            v
        };
        Line::from(vec![
            Span::styled(format!(" {k:<18}"), t.dim),
            Span::styled(v, t.fg),
        ])
    };
    let mut out = vec![
        Line::raw(""),
        row("modo", p.mode.as_str().into()),
        row(
            "postgres",
            format!(
                "{}@{}:{}",
                p.pg_user,
                if p.pg_host.is_empty() {
                    "socket"
                } else {
                    &p.pg_host
                },
                p.pg_port
            ),
        ),
        row(
            "contraseña",
            if p.pg_password.is_empty() {
                String::new()
            } else {
                "••••".into()
            },
        ),
    ];
    if p.is_docker() {
        out.push(row("contenedor postgres", p.pg_container.clone()));
        out.push(row("contenedor odoo", p.odoo_container.clone()));
    } else {
        out.push(row("odoo-bin", p.odoo_bin.clone()));
        out.push(row("python", p.python.clone()));
        out.push(row("odoo.conf", p.odoo_conf.clone()));
    }
    out.push(row("url", p.url().into()));
    out.push(row("addons propios", p.addons_dir.clone()));
    out.push(row("backups", p.backups_dir.clone()));
    out.push(row("filestore", p.filestore()));
    out
}
