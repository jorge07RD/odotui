//! La interfaz dibujada en una terminal de prueba: lo que se ve en cada
//! posición debe llevar a la acción correcta al hacer clic.

use ratatui::{
    Terminal,
    backend::TestBackend,
    buffer::Buffer,
    crossterm::event::{Event, KeyModifiers, MouseButton, MouseEvent, MouseEventKind},
};

use super::draw;
use crate::{
    app::{App, Confirm, Pending},
    input::{Action, Cmd, HitMap, Pane, Tab},
};

fn render(app: &mut App) -> Buffer {
    let mut term = Terminal::new(TestBackend::new(140, 36)).unwrap();
    let mut hits = HitMap::default();
    term.draw(|f| draw(f, app, &mut hits)).unwrap();
    app.hits = hits;
    term.backend().buffer().clone()
}

/// Posición del primer `text` en la fila `y` (o en cualquiera si es `None`).
fn find(buf: &Buffer, text: &str, y: Option<u16>) -> Option<(u16, u16)> {
    let rows = match y {
        Some(y) => y..y + 1,
        None => 0..buf.area.height,
    };
    for y in rows {
        // Cada celda aporta su símbolo; se recuerda en qué x empieza cada byte.
        let mut line = String::new();
        let mut xs = vec![];
        for x in 0..buf.area.width {
            let s = buf[(x, y)].symbol();
            xs.extend(std::iter::repeat_n(x, s.len()));
            line.push_str(s);
        }
        if let Some(i) = line.find(text) {
            return Some((xs[i], y));
        }
    }
    None
}

fn hit(app: &App, buf: &Buffer, text: &str, y: Option<u16>) -> Option<Action> {
    let (x, y) = find(buf, text, y).unwrap_or_else(|| panic!("no se ve «{text}»"));
    app.hits.at(x, y)
}

fn click(app: &mut App, x: u16, y: u16) {
    let ev = |kind| {
        Event::Mouse(MouseEvent {
            kind,
            column: x,
            row: y,
            modifiers: KeyModifiers::NONE,
        })
    };
    app.on_event(ev(MouseEventKind::Down(MouseButton::Left)));
    app.on_event(ev(MouseEventKind::Up(MouseButton::Left)));
}

#[test]
fn toolbar_tabs_and_rows_are_clickable() {
    let mut app = App::for_tests();
    let buf = render(&mut app);
    assert_eq!(
        hit(&app, &buf, "Refrescar", None),
        Some(Action::Cmd(Cmd::Refresh))
    );
    // Las pestañas están en la cabecera (fila 0).
    assert_eq!(
        hit(&app, &buf, "Restaurar", Some(0)),
        Some(Action::Tab(Tab::Restore))
    );
    assert_eq!(
        hit(&app, &buf, "Módulos", Some(0)),
        Some(Action::Tab(Tab::Modules))
    );
    assert_eq!(
        hit(&app, &buf, "acme_staging", None),
        Some(Action::Row(Pane::DbList, 1))
    );
    assert_eq!(hit(&app, &buf, "q salir", None), Some(Action::Quit));
}

#[test]
fn clicking_a_row_selects_it() {
    let mut app = App::for_tests();
    let buf = render(&mut app);
    let (x, y) = find(&buf, "acme_staging", None).unwrap();
    click(&mut app, x, y);
    assert_eq!(app.db_list.sel, 1);
    assert_eq!(app.selected_db().unwrap().name, "acme_staging");
    // Un arrastre largo no cuenta como clic.
    let (x0, y0) = find(&buf, "acme_prod", None).unwrap();
    app.on_event(Event::Mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: x0,
        row: y0,
        modifiers: KeyModifiers::NONE,
    }));
    app.on_event(Event::Mouse(MouseEvent {
        kind: MouseEventKind::Up(MouseButton::Left),
        column: x0,
        row: y0 + 5,
        modifiers: KeyModifiers::NONE,
    }));
    assert_eq!(app.db_list.sel, 1);
}

#[test]
fn disabled_buttons_do_nothing() {
    let mut app = App::for_tests();
    let buf = render(&mut app);
    // Sin tarea en curso, «Cancelar» no es clicable: debajo está la barra.
    assert_ne!(
        hit(&app, &buf, "Cancelar", None),
        Some(Action::Cmd(Cmd::Cancel))
    );
}

#[test]
fn modules_have_a_separate_checkbox() {
    let mut app = App::for_tests();
    app.tab = Tab::Modules;
    app.focus = Pane::ModList;
    let buf = render(&mut app);
    let (x, y) = find(&buf, " sale ", None).unwrap();
    assert_eq!(app.hits.at(x + 1, y), Some(Action::Row(Pane::ModList, 2)));
    let (cx, cy) = find(&buf, "[ ]", Some(y)).unwrap();
    assert_eq!(app.hits.at(cx, cy), Some(Action::Toggle(Pane::ModList, 2)));
    click(&mut app, cx, cy);
    assert!(app.marks.contains("sale"));
}

