//! Estado de la aplicación. Teclas, clics y la paleta se traducen a `Action`
//! y pasan todas por `App::apply` (patrón de tuisentry).

use std::{
    collections::BTreeSet,
    future::Future,
    path::PathBuf,
    time::{Instant, SystemTime},
};

use ratatui::{
    crossterm::event::{
        Event as TermEvent, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent,
        MouseEventKind,
    },
    layout::{Position, Rect},
};
use serde_json::Value;
use tokio::sync::{mpsc::UnboundedSender, oneshot};
use tokio_util::sync::CancellationToken;

use crate::{
    ai::{
        headless::{first_line, resolve},
        tools,
    },
    config::{Config, Profile},
    exec::{self, Spec, Stop},
    form::{self, Form, Target},
    history::{self, Entry},
    input::{Action, Cmd, HitMap, Pane, Tab},
    jobs::{AiRequest, Ev, Sink, Tx},
    log::{ListState, LogView},
    odoo::{
        filestore, instance, modules,
        modules::ModuleInfo,
        neutralize, pg,
        pg::DbInfo,
        restore::{self, Backup, Request},
    },
    theme::{self, Theme},
    ui::widgets::{Glyphs, NERD, PLAIN},
};

pub struct Confirm {
    pub message: String,
    pub then: Pending,
}

pub enum Pending {
    DropDb(String),
    Restore(Box<Request>),
    DeleteProfile(usize),
    RestartInstance,
    Quit,
}

/// El trabajo de escritura en curso (solo uno a la vez).
pub struct Busy {
    pub sink: Sink,
    pub label: String,
    pub cancel: CancellationToken,
    pub started: Instant,
    /// (backup, bd) de una restauración, para el historial.
    restore: Option<(String, String)>,
    /// Si lo pidió una IA, a quién avisar al terminar.
    reply: Option<oneshot::Sender<Result<String, String>>>,
}

#[derive(Default)]
pub struct Palette {
    pub query: String,
    pub sel: usize,
    pub offset: usize,
}

pub struct App {
    pub cfg: Config,
    cfg_path: PathBuf,
    pub profile: usize,
    /// Sube al cambiar de perfil: las respuestas de antes se descartan.
    generation: u64,
    pub theme: Theme,
    pub glyphs: &'static Glyphs,
    theme_stamp: Option<SystemTime>,
    pub tab: Tab,
    pub focus: Pane,
    pub hits: HitMap,
    press: Option<(u16, u16)>,
    drag_y: u16,
    pub hover: Option<Position>,
    pub help: bool,
    pub palette: Option<Palette>,
    pub form: Option<Form>,
    pub confirm: Option<Confirm>,
    pub toast: Option<(String, Instant)>,
    /// Panel cuyo filtro se está escribiendo.
    pub filtering: Option<Pane>,
    tx: UnboundedSender<Ev>,
    pub busy: Option<Busy>,
    pub progress: Option<f64>,
    pub spin: usize,
    ticks: u64,
    pub quit: bool,

    pub dbs: Vec<DbInfo>,
    pub dbs_loading: bool,
    pub dbs_err: Option<String>,
    pub db_list: ListState,
    pub db_log: LogView,

    pub backups: Vec<Backup>,
    pub backups_err: Option<String>,
    pub backup_list: ListState,
    pub restore_log: LogView,
    /// BD elegida con «Restaurar aquí» en la pestaña BDs.
    pub restore_target: Option<String>,

    pub inst_status: String,
    pub inst_log: LogView,
    logs_job: Option<CancellationToken>,
    /// Número del seguimiento de log en curso (los avisos de uno viejo se ignoran).
    logs_seq: u64,
    pub dev: bool,

    pub mod_db: Option<String>,
    pub modules: Vec<ModuleInfo>,
    pub mods_loading: bool,
    pub mods_err: Option<String>,
    pub mod_list: ListState,
    pub marks: BTreeSet<String>,
    pub mod_log: LogView,

    pub prof_list: ListState,
    pub prof_log: LogView,
    /// Último pedido de una IA (para el indicador de la cabecera).
    pub ai_seen: Option<Instant>,
    /// La TUI atiende a la IA por el socket.
    pub ai_listening: bool,
    pub history: Vec<Entry>,
}

impl App {
    pub fn new(cfg: Config, cfg_path: PathBuf, tx: UnboundedSender<Ev>) -> Self {
        let theme = Theme::by_name(cfg.theme.as_deref());
        let profile = cfg.default_index();
        Self {
            theme,
            glyphs: if cfg.icons.unwrap_or(true) {
                &NERD
            } else {
                &PLAIN
            },
            theme_stamp: theme::omarchy_stamp(),
            profile,
            cfg,
            cfg_path,
            generation: 0,
            tab: Tab::Dbs,
            focus: Pane::DbList,
            hits: HitMap::default(),
            press: None,
            drag_y: 0,
            hover: None,
            help: false,
            palette: None,
            form: None,
            confirm: None,
            toast: None,
            filtering: None,
            tx,
            busy: None,
            progress: None,
            spin: 0,
            ticks: 0,
            quit: false,
            dbs: vec![],
            dbs_loading: false,
            dbs_err: None,
            db_list: ListState::default(),
            db_log: LogView::default(),
            backups: vec![],
            backups_err: None,
            backup_list: ListState::default(),
            restore_log: LogView::default(),
            restore_target: None,
            inst_status: "—".into(),
            inst_log: LogView::default(),
            logs_job: None,
            logs_seq: 0,
            dev: true,
            mod_db: None,
            modules: vec![],
            mods_loading: false,
            mods_err: None,
            mod_list: ListState::default(),
            marks: BTreeSet::new(),
            mod_log: LogView::default(),
            prof_list: ListState {
                sel: profile,
                ..Default::default()
            },
            prof_log: LogView::default(),
            ai_seen: None,
            ai_listening: false,
            history: history::load(),
        }
    }

    /// Primera carga: sin perfiles se abre directamente el formulario.
    pub fn start(&mut self) {
        if self.cfg.profiles.is_empty() {
            self.set_tab(Tab::Profiles);
            self.form = Some(Form::profile(None, &Profile::default()));
            self.toast("crea un perfil para empezar");
            return;
        }
        self.refresh_dbs();
        self.refresh_backups();
    }

    pub fn current(&self) -> Option<&Profile> {
        self.cfg.profiles.get(self.profile)
    }

    pub fn toast(&mut self, msg: impl Into<String>) {
        self.toast = Some((msg.into(), Instant::now()));
    }

    fn tx(&self, sink: Sink) -> Tx {
        Tx::new(self.tx.clone(), sink)
    }

    pub fn log_mut(&mut self, sink: Sink) -> &mut LogView {
        match sink {
            Sink::Dbs => &mut self.db_log,
            Sink::Restore => &mut self.restore_log,
            Sink::Instance => &mut self.inst_log,
            Sink::Modules => &mut self.mod_log,
            Sink::Profiles => &mut self.prof_log,
        }
    }

    fn pane_log(&mut self, pane: Pane) -> Option<&mut LogView> {
        Some(match pane {
            Pane::DbLog => &mut self.db_log,
            Pane::RestoreLog => &mut self.restore_log,
            Pane::InstanceLog => &mut self.inst_log,
            Pane::ModLog => &mut self.mod_log,
            _ => return None,
        })
    }

    fn pane_list(&mut self, pane: Pane) -> Option<&mut ListState> {
        Some(match pane {
            Pane::DbList => &mut self.db_list,
            Pane::Backups => &mut self.backup_list,
            Pane::ModList => &mut self.mod_list,
            Pane::Profiles => &mut self.prof_list,
            _ => return None,
        })
    }

