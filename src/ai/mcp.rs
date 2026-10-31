//! Servidor MCP por stdio (`odotui mcp`): JSON-RPC 2.0, un mensaje por línea.
//! Cada herramienta va a la TUI abierta si la hay; si no, corre sin interfaz.

use std::{
    io::Write,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::Result;
use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    sync::mpsc,
    task::JoinSet,
};

use crate::{
    ai::{bridge, headless::Headless, tools},
    jobs::Progress,
    odoo::restore::human,
};

const VERSIONS: [&str; 4] = ["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

const INSTRUCTIONS: &str = "\
odotui maneja un entorno de desarrollo de Odoo: bases de datos, backups, el servidor y los módulos.
- Empieza con `status` para ver el perfil activo (docker o local), sus rutas y si la TUI está abierta.
- Ciclo al desarrollar un módulo: entender → editar → actualizar → testear.
  1. Entender: `describe_model` (campos y quién los define), `find_views` y `get_view` (vistas y su herencia), `module_info` (manifest, dependencias y si el código cambió sin actualizar) y `search_records` (datos por el ORM).
  2. Editar el código con tus propias herramientas de archivos, en addons_dir (addons_host_dir es la misma carpeta vista desde este equipo).
  3. Actualizar: `update_modules` (o `install_modules` si es nuevo). Reinicia con `instance` restart si cambiaste código Python.
  4. Testear: `run_tests`; si algo falla, `read_logs` con errors_only. Pasa el `since` que devolvió la lectura anterior para ver solo lo nuevo.
- Para ir más a fondo: `sql_query` (solo lectura) u `odoo_shell` (ORM; hace rollback salvo env.cr.commit()).
- Para probar con datos reales: `list_backups` → `restore_backup` (zip de Odoo, .dump, .sql, .sql.gz o carpetas; neutraliza por defecto; admin/admin). Antes de algo arriesgado, `backup_database`.
- Restaurar y hacer backups informan su avance si la llamada trae `_meta.progressToken`.
- `drop_database`, `restore_backup` con overwrite, `uninstall_modules` y `sql_query` con write=true son irreversibles: confírmalo con el usuario antes.
- Si la TUI está abierta el usuario ve tu trabajo en vivo; usa `show` para llevarlo a la pestaña relevante.";

fn reply(id: &Value, result: Value) -> String {
    json!({"jsonrpc": "2.0", "id": id, "result": result}).to_string()
}

fn error(id: &Value, code: i64, message: &str) -> String {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}}).to_string()
}

async fn route(
    h: &Headless,
    tool: &str,
    args: &Value,
    progress: Option<mpsc::UnboundedSender<Progress>>,
) -> Result<String, String> {
    if let Some(r) = bridge::request(tool, args, progress.clone()).await {
        return r;
    }
    h.call(tool, args, progress).await
}

/// Convierte el avance de una tarea en `notifications/progress`, que deben
/// crecer siempre. Una restauración va de 0 a 100. Un backup primero cuenta
/// MB (pg_dump no sabe el total) y luego comprime: eso es la segunda mitad.
struct Notifier {
    token: Value,
    last: f64,
    /// Lo contado en MB cuando empezó la parte con total.
    base: Option<f64>,
    bytes: bool,
    /// Como mucho un aviso cada tanto (salvo el último, que llega al total).
    gap: Duration,
    sent: Option<Instant>,
}

impl Notifier {
    fn new(token: Value) -> Self {
        Self {
            token,
            last: -1.0,
            base: None,
            bytes: false,
            gap: Duration::from_millis(250),
            sent: None,
        }
    }

    fn next(&mut self, p: Progress) -> Option<String> {
        let round = |v: f64| (v * 100.0).round() / 100.0;
        let (progress, total, message) = match p {
            Progress::Bytes(b) => {
                self.bytes = true;
                let mb = round(b as f64 / 1_048_576.0);
                (mb, None, format!("{} escritos", human(b)))
            }
            Progress::Ratio(r) if self.bytes => {
                let base = *self.base.get_or_insert(self.last.max(1.0));
                (
                    round(base + r * base),
                    Some(round(2.0 * base)),
                    format!("comprimiendo {:.0}%", r * 100.0),
                )
            }
            Progress::Ratio(r) => (round(r * 100.0), Some(100.0), format!("{:.0}%", r * 100.0)),
        };
        let done = total.is_some_and(|t| progress >= t);
        let soon = self.sent.is_some_and(|t| t.elapsed() < self.gap);
        if progress <= self.last || soon && !done {
            return None;
        }
        self.last = progress;
        self.sent = Some(Instant::now());
        let mut params = json!({
            "progressToken": self.token,
            "progress": progress,
            "message": message,
        });
        if let Some(t) = total {
            params["total"] = json!(t);
        }
        Some(
            json!({"jsonrpc": "2.0", "method": "notifications/progress", "params": params})
                .to_string(),
        )
    }
}

