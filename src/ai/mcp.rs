//! Servidor MCP por stdio (`odotui mcp`): JSON-RPC 2.0, un mensaje por línea.
//! Cada herramienta va a la TUI abierta si la hay; si no, corre sin interfaz.

use std::{io::Write, path::PathBuf, sync::Arc};

use anyhow::Result;
use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    sync::mpsc,
    task::JoinSet,
};

use crate::ai::{bridge, headless::Headless, tools};

const VERSIONS: [&str; 4] = ["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

const INSTRUCTIONS: &str = "\
odotui maneja un entorno de desarrollo de Odoo: bases de datos, backups, el servidor y los módulos.
- Empieza con `status` para ver el perfil activo (docker o local), sus rutas y si la TUI está abierta.
- Ciclo típico al desarrollar un módulo: editar el código (con tus propias herramientas de archivos, en addons_dir) → `update_modules` → `run_tests` → `read_logs` con errors_only si algo falla. Reinicia con `instance` restart si cambiaste código Python.
- Para inspeccionar datos usa `sql_query` (solo lectura) u `odoo_shell` (ORM; hace rollback salvo env.cr.commit()).
- Para probar con datos reales: `list_backups` → `restore_backup` (neutraliza por defecto; admin/admin).
- `drop_database`, `restore_backup` con overwrite y `sql_query` con write=true son irreversibles: confírmalo con el usuario antes.
- Si la TUI está abierta el usuario ve tu trabajo en vivo; usa `show` para llevarlo a la pestaña relevante.";

fn reply(id: &Value, result: Value) -> String {
    json!({"jsonrpc": "2.0", "id": id, "result": result}).to_string()
}

fn error(id: &Value, code: i64, message: &str) -> String {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}}).to_string()
}

async fn route(h: &Headless, tool: &str, args: &Value) -> Result<String, String> {
    if let Some(r) = bridge::request(tool, args).await {
        return r;
    }
    h.call(tool, args).await
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
                let (h, out) = (h.clone(), out.clone());
                calls.spawn(async move {
                    let r = route(&h, &name, &args).await;
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