    pub fn filter_text(&self, pane: Pane) -> &str {
        match pane {
            Pane::DbList => &self.db_list.filter,
            Pane::Backups => &self.backup_list.filter,
            Pane::ModList => &self.mod_list.filter,
            Pane::DbLog => &self.db_log.filter,
            Pane::RestoreLog => &self.restore_log.filter,
            Pane::InstanceLog => &self.inst_log.filter,
            Pane::ModLog => &self.mod_log.filter,
            _ => "",
        }
    }

    fn filter_mut(&mut self, pane: Pane) -> Option<&mut String> {
        Some(match pane {
            Pane::DbList => &mut self.db_list.filter,
            Pane::Backups => &mut self.backup_list.filter,
            Pane::ModList => &mut self.mod_list.filter,
            Pane::DbLog => &mut self.db_log.filter,
            Pane::RestoreLog => &mut self.restore_log.filter,
            Pane::InstanceLog => &mut self.inst_log.filter,
            Pane::ModLog => &mut self.mod_log.filter,
            _ => return None,
        })
    }

    // ── filas visibles (índices tras el filtro) ──────────────────────────

    pub fn db_rows(&self) -> Vec<usize> {
        (0..self.dbs.len())
            .filter(|&i| self.db_list.matches(&self.dbs[i].name))
            .collect()
    }

    pub fn backup_rows(&self) -> Vec<usize> {
        (0..self.backups.len())
            .filter(|&i| self.backup_list.matches(&self.backups[i].name))
            .collect()
    }

    pub fn module_rows(&self) -> Vec<usize> {
        (0..self.modules.len())
            .filter(|&i| {
                let m = &self.modules[i];
                self.mod_list.matches(&format!("{} {}", m.name, m.state))
            })
            .collect()
    }

    fn rows_len(&self, pane: Pane) -> usize {
        match pane {
            Pane::DbList => self.db_rows().len(),
            Pane::Backups => self.backup_rows().len(),
            Pane::ModList => self.module_rows().len(),
            Pane::Profiles => self.cfg.profiles.len(),
            _ => 0,
        }
    }

    pub fn selected_db(&self) -> Option<&DbInfo> {
        self.db_rows().get(self.db_list.sel).map(|&i| &self.dbs[i])
    }

    pub fn selected_backup(&self) -> Option<&Backup> {
        self.backup_rows()
            .get(self.backup_list.sel)
            .map(|&i| &self.backups[i])
    }

    pub fn selected_module(&self) -> Option<&ModuleInfo> {
        self.module_rows()
            .get(self.mod_list.sel)
            .map(|&i| &self.modules[i])
    }

    pub fn odoo_dbs(&self) -> Vec<&str> {
        self.dbs
            .iter()
            .filter(|d| d.version.is_some())
            .map(|d| d.name.as_str())
            .collect()
    }

    // ── carga de datos ───────────────────────────────────────────────────

    pub fn refresh_dbs(&mut self) {
        let Some(p) = self.current().cloned() else {
            return;
        };
        self.dbs_loading = true;
        let (tx, g) = (self.tx.clone(), self.generation);
        tokio::spawn(async move {
            let r = pg::list(&p).await.map_err(|e| format!("{e:#}"));
            let _ = tx.send(Ev::Dbs(g, r));
        });
    }

    pub fn refresh_backups(&mut self) {
        let Some(p) = self.current().cloned() else {
            return;
        };
        let (tx, g) = (self.tx.clone(), self.generation);
        tokio::spawn(async move {
            let r = restore::list(&p.backups_dir)
                .await
                .map_err(|e| format!("{e:#}"));
            let _ = tx.send(Ev::Backups(g, r));
        });
    }

    pub fn refresh_modules(&mut self) {
        let (Some(p), Some(db)) = (self.current().cloned(), self.mod_db.clone()) else {
            return;
        };
        self.mods_loading = true;
        let (tx, g) = (self.tx.clone(), self.generation);
        tokio::spawn(async move {
            let r = modules::list(&p, &db).await.map_err(|e| format!("{e:#}"));
            let _ = tx.send(Ev::Modules(g, db, r));
        });
    }

    pub fn refresh_status(&mut self) {
        let Some(p) = self.current().cloned() else {
            return;
        };
        let (tx, g) = (self.tx.clone(), self.generation);
        tokio::spawn(async move {
            let s = instance::status(&p).await;
            let _ = tx.send(Ev::Status(g, s));
        });
    }

    /// Sigue el log de Odoo (docker logs -f o tail -F del log local).
    fn start_logs(&mut self) {
        let Some(p) = self.current().cloned() else {
            return;
        };
        if self.logs_job.is_some() || p.is_docker() && p.odoo_container.is_empty() {
            return;
        }
        if !p.is_docker() && !instance::log_path(&p).exists() {
            return;
        }
        let Ok(spec) = instance::follow(&p) else {
            return;
        };
        let cancel = CancellationToken::new();
        self.logs_job = Some(cancel.clone());
        self.logs_seq += 1;
        let tx = self.tx(Sink::Instance);
        let g = self.logs_seq;
        tokio::spawn(async move {
            if let Err(e) = exec::stream(&spec, None, &tx, &cancel, Stop::Kill).await
                && !cancel.is_cancelled()
            {
                tx.log(format!("✗ {e:#}"));
            }
            tx.send(Ev::LogsEnded(g));
        });
    }

    fn stop_logs(&mut self) {
        if let Some(c) = self.logs_job.take() {
            c.cancel();
        }
    }

    /// Lanza un trabajo de escritura; devuelve `false` si ya había otro.
    fn spawn_busy<F, Fut>(&mut self, sink: Sink, label: impl Into<String>, f: F) -> bool
    where
        F: FnOnce(Tx, CancellationToken) -> Fut,
        Fut: Future<Output = anyhow::Result<String>> + Send + 'static,
    {
        if let Some(b) = &self.busy {
            let msg = format!("espera: {} en curso", b.label);
            self.toast(msg);
            return false;
        }
        let cancel = CancellationToken::new();
        let tx = self.tx(sink);
        let fut = f(tx.clone(), cancel.clone());
        self.busy = Some(Busy {
            sink,
            label: label.into(),
            cancel,
            started: Instant::now(),
            restore: None,
            reply: None,
        });
        tokio::spawn(async move {
            let r = fut.await.map_err(|e| format!("{e:#}"));
            tx.send(Ev::Done(sink, r));
        });
        true
    }

    // ── eventos de las tareas ────────────────────────────────────────────

    pub fn on_ev(&mut self, ev: Ev) {
        match ev {
            Ev::Log(sink, line) => self.log_mut(sink).push(line),
            Ev::Progress(p) => self.progress = Some(p),
            Ev::Done(sink, r) => self.on_done(sink, r),
            Ev::Dbs(g, r) if g == self.generation => {
                self.dbs_loading = false;
                match r {
                    Ok(dbs) => {
                        // Conserva la BD seleccionada aunque cambie el orden.
                        let keep = self.selected_db().map(|d| d.name.clone());
                        self.dbs = dbs;
                        self.dbs_err = None;
                        if let Some(name) = keep
                            && let Some(pos) = self
                                .db_rows()
                                .iter()
                                .position(|&i| self.dbs[i].name == name)
                        {
                            self.db_list.sel = pos;
                        }
                        let n = self.db_rows().len();
                        self.db_list.clamp(n);
                        let odoo = self.odoo_dbs();
                        if self.mod_db.as_deref().is_none_or(|d| !odoo.contains(&d)) {
                            self.mod_db = odoo.first().map(|s| s.to_string());
                            self.modules.clear();
                            self.marks.clear();
                            if self.tab == Tab::Modules {
                                self.refresh_modules();
                            }
                        }
                    }
                    Err(e) => {
                        self.dbs.clear();
                        self.dbs_err = Some(e);
                    }
                }
            }
            Ev::Backups(g, r) if g == self.generation => match r {
                Ok(b) => {
                    self.backups = b;
                    self.backups_err = None;
                    let n = self.backup_rows().len();
                    self.backup_list.clamp(n);
                }
                Err(e) => {
                    self.backups.clear();
                    self.backups_err = Some(e);
                }
            },
            Ev::Modules(g, db, r)
                if g == self.generation && self.mod_db.as_deref() == Some(&db) =>
            {
                self.mods_loading = false;
                match r {
                    Ok(m) => {
                        self.modules = m;
                        self.mods_err = None;
                        let n = self.module_rows().len();
                        self.mod_list.clamp(n);
                    }
                    Err(e) => {
                        self.modules.clear();
                        self.mods_err = Some(e);
                    }
                }
            }
            Ev::Status(g, s) if g == self.generation => self.inst_status = s,
            Ev::LogsEnded(seq) if seq == self.logs_seq => self.logs_job = None,
            Ev::Ai(req) => self.on_ai(req),
            _ => {}
        }
    }