pub async fn serve(cfg_path: PathBuf) -> Result<()> {
    let h = Arc::new(Headless::new(cfg_path));
    // Un solo escritor: las respuestas de llamadas en paralelo no se mezclan.
    let (out, mut out_rx) = mpsc::unbounded_channel::<String>();
    let writer = tokio::task::spawn_blocking(move || {
        let mut stdout = std::io::stdout().lock();
        while let Some(line) = out_rx.blocking_recv() {
            if writeln!(stdout, "{line}")
                .and_then(|_| stdout.flush())
                .is_err()
            {
                break;
            }
        }
    });

    let mut calls = JoinSet::new();
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        let msg: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                let _ = out.send(error(&Value::Null, -32700, &format!("JSON inválido: {e}")));
                continue;
            }
        };
        let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
        // Sin id es una notificación (p. ej. notifications/initialized): no se responde.
        let Some(id) = msg.get("id").cloned() else {
            continue;
        };
        let params = msg.get("params").cloned().unwrap_or(Value::Null);
        match method {
            "initialize" => {
                let asked = params
                    .get("protocolVersion")
                    .and_then(Value::as_str)
                    .unwrap_or(VERSIONS[1]);
                let version = if VERSIONS.contains(&asked) {
                    asked
                } else {
                    VERSIONS[1]
                };
                let _ = out.send(reply(
                    &id,
                    json!({
                        "protocolVersion": version,
                        "capabilities": {"tools": {"listChanged": false}},
                        "serverInfo": {"name": "odotui", "version": env!("CARGO_PKG_VERSION")},
                        "instructions": INSTRUCTIONS,
                    }),
                ));
            }
            "ping" => {
                let _ = out.send(reply(&id, json!({})));
            }
            "tools/list" => {
                let _ = out.send(reply(&id, json!({"tools": tools::definitions()})));
            }
            "tools/call" => {
                let name = params
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let args = params.get("arguments").cloned().unwrap_or(json!({}));
                let token = params
                    .pointer("/_meta/progressToken")
                    .filter(|t| t.is_string() || t.is_number())
                    .cloned();
                let (h, out) = (h.clone(), out.clone());
                calls.spawn(async move {
                    let progress = token.map(|token| {
                        let (tx, mut rx) = mpsc::unbounded_channel();
                        let out = out.clone();
                        tokio::spawn(async move {
                            let mut n = Notifier::new(token);
                            while let Some(p) = rx.recv().await {
                                if let Some(msg) = n.next(p) {
                                    let _ = out.send(msg);
                                }
                            }
                        });
                        tx
                    });
                    let r = route(&h, &name, &args, progress).await;
                    let (text, is_error) = match r {
                        Ok(t) => (t, false),
                        Err(t) => (t, true),
                    };
                    let _ = out.send(reply(
                        &id,
                        json!({"content": [{"type": "text", "text": text}], "isError": is_error}),
                    ));
                });
            }
            _ => {
                let _ = out.send(error(&id, -32601, &format!("método desconocido: {method}")));
            }
        }
    }
    // stdin cerrado: se terminan las llamadas en curso antes de salir.
    while calls.join_next().await.is_some() {}
    drop(out);
    let _ = writer.await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn progress(msg: &str) -> (f64, Option<f64>) {
        let v: Value = serde_json::from_str(msg).unwrap();
        let p = &v["params"];
        (p["progress"].as_f64().unwrap(), p["total"].as_f64())
    }

    #[test]
    fn progress_only_grows() {
        let mut n = Notifier::new(json!("t"));
        n.gap = Duration::ZERO;
        let a = n.next(Progress::Ratio(0.0)).unwrap();
        assert_eq!(progress(&a), (0.0, Some(100.0)));
        assert!(n.next(Progress::Ratio(0.0)).is_none());
        assert_eq!(
            progress(&n.next(Progress::Ratio(0.5)).unwrap()),
            (50.0, Some(100.0))
        );

        // Backup: MB sin total y luego la compresión como segunda mitad.
        let mut n = Notifier::new(json!(7));
        n.gap = Duration::ZERO;
        assert_eq!(
            progress(&n.next(Progress::Bytes(10 << 20)).unwrap()),
            (10.0, None)
        );
        assert_eq!(
            progress(&n.next(Progress::Bytes(20 << 20)).unwrap()),
            (20.0, None)
        );
        assert!(n.next(Progress::Ratio(0.0)).is_none());
        assert_eq!(
            progress(&n.next(Progress::Ratio(0.5)).unwrap()),
            (30.0, Some(40.0))
        );
        assert_eq!(
            progress(&n.next(Progress::Ratio(1.0)).unwrap()),
            (40.0, Some(40.0))
        );

        // Con pausa mínima se saltan los intermedios, pero no el final.
        let mut n = Notifier::new(json!("t"));
        assert!(n.next(Progress::Ratio(0.1)).is_some());
        assert!(n.next(Progress::Ratio(0.2)).is_none());
        assert!(n.next(Progress::Ratio(1.0)).is_some());
    }
}