#[test]
fn changed_addons_are_marked_and_updatable() {
    let mut app = App::for_tests();
    app.tab = Tab::Modules;
    let buf = render(&mut app);
    let (_, y) = find(&buf, "acme_ventas", None).unwrap();
    assert!(find(&buf, "cambió", Some(y)).is_some());
    assert!(find(&buf, "→ 17.0.1.1", Some(y)).is_some());
    assert_eq!(
        hit(&app, &buf, "Cambiados (1)", None),
        Some(Action::Cmd(Cmd::ModUpdateChanged))
    );
    // Visible aunque la barra se llene.
    assert!(find(&buf, "Cancelar", Some(1)).is_some());
}

#[test]
fn clicking_an_error_line_targets_its_block() {
    let mut app = App::for_tests();
    app.tab = Tab::Instance;
    app.focus = Pane::InstanceLog;
    for l in [
        "2025-01-01 10:00:00,000 1 INFO db odoo: inicio",
        "2025-01-01 10:00:01,000 1 ERROR db odoo.http: falló",
        "Traceback (most recent call last):",
        "  File \"/x.py\", line 3, in f",
        "ValueError: malo",
    ] {
        app.inst_log.push(l.into());
    }
    let buf = render(&mut app);
    assert_eq!(
        hit(&app, &buf, "ValueError: malo", None),
        Some(Action::LogLine(Pane::InstanceLog, 4))
    );
    // Una línea normal no copia nada: sigue siendo el panel.
    assert_eq!(
        hit(&app, &buf, "odoo: inicio", None),
        Some(Action::Focus(Pane::InstanceLog))
    );
    assert!(
        app.inst_log
            .block_at(4)
            .unwrap()
            .starts_with("2025-01-01 10:00:01")
    );
}

#[test]
fn a_confirm_blocks_what_is_below() {
    let mut app = App::for_tests();
    app.confirm = Some(Confirm {
        message: "¿Seguro?".into(),
        then: Pending::Quit,
    });
    let buf = render(&mut app);
    assert_eq!(hit(&app, &buf, "Refrescar", None), Some(Action::Close));
    assert_eq!(hit(&app, &buf, "Sí (s)", None), Some(Action::ConfirmYes));
    let (x, y) = find(&buf, "No (n)", None).unwrap();
    click(&mut app, x, y);
    assert!(app.confirm.is_none());
    assert!(!app.quit);
}

#[test]
fn connect_buttons_and_user_picker() {
    use crate::{
        app::{LoginPick, Palette},
        odoo::login::OdooUser,
    };
    let mut app = App::for_tests();
    let buf = render(&mut app);
    // El detalle de la BD de Odoo seleccionada trae «Conectar» y «Como…».
    assert_eq!(
        hit(&app, &buf, "Conectar", None),
        Some(Action::Cmd(Cmd::DbLogin))
    );
    assert_eq!(
        hit(&app, &buf, "Como…", None),
        Some(Action::Cmd(Cmd::DbLoginAs))
    );

    let user = |id, login: &str, name: &str, admin| OdooUser {
        id,
        login: login.into(),
        name: name.into(),
        portal: false,
        admin,
    };
    app.login_pick = Some(LoginPick {
        db: "dev".into(),
        users: vec![
            user(2, "admin", "Mitchell Admin", true),
            user(6, "demo", "Marc Demo", false),
        ],
    });
    app.palette = Some(Palette {
        users: true,
        query: "marc".into(),
        ..Palette::default()
    });
    let items = app.palette_items();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].1, Action::LoginAs(1));
    let buf = render(&mut app);
    assert!(find(&buf, "Conectar a dev como", None).is_some());
    assert_eq!(
        hit(&app, &buf, "Marc Demo", None),
        Some(Action::Row(Pane::Palette, 0))
    );
}

#[test]
fn run_one_database() {
    let mut app = App::for_tests();
    app.cfg.profiles[0].mode = crate::config::Mode::Local;
    let buf = render(&mut app);
    assert_eq!(
        hit(&app, &buf, "Correr solo esta", None),
        Some(Action::Cmd(Cmd::DbRunOnly))
    );
    // Corriendo sola: el detalle lo dice y el botón ya no hace nada.
    app.inst_status = "running".into();
    app.inst_db = Some("acme_prod".into());
    let buf = render(&mut app);
    assert!(find(&buf, "corre sola", None).is_some());
    assert_ne!(
        hit(&app, &buf, "Correr solo esta", None),
        Some(Action::Cmd(Cmd::DbRunOnly))
    );
    // En Instancia: la pastilla y «Todas» para volver.
    app.tab = Tab::Instance;
    let buf = render(&mut app);
    assert!(find(&buf, "solo acme_prod", None).is_some());
    assert_eq!(
        hit(&app, &buf, "Todas", None),
        Some(Action::Cmd(Cmd::InstAllDbs))
    );
}