    fn on_done(&mut self, sink: Sink, r: Result<String, String>) {
        let mut busy = self.busy.take();
        let reply = busy.as_mut().and_then(|b| b.reply.take());
        let from_ai = reply.is_some();
        // El resultado de una IA trae además un resumen del log: aquí basta
        // la primera línea, el log ya está en pantalla.
        let first = first_line(&r);
        let mark = if r.is_ok() { "✓" } else { "✗" };
        let ai = if from_ai { "IA: " } else { "" };
        self.log_mut(sink).push(format!("{mark} {ai}{first}"));
        self.toast(format!("{mark} {ai}{first}"));
        if let Some(tx) = reply {
            let _ = tx.send(r.clone());
        }
        match sink {
            Sink::Restore => {
                if r.is_err() {
                    self.progress = None;
                }
                if let Some((backup, db)) = busy.and_then(|b| b.restore) {
                    let profile = self.current().map(|p| p.name.clone()).unwrap_or_default();
                    history::push(
                        &mut self.history,
                        Entry {
                            at: chrono::Local::now(),
                            profile,
                            backup,
                            db,
                            ok: r.is_ok(),
                            message: format!("{ai}{first}"),
                        },
                    );
                }
                self.refresh_dbs();
            }
            Sink::Dbs => self.refresh_dbs(),
            Sink::Modules => {
                self.refresh_modules();
                // Una IA decide sola si reinicia; al usuario se le pregunta.
                if r.is_ok() && !from_ai && self.inst_status == "running" {
                    self.confirm = Some(Confirm {
                        message:
                            "Módulos listos. ¿Reiniciar la instancia para cargar el código nuevo?"
                                .into(),
                        then: Pending::RestartInstance,
                    });
                }
            }
            Sink::Instance => {
                self.refresh_status();
                // `docker logs -f` termina al parar el contenedor: hay que
                // volver a engancharse. En local `tail -F` sigue solo.
                if self.current().is_some_and(Profile::is_docker) {
                    self.stop_logs();
                }
                self.start_logs();
            }
            Sink::Profiles => {}
        }
    }

    // ── acciones ─────────────────────────────────────────────────────────

    pub fn set_tab(&mut self, tab: Tab) {
        self.tab = tab;
        self.focus = tab.panes()[0];
        self.filtering = None;
        match tab {
            Tab::Instance => {
                self.refresh_status();
                self.start_logs();
            }
            Tab::Modules if self.modules.is_empty() && !self.mods_loading => {
                if self.dbs.is_empty() {
                    self.refresh_dbs();
                } else {
                    self.refresh_modules();
                }
            }
            Tab::Restore if self.backups.is_empty() => self.refresh_backups(),
            _ => {}
        }
    }

    fn switch_profile(&mut self, i: usize) {
        if i >= self.cfg.profiles.len() || i == self.profile && !self.dbs.is_empty() {
            return;
        }
        if self.busy.is_some() {
            return self.toast("espera a que termine la tarea en curso");
        }
        self.stop_logs();
        self.profile = i;
        self.generation += 1;
        self.dbs.clear();
        self.dbs_err = None;
        self.backups.clear();
        self.backups_err = None;
        self.modules.clear();
        self.marks.clear();
        self.mod_db = None;
        self.inst_status = "—".into();
        self.inst_log.clear();
        self.restore_target = None;
        let name = self.cfg.profiles[i].name.clone();
        self.toast(format!("perfil: {name}"));
        self.refresh_dbs();
        self.refresh_backups();
        if self.tab == Tab::Instance {
            self.refresh_status();
            self.start_logs();
        }
    }

    pub fn apply(&mut self, a: Action) {
        match a {
            Action::Tab(t) => self.set_tab(t),
            Action::NextProfile => {
                let n = self.cfg.profiles.len();
                if n > 0 {
                    self.switch_profile((self.profile + 1) % n);
                }
            }
            Action::UseProfile(i) => self.switch_profile(i),
            Action::Row(pane, i) => self.click_row(pane, i),
            Action::Toggle(Pane::ModList, i) => {
                self.focus = Pane::ModList;
                self.mod_list.sel = i;
                self.toggle_mark();
            }
            Action::Toggle(..) => {}
            Action::Focus(p) => self.focus = p,
            Action::Filter(p) => {
                self.focus = p;
                self.filtering = Some(p);
            }
            Action::Cmd(c) => self.cmd(c),
            Action::Help => self.help = !self.help,
            Action::Palette => {
                self.palette = match self.palette {
                    Some(_) => None,
                    None => Some(Palette::default()),
                }
            }
            Action::Close => {
                // Cierra la capa de más arriba.
                if self.confirm.is_some() {
                    self.confirm = None;
                } else if self.palette.is_some() {
                    self.palette = None;
                } else if self.help {
                    self.help = false;
                } else {
                    self.filtering = None;
                }
            }
            Action::Quit => self.request_quit(),
            Action::Block => {}
            Action::FormFocus(i) => {
                if let Some(f) = &mut self.form {
                    f.focus_field(i);
                }
            }
            Action::FormPrev(i) => {
                if let Some(f) = &mut self.form {
                    f.cycle(i, -1);
                }
            }
            Action::FormNext(i) => {
                if let Some(f) = &mut self.form {
                    f.cycle(i, 1);
                }
            }
            Action::FormSave => self.save_form(),
            Action::FormCancel => self.form = None,
            Action::ConfirmYes => {
                if let Some(c) = self.confirm.take() {
                    self.run_pending(c.then);
                }
            }
            Action::ConfirmNo => self.confirm = None,
        }
    }

    fn request_quit(&mut self) {
        if self.busy.is_some() {
            self.confirm = Some(Confirm {
                message: "Hay una tarea en curso; salir la cancela. ¿Salir?".into(),
                then: Pending::Quit,
            });
        } else {
            self.quit = true;
        }
    }

    fn click_row(&mut self, pane: Pane, i: usize) {
        if pane == Pane::Palette {
            let items = self.palette_items();
            if let Some((_, a)) = items.get(i) {
                let a = *a;
                self.palette = None;
                self.apply(a);
            }
            return;
        }
        let was = self.focus == pane;
        self.focus = pane;
        let Some(list) = self.pane_list(pane) else {
            return;
        };
        if was && list.sel == i {
            self.activate(pane);
        } else {
            list.sel = i;
        }
    }

    /// Enter / segundo clic sobre la fila seleccionada.
    fn activate(&mut self, pane: Pane) {
        match pane {
            Pane::DbList => self.cmd(Cmd::DbModules),
            Pane::Backups => self.cmd(Cmd::Restore),
            Pane::ModList => self.toggle_mark(),
            Pane::Profiles => self.cmd(Cmd::ProfUse),
            _ => {}
        }
    }

    fn toggle_mark(&mut self) {
        if let Some(m) = self.selected_module().map(|m| m.name.clone())
            && !self.marks.remove(&m)
        {
            self.marks.insert(m);
        }
    }

