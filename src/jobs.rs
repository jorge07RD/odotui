//! Mensajes de las tareas en segundo plano hacia la interfaz.

use tokio::sync::mpsc::UnboundedSender;

use crate::odoo::{modules::ModuleInfo, pg::DbInfo, restore::Backup};

/// A qué log de la interfaz va cada línea.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sink {
    Dbs,
    Restore,
    Instance,
    Modules,
    Profiles,
}

#[derive(Debug)]
pub enum Ev {
    Log(Sink, String),
    /// 0.0‥1.0
    Progress(f64),
    /// Fin del trabajo de escritura en curso (`App::busy`).
    Done(Sink, Result<String, String>),
    /// Las listas llevan la generación del perfil para descartar respuestas viejas.
    Dbs(u64, Result<Vec<DbInfo>, String>),
    Backups(u64, Result<Vec<Backup>, String>),
    Modules(u64, String, Result<Vec<ModuleInfo>, String>),
    Status(u64, String),
    /// Terminó `docker logs -f` (el contenedor se detuvo o se canceló).
    LogsEnded(u64),
    /// Terminó el `odoo-bin` lanzado en modo local.
    ServerEnded(Result<String, String>),
}

/// Remitente atado a un log.
#[derive(Debug, Clone)]
pub struct Tx {
    pub tx: UnboundedSender<Ev>,
    pub sink: Sink,
}

impl Tx {
    pub fn new(tx: UnboundedSender<Ev>, sink: Sink) -> Self {
        Self { tx, sink }
    }

    pub fn log(&self, line: impl Into<String>) {
        let _ = self.tx.send(Ev::Log(self.sink, line.into()));
    }

    pub fn progress(&self, p: f64) {
        let _ = self.tx.send(Ev::Progress(p));
    }

    pub fn send(&self, ev: Ev) {
        let _ = self.tx.send(ev);
    }
}