#[test]
fn module_search_bar_and_chips() {
    use crate::input::ModView;
    use ratatui::crossterm::event::{KeyCode, KeyEvent};
    let mut app = App::for_tests();
    app.tab = Tab::Modules;
    app.focus = Pane::ModList;
    app.modules[2].title = "Ventas".into();
    let names = |app: &App| -> Vec<String> {
        app.module_rows()
            .iter()
            .map(|&i| app.modules[i].name.clone())
            .collect()
    };
    let key =
        |app: &mut App, code| app.on_event(Event::Key(KeyEvent::new(code, KeyModifiers::NONE)));

    // Siempre visible, con su ayuda; un clic empieza a escribir.
    let buf = render(&mut app);
    let (x, y) = find(&buf, "Buscar módulo", None).unwrap();
    assert_eq!(app.hits.at(x, y), Some(Action::Filter(Pane::ModList)));
    click(&mut app, x, y);
    assert_eq!(app.filtering, Some(Pane::ModList));

    // «ventas»: sale primero (su título es exactamente «Ventas»), luego
    // acme_ventas (lo lleva dentro del nombre).
    for c in "ventas".chars() {
        key(&mut app, KeyCode::Char(c));
    }
    assert_eq!(names(&app), ["sale", "acme_ventas"]);
    let buf = render(&mut app);
    assert!(find(&buf, "2 de 3", None).is_some());

    // ↓ elige la siguiente sin dejar de escribir.
    key(&mut app, KeyCode::Down);
    assert_eq!(app.filtering, Some(Pane::ModList));
    assert_eq!(app.selected_module().unwrap().name, "acme_ventas");

    // Chip «Cambiados»: solo acme_ventas (su código cambió).
    key(&mut app, KeyCode::Enter);
    let buf = render(&mut app);
    // Con nombre largo o corto («⟳ Camb.»), según el ancho.
    let (cx, cy) = find(&buf, "⟳ Camb", None).unwrap();
    assert_eq!(app.hits.at(cx, cy), Some(Action::ModView(ModView::Changed)));
    click(&mut app, cx, cy);
    assert_eq!(names(&app), ["acme_ventas"]);
}

#[test]
fn favorite_and_recent_databases_go_first() {
    let mut app = App::for_tests();
    let names = |app: &App| -> Vec<String> {
        app.db_rows()
            .iter()
            .map(|&i| app.dbs[i].name.clone())
            .collect()
    };
    assert_eq!(names(&app), ["acme_prod", "acme_staging", "postgres"]);

    // Clic en la estrella de acme_staging: favorita, sube y sigue seleccionada.
    let buf = render(&mut app);
    let (x, y) = find(&buf, "acme_staging", None).unwrap();
    assert_eq!(app.hits.at(x - 2, y), Some(Action::Toggle(Pane::DbList, 1)));
    click(&mut app, x - 2, y);
    assert!(app.db_marks.is_favorite("acme_staging"));
    assert_eq!(names(&app), ["acme_staging", "acme_prod", "postgres"]);
    assert_eq!(app.selected_db().unwrap().name, "acme_staging");

    // Usada hace poco: justo después de las favoritas, con su «cuándo».
    app.db_marks.touch("postgres", "psql");
    assert_eq!(names(&app), ["acme_staging", "postgres", "acme_prod"]);
    let buf = render(&mut app);
    assert!(find(&buf, "★ acme_staging", None).is_some());
    assert!(find(&buf, "ahora", None).is_some());
}

#[test]
fn running_odoo_is_visible_everywhere() {
    use crate::odoo::running::Running;
    let mut app = App::for_tests();
    let buf = render(&mut app);
    assert!(find(&buf, "○ Odoo", Some(0)).is_some(), "ninguno: apagado");

    let r = |pid, profile: Option<&str>, managed, port, db: Option<&str>| Running {
        pid: Some(pid),
        container: None,
        profile: profile.map(String::from),
        managed,
        port,
        db: db.map(String::from),
        dev: false,
    };
    app.running = vec![r(4321, Some("dev"), true, 8069, None)];
    let buf = render(&mut app);
    let (x, y) = find(&buf, "● Odoo :8069", Some(0)).unwrap();
    assert_eq!(app.hits.at(x + 2, y), Some(Action::Tab(Tab::Instance)));

    // Dos: el de este perfil y uno lanzado fuera, con su BD.
    app.running.push(r(999, None, false, 8070, Some("pro_x")));
    app.tab = Tab::Instance;
    let buf = render(&mut app);
    assert!(find(&buf, "● 2 Odoo", Some(0)).is_some());
    assert!(find(&buf, "2 Odoo corriendo", None).is_some());
    let (_, y1) = find(&buf, "pid 4321", None).unwrap();
    assert!(find(&buf, "este perfil", Some(y1)).is_some());
    let (_, y2) = find(&buf, "pid 999", None).unwrap();
    assert!(find(&buf, "solo pro_x", Some(y2)).is_some());
    assert!(find(&buf, "iniciado fuera de odotui", Some(y2)).is_some());
    let (ox, oy) = find(&buf, " abrir ", Some(y2)).unwrap();
    assert_eq!(app.hits.at(ox + 1, oy), Some(Action::OpenRunning(1)));
}