    fn cmd(&mut self, c: Cmd) {
        let Some(p) = self.current().cloned() else {
            // Sin perfiles solo tiene sentido crear uno.
            if c == Cmd::ProfNew {
                self.form = Some(Form::profile(None, &Profile::default()));
            } else {
                self.set_tab(Tab::Profiles);
                self.toast("primero crea un perfil");
            }
            return;
        };
        match c {
            Cmd::Refresh => match self.tab {
                Tab::Dbs => self.refresh_dbs(),
                Tab::Restore => self.refresh_backups(),
                Tab::Instance => self.refresh_status(),
                Tab::Modules => {
                    if self.dbs.is_empty() {
                        self.refresh_dbs();
                    }
                    self.refresh_modules();
                }
                Tab::Profiles => {}
            },
            Cmd::Cancel => match &self.busy {
                Some(b) => {
                    b.cancel.cancel();
                    self.toast("cancelando…");
                }
                None => self.toast("no hay nada en curso"),
            },
            Cmd::Follow => {
                if let Some(l) = self.pane_log(self.focus_log()) {
                    l.follow = !l.follow;
                }
            }
            Cmd::ClearLog => {
                if let Some(l) = self.pane_log(self.focus_log()) {
                    l.clear();
                }
            }
            Cmd::DbDrop => {
                if let Some(db) = self.selected_db().map(|d| d.name.clone()) {
                    self.confirm = Some(Confirm {
                        message: format!(
                            "Se borrará la BD «{db}» y su filestore. No se puede deshacer."
                        ),
                        then: Pending::DropDb(db),
                    });
                }
            }
            Cmd::DbDuplicate => {
                if let Some(db) = self.selected_db() {
                    self.form = Some(Form::duplicate(&db.name));
                }
            }
            Cmd::DbRename => {
                if let Some(db) = self.selected_db() {
                    self.form = Some(Form::rename(&db.name));
                }
            }
            Cmd::DbNeutralize => {
                if let Some(db) = self.selected_db() {
                    self.form = Some(Form::neutralize(&db.name));
                }
            }
            Cmd::DbRestoreHere => {
                if let Some(db) = self.selected_db().map(|d| d.name.clone()) {
                    self.toast(format!("elige el backup para «{db}»"));
                    self.restore_target = Some(db);
                    self.set_tab(Tab::Restore);
                }
            }
            Cmd::DbModules => {
                if let Some(db) = self.selected_db() {
                    if db.version.is_none() {
                        return self.toast("no es una BD de Odoo");
                    }
                    let db = db.name.clone();
                    if self.mod_db.as_deref() != Some(&db) {
                        self.mod_db = Some(db);
                        self.modules.clear();
                        self.marks.clear();
                        self.mod_list = ListState::default();
                    }
                    self.set_tab(Tab::Modules);
                    self.refresh_modules();
                }
            }
            Cmd::Restore => self.open_restore(),
            Cmd::InstStart | Cmd::InstStop | Cmd::InstRestart => self.instance_cmd(c, &p),
            Cmd::InstLogs => {
                if self.logs_job.is_some() {
                    self.stop_logs();
                    self.toast("logs pausados");
                } else {
                    self.start_logs();
                    if self.logs_job.is_none() {
                        self.toast("todavía no hay log: inicia Odoo");
                    }
                }
            }
            Cmd::InstDev => {
                self.dev = !self.dev;
                let state = if self.dev { "activado" } else { "desactivado" };
                self.toast(format!("--dev=all {state} (al próximo inicio)"));
            }
            Cmd::InstOpen => {
                let url = p.url().to_string();
                match std::process::Command::new("xdg-open")
                    .arg(&url)
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .spawn()
                {
                    Ok(_) => self.toast(format!("abriendo {url}")),
                    Err(e) => self.toast(format!("xdg-open: {e}")),
                }
            }
            Cmd::ModDb(dir) => {
                let dbs: Vec<String> = self.odoo_dbs().iter().map(|s| s.to_string()).collect();
                if dbs.is_empty() {
                    return self.toast("no hay BDs de Odoo");
                }
                let cur = self
                    .mod_db
                    .as_ref()
                    .and_then(|d| dbs.iter().position(|x| x == d))
                    .unwrap_or(0) as i32;
                let next = (cur + i32::from(dir)).rem_euclid(dbs.len() as i32) as usize;
                self.mod_db = Some(dbs[next].clone());
                self.modules.clear();
                self.marks.clear();
                self.mod_list = ListState::default();
                self.refresh_modules();
            }
            Cmd::ModToggle => self.toggle_mark(),
            Cmd::ModClear => self.marks.clear(),
            Cmd::ModInstall | Cmd::ModUpdate => {
                let Some(db) = self.mod_db.clone() else {
                    return self.toast("elige una BD");
                };
                let mut names: Vec<String> = self.marks.iter().cloned().collect();
                if names.is_empty()
                    && let Some(m) = self.selected_module()
                {
                    names.push(m.name.clone());
                }
                if names.is_empty() {
                    return self.toast("marca algún módulo (Espacio)");
                }
                let install = c == Cmd::ModInstall;
                self.mod_log.clear();
                self.focus = Pane::ModLog;
                let verb = if install {
                    "instalando"
                } else {
                    "actualizando"
                };
                self.spawn_busy(Sink::Modules, format!("{verb} {}", names.join(",")), {
                    move |tx, cancel| async move {
                        modules::run(&p, &db, install, &names, &tx, &cancel).await
                    }
                });
            }
            Cmd::ProfNew => {
                self.form = Some(Form::profile(None, &Profile::default()));
            }
            Cmd::ProfEdit => {
                let i = self.prof_list.sel;
                if let Some(pr) = self.cfg.profiles.get(i) {
                    self.form = Some(Form::profile(Some(i), pr));
                }
            }
            Cmd::ProfDelete => {
                let i = self.prof_list.sel;
                if let Some(pr) = self.cfg.profiles.get(i) {
                    self.confirm = Some(Confirm {
                        message: format!("¿Borrar el perfil «{}»? (las BDs no se tocan)", pr.name),
                        then: Pending::DeleteProfile(i),
                    });
                }
            }
            Cmd::ProfDefault => {
                let i = self.prof_list.sel;
                if let Some(pr) = self.cfg.profiles.get(i) {
                    self.cfg.default_profile = Some(pr.name.clone());
                    self.save_cfg("perfil por defecto guardado");
                }
            }
            Cmd::ProfUse => self.switch_profile(self.prof_list.sel),
            Cmd::ProfTest => {
                let Some(pr) = self.cfg.profiles.get(self.prof_list.sel).cloned() else {
                    return;
                };
                self.prof_log.clear();
                self.spawn_busy(Sink::Profiles, format!("probando {}", pr.name), {
                    move |tx, _| async move { test_profile(&pr, &tx).await }
                });
            }
        }
    }

    /// El log del panel enfocado, o el de la pestaña.
    fn focus_log(&self) -> Pane {
        if self.focus.is_log() {
            return self.focus;
        }
        self.tab
            .panes()
            .iter()
            .copied()
            .find(|p| p.is_log())
            .unwrap_or(self.focus)
    }

    fn instance_cmd(&mut self, c: Cmd, p: &Profile) {
        let verb = match c {
            Cmd::InstStart => "start",
            Cmd::InstStop => "stop",
            _ => "restart",
        };
        let (p, dev) = (p.clone(), self.dev);
        self.inst_log.push(format!("▶ {verb}"));
        self.spawn_busy(Sink::Instance, format!("odoo {verb}"), {
            move |tx, _| async move { instance::action(&p, verb, dev, &tx).await }
        });
    }

