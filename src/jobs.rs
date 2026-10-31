//! Mensajes de las tareas en segundo plano hacia la interfaz.

use std::sync::{Arc, Mutex};

use tokio::sync::{mpsc::UnboundedSender, oneshot};

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

/// Pedido de una IA (por MCP) a la TUI abierta.
#[derive(Debug)]
pub struct AiRequest {
    pub tool: String,
    pub args: serde_json::Value,
    pub reply: oneshot::Sender<Result<String, String>>,
}

#[derive(Debug)]
pub enum Ev {
    Log(Sink, String),
    /// 0.0‥1.0, del trabajo que escribe en ese log.
    Progress(Sink, f64),
    /// Bytes escritos de una salida sin total conocido (p. ej. `pg_dump`).
    Written(Sink, u64),
    /// Fin del trabajo de escritura en curso (`App::busy`).
    Done(Sink, Result<String, String>),
    /// Las listas llevan la generación del perfil para descartar respuestas viejas.
    Dbs(u64, Result<Vec<DbInfo>, String>),
    Backups(u64, Result<Vec<Backup>, String>),
    Modules(u64, String, Result<Vec<ModuleInfo>, String>),
    Status(u64, String),
    /// Terminó el seguimiento n.º tal del log de Odoo (se detuvo o se canceló).
    LogsEnded(u64),
    Ai(AiRequest),
}

/// Remitente atado a un log; además puede guardar las líneas para devolvérselas
/// a una IA.
#[derive(Debug, Clone)]
pub struct Tx {
    tx: Option<UnboundedSender<Ev>>,
    pub sink: Sink,
    /// Cada `capturing()` añade un destino más; ninguno reemplaza a otro.
    capture: Vec<Arc<Mutex<Vec<String>>>>,
}

impl Tx {
    pub fn new(tx: UnboundedSender<Ev>, sink: Sink) -> Self {
        Self {
            tx: Some(tx),
            sink,
            capture: vec![],
        }
    }

    /// Sin interfaz: las líneas solo se guardan.
    pub fn detached() -> Self {
        Self {
            tx: None,
            sink: Sink::Profiles,
            capture: vec![],
        }
    }

    /// Copia que además guarda cada línea; devuelve dónde.
    pub fn capturing(&self) -> (Self, Arc<Mutex<Vec<String>>>) {
        let buf = Arc::new(Mutex::new(Vec::new()));
        let mut t = self.clone();
        t.capture.push(buf.clone());
        (t, buf)
    }

    pub fn log(&self, line: impl Into<String>) {
        let line = line.into();
        for c in &self.capture {
            if let Ok(mut c) = c.lock() {
                c.push(line.clone());
            }
        }
        self.send(Ev::Log(self.sink, line));
    }

    pub fn progress(&self, p: f64) {
        self.send(Ev::Progress(self.sink, p));
    }

    pub fn written(&self, bytes: u64) {
        self.send(Ev::Written(self.sink, bytes));
    }

    pub fn send(&self, ev: Ev) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(ev);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nested_captures_all_receive_lines() {
        let (outer, a) = Tx::detached().capturing();
        let (inner, b) = outer.capturing();
        inner.log("x");
        outer.log("y");
        assert_eq!(*a.lock().unwrap(), ["x", "y"]);
        assert_eq!(*b.lock().unwrap(), ["x"]);
    }
}