    fn open_restore(&mut self) {
        let Some(b) = self.selected_backup().cloned() else {
            return self.toast("no hay backup seleccionado");
        };
        if b.dump.is_none() {
            return self.toast("esa carpeta no tiene dump.sql");
        }
        let (db, overwrite) = match &self.restore_target {
            Some(t) => (t.clone(), true),
            None => (sanitize(&b.name), false),
        };
        self.form = Some(Form::restore(&b.name, &db, overwrite, b.filestore));
    }

    fn save_cfg(&mut self, ok: &str) {
        match self.cfg.save(&self.cfg_path) {
            Ok(()) => self.toast(ok.to_string()),
            Err(e) => self.toast(format!("✗ no pude guardar: {e:#}")),
        }
    }

    fn save_form(&mut self) {
        let Some(form) = self.form.take() else { return };
        match self.submit(&form) {
            Ok(()) => {}
            Err(e) => {
                let mut form = form;
                form.error = Some(e);
                self.form = Some(form);
            }
        }
    }

    fn submit(&mut self, form: &Form) -> Result<(), String> {
        let p = self.current().cloned();
        match &form.target {
            Target::Profile(index) => {
                let new = form.to_profile()?;
                let clash = self
                    .cfg
                    .profiles
                    .iter()
                    .enumerate()
                    .any(|(i, x)| x.name == new.name && Some(i) != *index);
                if clash {
                    return Err(format!("ya hay un perfil «{}»", new.name));
                }
                let first = self.cfg.profiles.is_empty();
                match index {
                    Some(i) => {
                        let old = std::mem::replace(&mut self.cfg.profiles[*i], new.clone());
                        if self.cfg.default_profile.as_ref() == Some(&old.name) {
                            self.cfg.default_profile = Some(new.name.clone());
                        }
                    }
                    None => {
                        self.cfg.profiles.push(new.clone());
                        self.prof_list.sel = self.cfg.profiles.len() - 1;
                    }
                }
                if first {
                    self.cfg.default_profile = Some(new.name.clone());
                }
                self.save_cfg(&format!("perfil «{}» guardado", new.name));
                // Si se editó el perfil activo, se recarga todo.
                if first || *index == Some(self.profile) {
                    let i = if first { 0 } else { self.profile };
                    self.profile = usize::MAX;
                    self.dbs.clear();
                    self.switch_profile(i);
                    if self.profile == usize::MAX {
                        self.profile = i;
                    }
                }
                Ok(())
            }
            Target::Restore { backup } => {
                let p = p.ok_or("sin perfil")?;
                let db = form.value("db");
                pg::valid_name(&db).map_err(|e| e.to_string())?;
                let overwrite = form.checked("overwrite");
                let exists = self.dbs.iter().any(|d| d.name == db);
                if exists && !overwrite {
                    return Err(format!(
                        "«{db}» ya existe: marca «Sobrescribir» u otro nombre"
                    ));
                }
                let req = Request {
                    profile: p,
                    backup: backup.clone(),
                    db: db.clone(),
                    overwrite,
                    filestore: form.checked("filestore"),
                    neutralize: form.checked("neutralize"),
                    admin_password: form.value("admin_password"),
                };
                if exists {
                    self.confirm = Some(Confirm {
                        message: format!(
                            "La BD «{db}» se borrará y se reemplazará con «{backup}». ¿Seguir?"
                        ),
                        then: Pending::Restore(Box::new(req)),
                    });
                } else {
                    self.start_restore(req);
                }
                Ok(())
            }
            Target::Duplicate(from) => {
                let p = p.ok_or("sin perfil")?;
                let to = form.value("name");
                pg::valid_name(&to).map_err(|e| e.to_string())?;
                if self.dbs.iter().any(|d| d.name == to) {
                    return Err(format!("«{to}» ya existe"));
                }
                let from = from.clone();
                self.focus = Pane::DbLog;
                self.spawn_busy(Sink::Dbs, format!("duplicando {from}"), {
                    move |tx, _| async move {
                        tx.log(format!("▶ duplicando «{from}» → «{to}»"));
                        pg::duplicate(&p, &from, &to).await?;
                        filestore::copy(&p, &from, &to, &tx).await?;
                        Ok(format!("«{from}» duplicada como «{to}»"))
                    }
                });
                Ok(())
            }
            Target::Rename(from) => {
                let p = p.ok_or("sin perfil")?;
                let to = form.value("name");
                pg::valid_name(&to).map_err(|e| e.to_string())?;
                if to == *from {
                    return Ok(());
                }
                if self.dbs.iter().any(|d| d.name == to) {
                    return Err(format!("«{to}» ya existe"));
                }
                let from = from.clone();
                self.focus = Pane::DbLog;
                self.spawn_busy(Sink::Dbs, format!("renombrando {from}"), {
                    move |tx, _| async move {
                        tx.log(format!("▶ renombrando «{from}» → «{to}»"));
                        pg::rename(&p, &from, &to).await?;
                        filestore::rename(&p, &from, &to, &tx).await?;
                        Ok(format!("«{from}» ahora es «{to}»"))
                    }
                });
                Ok(())
            }
            Target::Neutralize(db) => {
                let p = p.ok_or("sin perfil")?;
                let db = db.clone();
                let pw = form.value("admin_password");
                self.focus = Pane::DbLog;
                self.spawn_busy(Sink::Dbs, format!("neutralizando {db}"), {
                    move |tx, _| async move {
                        tx.log(format!("▶ neutralizando «{db}»"));
                        neutralize::run(&p, &db, &pw, &tx).await?;
                        Ok(format!("«{db}» neutralizada"))
                    }
                });
                Ok(())
            }
        }
    }

    fn start_restore(&mut self, req: Request) {
        self.set_tab(Tab::Restore);
        self.restore_log.clear();
        self.focus = Pane::RestoreLog;
        let meta = (req.backup.clone(), req.db.clone());
        let label = format!("restaurando {}", req.db);
        if self.spawn_busy(Sink::Restore, label, move |tx, cancel| async move {
            restore::run(req, &tx, &cancel).await
        }) {
            self.progress = Some(0.0);
            self.restore_target = None;
            if let Some(b) = &mut self.busy {
                b.restore = Some(meta);
            }
        }
    }

    fn run_pending(&mut self, then: Pending) {
        match then {
            Pending::DropDb(db) => {
                let Some(p) = self.current().cloned() else {
                    return;
                };
                self.focus = Pane::DbLog;
                self.spawn_busy(Sink::Dbs, format!("borrando {db}"), {
                    move |tx, _| async move {
                        tx.log(format!("▶ borrando «{db}»"));
                        pg::drop(&p, &db).await?;
                        filestore::remove(&p, &db, &tx).await?;
                        Ok(format!("«{db}» borrada"))
                    }
                });
            }
            Pending::Restore(req) => self.start_restore(*req),
            Pending::DeleteProfile(i) => {
                if i >= self.cfg.profiles.len() {
                    return;
                }
                if i == self.profile && self.busy.is_some() {
                    return self.toast("el perfil está en uso");
                }
                let gone = self.cfg.profiles.remove(i);
                if self.cfg.default_profile.as_ref() == Some(&gone.name) {
                    self.cfg.default_profile = self.cfg.profiles.first().map(|p| p.name.clone());
                }
                self.save_cfg(&format!("perfil «{}» borrado", gone.name));
                self.prof_list.clamp(self.cfg.profiles.len());
                if i == self.profile {
                    self.profile = usize::MAX;
                    self.switch_profile(0);
                    if self.profile == usize::MAX {
                        self.profile = 0;
                    }
                } else if i < self.profile {
                    self.profile -= 1;
                }
            }
            Pending::RestartInstance => {
                if let Some(p) = self.current().cloned() {
                    self.instance_cmd(Cmd::InstRestart, &p);
                }
            }
            Pending::Quit => {
                if let Some(b) = &self.busy {
                    b.cancel.cancel();
                }
                self.quit = true;
            }
        }
    }

    // ── IA (MCP) ─────────────────────────────────────────────────────────

    /// ¿Hay una IA trabajando (o lo hizo hace poco)?
    pub fn ai_active(&self) -> bool {
        self.busy.as_ref().is_some_and(|b| b.reply.is_some())
            || self.ai_seen.is_some_and(|t| t.elapsed().as_secs() < 8)
    }

    /// Un pedido de una IA: se ejecuta aquí para que el usuario lo vea en vivo.
    fn on_ai(&mut self, req: AiRequest) {
        self.ai_seen = Some(Instant::now());
        let AiRequest { tool, args, reply } = req;
        let answer = |reply: oneshot::Sender<Result<String, String>>, r: Result<String, String>| {
            let _ = reply.send(r);
        };

        // Un perfil distinto en los argumentos cambia el de la TUI.
        if tool != "switch_profile"
            && let Some(name) = tools::opt_str(&args, "profile").filter(|s| !s.is_empty())
        {
            match self.cfg.profiles.iter().position(|p| p.name == name) {
                None => return answer(reply, Err(format!("no existe el perfil «{name}»"))),
                Some(i) if i != self.profile => {
                    if let Some(b) = &self.busy {
                        let msg =
                            format!("ocupado: {} en curso; no puedo cambiar de perfil", b.label);
                        return answer(reply, Err(msg));
                    }
                    self.switch_profile(i);
                }
                _ => {}
            }
        }

        match tool.as_str() {
            "status" => {
                let profiles = self.cfg.profiles.clone();
                let current = (!profiles.is_empty()).then_some(self.profile);
                let busy = self.busy.as_ref().map(|b| b.label.clone());
                tokio::spawn(async move {
                    let text = tools::describe(&profiles, current, busy.as_deref(), true).await;
                    let _ = reply.send(Ok(text));
                });
            }
            "switch_profile" => {
                let r = resolve(&self.cfg.profiles, Some(self.profile), &args).and_then(|i| {
                    if i != self.profile && self.busy.is_some() {
                        return Err("hay una tarea en curso".to_string());
                    }
                    self.switch_profile(i);
                    Ok(format!("perfil activo: {}", self.cfg.profiles[i].name))
                });
                answer(reply, r);
            }
            "show" => {
                let tab = match tools::opt_str(&args, "tab").as_deref() {
                    Some("dbs") => Tab::Dbs,
                    Some("restore") => Tab::Restore,
                    Some("instance") => Tab::Instance,
                    Some("modules") => Tab::Modules,
                    Some("profiles") => Tab::Profiles,
                    other => return answer(reply, Err(format!("pestaña desconocida: {other:?}"))),
                };
                self.set_tab(tab);
                answer(reply, Ok(format!("mostrando {}", tab.title())));
            }
            "cancel" => {
                let r = match &self.busy {
                    Some(b) => {
                        b.cancel.cancel();
                        Ok(format!("cancelando: {}", b.label))
                    }
                    None => Ok("no hay nada en curso".into()),
                };
                answer(reply, r);
            }
            _ => {
                let Some(p) = self.current().cloned() else {
                    return answer(
                        reply,
                        Err("no hay perfiles: créalo en la pestaña Perfiles".into()),
                    );
                };
                let Some(sink) = tools::sink(&tool) else {
                    self.follow_ai_read(&tool, &args);
                    tokio::spawn(async move {
                        let r = tools::call(
                            &tool,
                            &args,
                            &p,
                            &Tx::detached(),
                            &CancellationToken::new(),
                        )
                        .await;
                        let _ = reply.send(r);
                    });
                    return;
                };
                if let Some(b) = &self.busy {
                    let msg = format!("ocupado: {} en curso (usa cancel o espera)", b.label);
                    return answer(reply, Err(msg));
                }
                self.start_ai_job(sink, tool, args, p, reply);
            }
        }
    }

    /// Lleva la vista a lo que la IA está leyendo, si no hay nada en curso.
    fn follow_ai_read(&mut self, tool: &str, args: &Value) {
        if self.busy.is_some() {
            return;
        }
        match tool {
            "list_databases" => {
                self.set_tab(Tab::Dbs);
                self.refresh_dbs();
            }
            "list_backups" => {
                self.set_tab(Tab::Restore);
                self.refresh_backups();
            }
            "list_modules" => {
                if let Some(db) = tools::opt_str(args, "database") {
                    self.select_module_db(db);
                }
                self.set_tab(Tab::Modules);
            }
            "read_logs" => self.set_tab(Tab::Instance),
            _ => {}
        }
    }

    fn select_module_db(&mut self, db: String) {
        if self.mod_db.as_deref() != Some(&db) {
            self.mod_db = Some(db);
            self.modules.clear();
            self.marks.clear();
            self.mod_list = ListState::default();
        }
        self.refresh_modules();
    }

    fn start_ai_job(
        &mut self,
        sink: Sink,
        tool: String,
        args: Value,
        p: Profile,
        reply: oneshot::Sender<Result<String, String>>,
    ) {
        let tab = match sink {
            Sink::Dbs => Tab::Dbs,
            Sink::Restore => Tab::Restore,
            Sink::Instance => Tab::Instance,
            Sink::Modules => Tab::Modules,
            Sink::Profiles => Tab::Profiles,
        };
        self.set_tab(tab);
        if let Some(log) = tab.panes().iter().copied().find(|p| p.is_log()) {
            self.focus = log;
        }
        if sink != Sink::Instance {
            self.log_mut(sink).clear();
        }
        if sink == Sink::Modules
            && let Some(db) = tools::opt_str(&args, "database")
        {
            self.select_module_db(db);
        }
        self.log_mut(sink)
            .push(format!("▶ IA: {tool} {}", brief(&args)));
        let restore = (tool == "restore_backup").then(|| {
            (
                tools::opt_str(&args, "backup").unwrap_or_default(),
                tools::opt_str(&args, "database").unwrap_or_default(),
            )
        });
        let label = format!("IA: {tool}");
        let started = self.spawn_busy(sink, label, move |tx, cancel| async move {
            tools::call(&tool, &args, &p, &tx, &cancel)
                .await
                .map_err(|e| anyhow::anyhow!(e))
        });
        if started && let Some(b) = &mut self.busy {
            b.reply = Some(reply);
            if restore.is_some() {
                self.progress = Some(0.0);
                b.restore = restore;
            }
        }
    }

    // ── paleta de comandos ───────────────────────────────────────────────

    pub fn palette_items(&self) -> Vec<(String, Action)> {
        let mut all: Vec<(String, Action)> = Tab::ALL
            .iter()
            .map(|t| (format!("Ir a {}", t.title()), Action::Tab(*t)))
            .collect();
        let cmds: &[(&str, Cmd)] = &[
            ("BDs: refrescar", Cmd::Refresh),
            ("BDs: duplicar la seleccionada", Cmd::DbDuplicate),
            ("BDs: renombrar la seleccionada", Cmd::DbRename),
            ("BDs: neutralizar la seleccionada", Cmd::DbNeutralize),
            ("BDs: borrar la seleccionada", Cmd::DbDrop),
            (
                "BDs: restaurar un backup sobre la seleccionada",
                Cmd::DbRestoreHere,
            ),
            ("Restaurar: backup seleccionado…", Cmd::Restore),
            ("Cancelar la tarea en curso", Cmd::Cancel),
            ("Instancia: iniciar", Cmd::InstStart),
            ("Instancia: parar", Cmd::InstStop),
            ("Instancia: reiniciar", Cmd::InstRestart),
            ("Instancia: logs on/off", Cmd::InstLogs),
            ("Instancia: alternar --dev=all", Cmd::InstDev),
            ("Instancia: abrir en el navegador", Cmd::InstOpen),
            ("Módulos: instalar marcados", Cmd::ModInstall),
            ("Módulos: actualizar marcados", Cmd::ModUpdate),
            ("Módulos: desmarcar todo", Cmd::ModClear),
            ("Perfiles: nuevo", Cmd::ProfNew),
            ("Perfiles: editar seleccionado", Cmd::ProfEdit),
            ("Perfiles: probar conexión", Cmd::ProfTest),
            ("Log: seguir el final on/off", Cmd::Follow),
            ("Log: limpiar", Cmd::ClearLog),
        ];
        all.extend(cmds.iter().map(|(l, c)| (l.to_string(), Action::Cmd(*c))));
        for (i, p) in self.cfg.profiles.iter().enumerate() {
            if i != self.profile {
                all.push((format!("Perfil: usar {}", p.name), Action::UseProfile(i)));
            }
        }
        all.push(("Ayuda".into(), Action::Help));
        all.push(("Salir".into(), Action::Quit));
        let Some(pal) = &self.palette else { return all };
        let q = crate::log::fold(&pal.query);
        let words: Vec<&str> = q.split_whitespace().collect();
        all.into_iter()
            .filter(|(l, _)| {
                let l = crate::log::fold(l);
                words.iter().all(|w| l.contains(w))
            })
            .collect()
    }

    fn palette_key(&mut self, k: KeyEvent) {
        let items = self.palette_items();
        let rows = self.hits.visible(Pane::Palette);
        let Some(pal) = &mut self.palette else { return };
        match k.code {
            KeyCode::Esc => {
                self.palette = None;
                return;
            }
            KeyCode::Enter => {
                if let Some((_, a)) = items.get(pal.sel) {
                    let a = *a;
                    self.palette = None;
                    self.apply(a);
                }
                return;
            }
            KeyCode::Down => pal.sel = (pal.sel + 1).min(items.len().saturating_sub(1)),
            KeyCode::Up => pal.sel = pal.sel.saturating_sub(1),
            KeyCode::Backspace => {
                pal.query.pop();
                pal.sel = 0;
            }
            KeyCode::Char(c) => {
                pal.query.push(c);
                pal.sel = 0;
            }
            _ => {}
        }
        if pal.sel < pal.offset {
            pal.offset = pal.sel;
        } else if pal.sel >= pal.offset + rows {
            pal.offset = pal.sel + 1 - rows;
        }
    }

    // ── teclado y mouse ──────────────────────────────────────────────────

    pub fn on_event(&mut self, ev: TermEvent) {
        match ev {
            TermEvent::Key(k) if k.kind == KeyEventKind::Press => self.on_key(k),
            TermEvent::Mouse(m) => self.on_mouse(m),
            TermEvent::Paste(text) => {
                if let Some(f) = &mut self.form {
                    f.paste(&text);
                } else if let Some(p) = &mut self.palette {
                    p.query.push_str(text.trim());
                } else if let Some(pane) = self.filtering
                    && let Some(s) = self.filter_mut(pane)
                {
                    s.push_str(text.trim());
                }
            }
            _ => {}
        }
    }

    fn on_key(&mut self, k: KeyEvent) {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl && k.code == KeyCode::Char('c') {
            return self.request_quit();
        }
        if self.confirm.is_some() {
            return match k.code {
                KeyCode::Enter | KeyCode::Char('s' | 'y') => self.apply(Action::ConfirmYes),
                KeyCode::Esc | KeyCode::Char('n') => self.apply(Action::ConfirmNo),
                _ => {}
            };
        }
        if let Some(f) = &mut self.form {
            match f.key(k) {
                form::Outcome::Save => self.save_form(),
                form::Outcome::Cancel => self.form = None,
                form::Outcome::Continue => {}
            }
            return;
        }
        if self.palette.is_some() {
            return self.palette_key(k);
        }
        if let Some(pane) = self.filtering {
            return self.filter_key(pane, k);
        }
        if self.help {
            if matches!(k.code, KeyCode::Esc | KeyCode::Char('?' | 'q')) {
                self.help = false;
            }
            return;
        }

        // Globales.
        let a = match k.code {
            KeyCode::Char('q') => Some(Action::Quit),
            KeyCode::Char('?') => Some(Action::Help),
            KeyCode::Char(':') => Some(Action::Palette),
            KeyCode::Char('p') => Some(Action::NextProfile),
            KeyCode::Tab => Some(Action::Tab(
                Tab::ALL[(self.tab.index() + 1) % Tab::ALL.len()],
            )),
            KeyCode::BackTab => Some(Action::Tab(
                Tab::ALL[(self.tab.index() + Tab::ALL.len() - 1) % Tab::ALL.len()],
            )),
            KeyCode::Char(c @ '1'..='5') => Some(Action::Tab(Tab::ALL[c as usize - '1' as usize])),
            KeyCode::Char('X') => Some(Action::Cmd(Cmd::Cancel)),
            KeyCode::Char('/') if self.filter_mut(self.focus).is_some() => {
                Some(Action::Filter(self.focus))
            }
            _ => None,
        };
        if let Some(a) = a {
            return self.apply(a);
        }

        // Navegación del panel enfocado.
        let panes = self.tab.panes();
        let fi = panes.iter().position(|p| *p == self.focus).unwrap_or(0);
        match k.code {
            KeyCode::Left | KeyCode::Char('h') => {
                self.focus = panes[(fi + panes.len() - 1) % panes.len()];
                return;
            }
            KeyCode::Right | KeyCode::Char('l') => {
                self.focus = panes[(fi + 1) % panes.len()];
                return;
            }
            KeyCode::Esc => {
                if let Some(f) = self.filter_mut(self.focus)
                    && !f.is_empty()
                {
                    f.clear();
                }
                return;
            }
            _ => {}
        }
        let page = self.hits.visible(self.focus) as i32;
        let delta = match k.code {
            KeyCode::Down | KeyCode::Char('j') => Some(1),
            KeyCode::Up | KeyCode::Char('k') => Some(-1),
            KeyCode::PageDown => Some(page),
            KeyCode::PageUp => Some(-page),
            KeyCode::Home => Some(i32::MIN / 2),
            KeyCode::End => Some(i32::MAX / 2),
            _ => None,
        };
        if let Some(d) = delta {
            return self.move_focus(d);
        }
        if k.code == KeyCode::Enter {
            return self.activate(self.focus);
        }

        // Atajos de la pestaña.
        let c = match (self.tab, k.code) {
            (_, KeyCode::Char('f')) if self.focus.is_log() || self.tab == Tab::Instance => {
                Cmd::Follow
            }
            (_, KeyCode::Char('C')) => Cmd::ClearLog,
            (Tab::Dbs, KeyCode::Char('r')) => Cmd::Refresh,
            (Tab::Dbs, KeyCode::Char('d')) => Cmd::DbDrop,
            (Tab::Dbs, KeyCode::Char('c')) => Cmd::DbDuplicate,
            (Tab::Dbs, KeyCode::Char('n')) => Cmd::DbRename,
            (Tab::Dbs, KeyCode::Char('x')) => Cmd::DbNeutralize,
            (Tab::Dbs, KeyCode::Char('s')) => Cmd::DbRestoreHere,
            (Tab::Dbs, KeyCode::Char('m')) => Cmd::DbModules,
            (Tab::Restore, KeyCode::Char('r')) => Cmd::Refresh,
            (Tab::Restore, KeyCode::Char('R')) => Cmd::Restore,
            (Tab::Instance, KeyCode::Char('s')) => Cmd::InstStart,
            (Tab::Instance, KeyCode::Char('t')) => Cmd::InstStop,
            (Tab::Instance, KeyCode::Char('R')) => Cmd::InstRestart,
            (Tab::Instance, KeyCode::Char('L')) => Cmd::InstLogs,
            (Tab::Instance, KeyCode::Char('v')) => Cmd::InstDev,
            (Tab::Instance, KeyCode::Char('o')) => Cmd::InstOpen,
            (Tab::Instance, KeyCode::Char('r')) => Cmd::Refresh,
            (Tab::Modules, KeyCode::Char('r')) => Cmd::Refresh,
            (Tab::Modules, KeyCode::Char(' ')) => Cmd::ModToggle,
            (Tab::Modules, KeyCode::Char('i')) => Cmd::ModInstall,
            (Tab::Modules, KeyCode::Char('u')) => Cmd::ModUpdate,
            (Tab::Modules, KeyCode::Char('x')) => Cmd::ModClear,
            (Tab::Modules, KeyCode::Char('b')) => Cmd::ModDb(1),
            (Tab::Modules, KeyCode::Char('B')) => Cmd::ModDb(-1),
            (Tab::Profiles, KeyCode::Char('a')) => Cmd::ProfNew,
            (Tab::Profiles, KeyCode::Char('e')) => Cmd::ProfEdit,
            (Tab::Profiles, KeyCode::Char('d')) => Cmd::ProfDelete,
            (Tab::Profiles, KeyCode::Char('*')) => Cmd::ProfDefault,
            (Tab::Profiles, KeyCode::Char('t')) => Cmd::ProfTest,
            _ => return,
        };
        self.cmd(c);
    }

    fn filter_key(&mut self, pane: Pane, k: KeyEvent) {
        let Some(f) = self.filter_mut(pane) else {
            self.filtering = None;
            return;
        };
        match k.code {
            KeyCode::Esc => {
                f.clear();
                self.filtering = None;
            }
            KeyCode::Enter | KeyCode::Down | KeyCode::Tab => self.filtering = None,
            KeyCode::Backspace => {
                f.pop();
            }
            KeyCode::Char(c) => f.push(c),
            _ => return,
        }
        // La selección vuelve arriba al cambiar el filtro.
        if let Some(l) = self.pane_list(pane) {
            l.sel = 0;
            l.offset = 0;
        }
    }

    fn move_focus(&mut self, delta: i32) {
        let pane = self.focus;
        let rows = self.hits.visible(pane);
        if pane.is_log() {
            let max = self.hits.max_scroll(pane);
            if let Some(l) = self.pane_log(pane) {
                l.scroll(delta, max);
            }
            return;
        }
        let len = self.rows_len(pane);
        if let Some(l) = self.pane_list(pane) {
            l.step(delta, len, rows);
        }
    }

    fn on_mouse(&mut self, m: MouseEvent) {
        match m.kind {
            MouseEventKind::Moved => {
                self.hover = Some(Position::new(m.column, m.row));
            }
            MouseEventKind::Down(MouseButton::Left) => {
                self.press = Some((m.column, m.row));
                self.drag_y = m.row;
            }
            // Un clic cuenta solo si el puntero casi no se movió; así arrastrar
            // para hacer scroll no dispara acciones.
            MouseEventKind::Up(MouseButton::Left) => {
                if let Some((x, y)) = self.press.take()
                    && m.row.abs_diff(y) <= 1
                    && m.column.abs_diff(x) <= 2
                    && let Some(a) = self.hits.at(x, y)
                {
                    self.apply(a);
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                if let Some((x, y)) = self.press {
                    let delta = self.drag_y as i32 - m.row as i32;
                    self.drag_y = m.row;
                    self.scroll_at(x, y, delta);
                }
            }
            MouseEventKind::ScrollDown => self.scroll_at(m.column, m.row, 3),
            MouseEventKind::ScrollUp => self.scroll_at(m.column, m.row, -3),
            _ => {}
        }
    }

    fn scroll_at(&mut self, x: u16, y: u16, delta: i32) {
        let Some((pane, max)) = self.hits.scroll_at(x, y) else {
            return;
        };
        let clamp = |s: &mut usize| *s = (*s as i32 + delta).clamp(0, max as i32) as usize;
        match pane {
            Pane::Form => {
                if let Some(f) = &mut self.form {
                    clamp(&mut f.scroll);
                }
            }
            Pane::Palette => {
                if let Some(p) = &mut self.palette {
                    clamp(&mut p.offset);
                }
            }
            p if p.is_log() => {
                if let Some(l) = self.pane_log(p) {
                    l.scroll(delta, max);
                }
            }
            p => {
                if let Some(l) = self.pane_list(p) {
                    clamp(&mut l.offset);
                }
            }
        }
    }

    pub fn hovered(&self, r: Rect) -> bool {
        self.hover.is_some_and(|h| r.contains(h))
    }

    /// Cada 250 ms: spinner, estado de la instancia y tema del sistema.
    pub fn on_tick(&mut self) {
        self.ticks += 1;
        if self.busy.is_some() || self.dbs_loading || self.mods_loading {
            self.spin = self.spin.wrapping_add(1);
        }
        if self.ticks.is_multiple_of(20) && self.tab == Tab::Instance {
            self.refresh_status();
        }
        if self.ticks.is_multiple_of(4) && self.cfg.theme.as_deref().is_none_or(|t| t == "omarchy")
        {
            let stamp = theme::omarchy_stamp();
            if stamp != self.theme_stamp {
                self.theme_stamp = stamp;
                self.theme = Theme::by_name(self.cfg.theme.as_deref());
            }
        }
    }
}

/// Argumentos de una IA en una línea, para el log (el código va aparte).
fn brief(args: &Value) -> String {
    let mut a = args.clone();
    if let Some(o) = a.as_object_mut()
        && let Some(code) = o.get("code").and_then(Value::as_str)
    {
        let n = code.lines().count();
        o.insert("code".into(), Value::String(format!("<{n} línea(s)>")));
    }
    let s = a.to_string();
    if s.chars().count() > 160 {
        format!("{}…", s.chars().take(160).collect::<String>())
    } else {
        s
    }
}

/// Nombre de BD sugerido a partir del de la carpeta de backup.
pub fn sanitize(s: &str) -> String {
    let out: String = s
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect();
    out.trim_start_matches(['-', '.'])
        .chars()
        .take(63)
        .collect()
}

async fn test_profile(p: &Profile, tx: &Tx) -> anyhow::Result<String> {
    let mut problems = 0;
    match pg::version(p).await {
        Ok(v) => tx.log(format!("✓ Postgres {v}")),
        Err(e) => {
            problems += 1;
            tx.log(format!("✗ Postgres: {e:#}"));
        }
    }
    if p.is_docker() {
        for c in [&p.pg_container, &p.odoo_container] {
            if c.is_empty() {
                continue;
            }
            match exec::capture(&Spec::new("docker").args([
                "inspect",
                "-f",
                "{{.State.Status}}",
                c,
            ]))
            .await
            {
                Ok(s) => tx.log(format!("✓ contenedor {c}: {}", s.trim())),
                Err(e) => {
                    problems += 1;
                    tx.log(format!("✗ contenedor {c}: {e:#}"));
                }
            }
        }
    } else if !p.odoo_bin.is_empty() {
        if std::path::Path::new(&p.odoo_bin).is_file() {
            tx.log(format!("✓ {}", p.odoo_bin));
        } else {
            problems += 1;
            tx.log(format!("✗ no existe {}", p.odoo_bin));
        }
    }
    if p.backups_dir.is_empty() {
        tx.log("⚠ sin carpeta de backups");
    } else if std::path::Path::new(&p.backups_dir).is_dir() {
        tx.log(format!("✓ backups en {}", p.backups_dir));
    } else {
        problems += 1;
        tx.log(format!("✗ no existe {}", p.backups_dir));
    }
    if problems > 0 {
        anyhow::bail!("{problems} problema(s) en «{}»", p.name)
    }
    Ok(format!("«{}» funciona", p.name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitizes_backup_names() {
        assert_eq!(sanitize("prod 2025/04"), "prod_2025_04");
        assert_eq!(sanitize("-x.y"), "x.y");
        assert!(pg::valid_name(&sanitize("Producción ñ")).is_ok());
    }
}
