//! Herramientas que odotui ofrece a una IA. Las mismas funciones corren dentro
//! de la TUI abierta (y se ven en vivo) o sin interfaz.

use anyhow::{Context, Result, anyhow, bail};
use chrono::{DateTime, Local};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use crate::{
    config::Profile,
    jobs::{Sink, Tx},
    odoo::{
        backup, dev, filestore, instance, modules, neutralize, pg,
        restore::{self, Request, human},
    },
};

/// Log (y pestaña) de las herramientas que escriben; `None` = solo lectura.
pub fn sink(tool: &str) -> Option<Sink> {
    Some(match tool {
        "restore_backup" => Sink::Restore,
        "drop_database"
        | "duplicate_database"
        | "rename_database"
        | "neutralize_database"
        | "backup_database" => Sink::Dbs,
        "install_modules" | "update_modules" | "uninstall_modules" | "run_tests" | "odoo_shell"
        | "scaffold_module" => Sink::Modules,
        "instance" => Sink::Instance,
        _ => return None,
    })
}

fn tool(
    name: &str,
    description: &str,
    props: Value,
    required: &[&str],
    read_only: bool,
    destructive: bool,
) -> Value {
    let mut props = props;
    if let Some(o) = props.as_object_mut()
        && !matches!(name, "switch_profile" | "status" | "show" | "cancel")
    {
        o.insert(
            "profile".into(),
            json!({"type": "string", "description": "Perfil de odotui; omitido = el activo"}),
        );
    }
    json!({
        "name": name,
        "description": description,
        "inputSchema": {"type": "object", "properties": props, "required": required},
        "annotations": {
            "readOnlyHint": read_only,
            "destructiveHint": destructive,
            "openWorldHint": false,
        },
    })
}

pub fn definitions() -> Vec<Value> {
    let db = json!({"type": "string", "description": "Nombre de la base de datos"});
    let mods = json!({"type": "array", "items": {"type": "string"}, "description": "Nombres técnicos de módulos"});
    vec![
        tool(
            "status",
            "Estado general: perfil activo y perfiles disponibles (modo, contenedores, rutas), estado de la instancia de Odoo, tarea en curso y si la TUI está abierta. Llámala primero.",
            json!({}),
            &[],
            true,
            false,
        ),
        tool(
            "switch_profile",
            "Cambia el perfil activo (conexión a Postgres/Odoo) por nombre.",
            json!({"profile": {"type": "string"}}),
            &["profile"],
            false,
            false,
        ),
        tool(
            "show",
            "Muestra una pestaña en la TUI abierta para que el usuario vea lo que haces.",
            json!({"tab": {"type": "string", "enum": ["dbs", "restore", "instance", "modules", "profiles"]}}),
            &["tab"],
            true,
            false,
        ),
        tool(
            "cancel",
            "Cancela la tarea larga en curso (restauración, actualización, tests…).",
            json!({}),
            &[],
            false,
            false,
        ),
        tool(
            "list_databases",
            "Lista las bases de datos con tamaño, dueño y versión de Odoo (— = no es de Odoo).",
            json!({}),
            &[],
            true,
            false,
        ),
        tool(
            "list_backups",
            "Lista los backups de backups_dir: carpetas <carpeta>/dump.sql (+ filestore/), .zip del gestor de Odoo / Odoo.sh (con la versión de su manifest), .dump de pg_dump -Fc y .sql / .sql.gz.",
            json!({}),
            &[],
            true,
            false,
        ),
        tool(
            "list_modules",
            "Lista los módulos de una BD con estado (installed, uninstalled, to upgrade…) y versión.",
            json!({
                "database": db,
                "filter": {"type": "string", "description": "Texto a buscar en el nombre"},
                "state": {"type": "string", "description": "Solo este estado, p. ej. installed"},
            }),
            &["database"],
            true,
            false,
        ),
        tool(
            "sql_query",
            "Ejecuta SQL con psql y devuelve la tabla. Por defecto en transacción de SOLO LECTURA; para modificar datos pasa write=true. Útil para inspeccionar ir_model, ir_ui_view, ir_model_fields, datos…",
            json!({
                "database": db,
                "sql": {"type": "string"},
                "write": {"type": "boolean", "default": false},
                "limit": {"type": "integer", "default": 200, "description": "Máximo de líneas de salida"},
            }),
            &["database", "sql"],
            false,
            true,
        ),
        tool(
            "read_logs",
            "Últimas líneas del log del servidor de Odoo (contenedor o odoo-bin local).",
            json!({
                "lines": {"type": "integer", "default": 200},
                "filter": {"type": "string", "description": "Solo líneas que contengan este texto"},
                "errors_only": {"type": "boolean", "default": false, "description": "Solo ERROR/CRITICAL y sus tracebacks"},
            }),
            &[],
            true,
            false,
        ),
        tool(
            "restore_backup",
            "Restaura un backup en una BD: crea la BD, carga el dump (carpeta, .zip, .dump, .sql o .sql.gz), copia el filestore y (por defecto) neutraliza (sin crons ni correo, admin con password conocido). Si la BD existe hace falta overwrite=true y se BORRA antes.",
            json!({
                "backup": {"type": "string", "description": "Nombre de la carpeta o archivo (ver list_backups)"},
                "database": db,
                "overwrite": {"type": "boolean", "default": false},
                "neutralize": {"type": "boolean", "default": true},
                "filestore": {"type": "boolean", "default": true},
                "admin_password": {"type": "string", "default": "admin", "description": "Vacío = no cambiar"},
            }),
            &["backup", "database"],
            false,
            true,
        ),
        tool(
            "backup_database",
            "Hace un backup de una BD en backups_dir: zip compatible con el gestor de Odoo (dump.sql + filestore/ + manifest.json) o dump de pg_dump -Fc (sin filestore). Devuelve la ruta del archivo.",
            json!({
                "database": db,
                "format": {"type": "string", "enum": ["zip", "dump"], "default": "zip"},
                "filestore": {"type": "boolean", "default": true, "description": "Solo en zip"},
            }),
            &["database"],
            false,
            false,
        ),
        tool(
            "drop_database",
            "BORRA una base de datos y su filestore. Irreversible.",
            json!({"database": db}),
            &["database"],
            false,
            true,
        ),
        tool(
            "duplicate_database",
            "Copia una BD (y su filestore) con otro nombre; útil para probar sin riesgo.",
            json!({"source": db, "target": {"type": "string"}}),
            &["source", "target"],
            false,
            false,
        ),
        tool(
            "rename_database",
            "Renombra una BD (y su filestore).",
            json!({"source": db, "target": {"type": "string"}}),
            &["source", "target"],
            false,
            false,
        ),
        tool(
            "neutralize_database",
            "Neutraliza una BD para desarrollo: desactiva crons, servidores de correo y pagos, regenera el uuid, apunta web.base.url a local y cambia el password de admin.",
            json!({"database": db, "admin_password": {"type": "string", "default": "admin"}}),
            &["database"],
            false,
            false,
        ),
        tool(
            "install_modules",
            "Instala módulos en una BD (odoo -i … --stop-after-init). Devuelve los errores del log si los hay.",
            json!({"database": db, "modules": mods}),
            &["database", "modules"],
            false,
            false,
        ),
        tool(
            "update_modules",
            "Actualiza módulos en una BD (odoo -u … --stop-after-init) tras cambiar su código (modelos, vistas, datos). Devuelve los errores del log si los hay. Reinicia la instancia después para cargar el código Python nuevo.",
            json!({"database": db, "modules": mods}),
            &["database", "modules"],
            false,
            false,
        ),
        tool(
            "uninstall_modules",
            "Desinstala módulos de una BD (button_immediate_uninstall por el ORM). Borra sus datos, vistas y tablas: confírmalo con el usuario antes.",
            json!({"database": db, "modules": mods}),
            &["database", "modules"],
            false,
            true,
        ),
        tool(
            "run_tests",
            "Corre los tests de módulos (-u <módulos> --test-enable --test-tags). Devuelve el resumen y cada fallo con su traceback.",
            json!({
                "database": db,
                "modules": mods,
                "test_tags": {"type": "string", "description": "Opcional, p. ej. /mi_modulo:TestX.test_y; por defecto /<módulo> de cada uno"},
            }),
            &["database", "modules"],
            false,
            false,
        ),
        tool(
            "odoo_shell",
            "Ejecuta código Python en `odoo shell` con `env` (ORM) disponible, p. ej. print(env['res.partner'].search_count([])). Al terminar se hace ROLLBACK: llama a env.cr.commit() si quieres guardar cambios. Usa print() para ver resultados.",
            json!({"database": db, "code": {"type": "string"}}),
            &["database", "code"],
            false,
            true,
        ),
        tool(
            "scaffold_module",
            "Crea el esqueleto de un módulo nuevo (odoo scaffold) en la carpeta addons_dir del perfil y devuelve su ruta.",
            json!({"name": {"type": "string", "description": "Nombre técnico: minúsculas, números y _"}}),
            &["name"],
            false,
            false,
        ),
        tool(
            "instance",
            "Inicia, detiene o reinicia el servidor de Odoo (contenedor en docker; odoo-bin en segundo plano en local).",
            json!({
                "action": {"type": "string", "enum": ["start", "stop", "restart"]},
                "dev": {"type": "boolean", "default": true, "description": "Solo local: --dev=all"},
            }),
            &["action"],
            false,
            false,
        ),
    ]
}

// ── argumentos ───────────────────────────────────────────────────────────

pub fn str_arg(args: &Value, k: &str) -> Result<String> {
    args.get(k)
        .and_then(Value::as_str)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow!("falta el argumento «{k}»"))
}

pub fn opt_str(args: &Value, k: &str) -> Option<String> {
    args.get(k).and_then(Value::as_str).map(String::from)
}

fn bool_arg(args: &Value, k: &str, default: bool) -> bool {
    args.get(k).and_then(Value::as_bool).unwrap_or(default)
}

fn num_arg(args: &Value, k: &str, default: usize) -> usize {
    args.get(k)
        .and_then(Value::as_u64)
        .map_or(default, |n| n as usize)
}

/// Lista de módulos como array o como texto separado por comas.
fn list_arg(args: &Value, k: &str) -> Result<Vec<String>> {
    let v: Vec<String> = match args.get(k) {
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(Value::as_str)
            .map(|s| s.trim().to_string())
            .collect(),
        Some(Value::String(s)) => s.split(',').map(|s| s.trim().to_string()).collect(),
        _ => vec![],
    };
    let v: Vec<String> = v.into_iter().filter(|s| !s.is_empty()).collect();
    if v.is_empty() {
        bail!("falta la lista «{k}»");
    }
    for m in &v {
        dev::valid_module(m)?;
    }
    Ok(v)
}

fn db_arg(args: &Value, k: &str) -> Result<String> {
    let db = str_arg(args, k)?;
    pg::valid_name(&db)?;
    Ok(db)
}

// ── ejecución ────────────────────────────────────────────────────────────

/// Ejecuta una herramienta que no es de `HOST_TOOLS`. Las que generan log
/// devuelven, además del resultado, sus errores y el final de la salida.
pub async fn call(
    tool: &str,
    args: &Value,
    p: &Profile,
    tx: &Tx,
    cancel: &CancellationToken,
) -> Result<String, String> {
    let (tx, lines) = tx.capturing();
    let r = run(tool, args, p, &tx, cancel).await;
    let lines = lines.lock().map(|l| l.clone()).unwrap_or_default();
    let tail = match tool {
        "odoo_shell" => 150,
        "run_tests" => 20,
        _ => 30,
    };
    let extra = if lines.is_empty() || sink(tool).is_none() {
        String::new()
    } else {
        format!("\n\n{}", dev::digest(&lines, tail))
    };
    match r {
        Ok(s) => Ok(format!("{s}{extra}")),
        Err(e) => Err(format!("{e:#}{extra}")),
    }
}

async fn run(
    tool: &str,
    args: &Value,
    p: &Profile,
    tx: &Tx,
    cancel: &CancellationToken,
) -> Result<String> {
    match tool {
        "list_databases" => {
            let dbs = pg::list(p).await?;
            let mut out = format!("{} base(s) en «{}»:\n", dbs.len(), p.name);
            for d in dbs {
                out += &format!(
                    "{}\todoo={}\t{}\tdueño={}\n",
                    d.name,
                    d.version.as_deref().unwrap_or("—"),
                    human(d.size),
                    d.owner
                );
            }
            Ok(out)
        }
        "list_backups" => {
            let bs = restore::list(&p.backups_dir).await?;
            let mut out = format!("{} backup(s) en {}:\n", bs.len(), p.backups_dir);
            for b in bs {
                let date = b
                    .modified
                    .map(|m| {
                        DateTime::<Local>::from(m)
                            .format("%Y-%m-%d %H:%M")
                            .to_string()
                    })
                    .unwrap_or_default();
                let odoo = b
                    .manifest
                    .as_ref()
                    .map(|m| format!("\todoo={}", restore::major(&m.version)))
                    .unwrap_or_default();
                out += &format!(
                    "{}\t{}\tdump={}\tfilestore={}{odoo}\t{date}\n",
                    b.name,
                    b.kind.label(),
                    b.dump.map(human).unwrap_or_else(|| "NO".into()),
                    if b.filestore { "sí" } else { "no" },
                );
            }
            Ok(out)
        }
        "list_modules" => {
            let db = db_arg(args, "database")?;
            let filter = opt_str(args, "filter").unwrap_or_default().to_lowercase();
            let state = opt_str(args, "state").unwrap_or_default();
            let ms: Vec<_> = modules::list(p, &db)
                .await?
                .into_iter()
                .filter(|m| filter.is_empty() || m.name.contains(&filter))
                .filter(|m| state.is_empty() || m.state == state)
                .collect();
            let mut out = format!("{} módulo(s):\n", ms.len());
            for m in ms.iter().take(500) {
                out += &format!(
                    "{}\t{}\t{}{}\n",
                    m.name,
                    m.state,
                    m.version,
                    if m.application { "\tapp" } else { "" }
                );
            }
            if ms.len() > 500 {
                out += "… (usa filter o state para acotar)\n";
            }
            Ok(out)
        }
        "sql_query" => {
            let db = db_arg(args, "database")?;
            let sql = str_arg(args, "sql")?;
            dev::sql(
                p,
                &db,
                &sql,
                bool_arg(args, "write", false),
                num_arg(args, "limit", 200),
            )
            .await
        }
        "read_logs" => {
            let n = num_arg(args, "lines", 200).clamp(1, 5000);
            let errors = bool_arg(args, "errors_only", false);
            // Para errores se lee más y se resume.
            let mut lines = instance::read(p, if errors { n.max(2000) } else { n }).await?;
            if let Some(f) = opt_str(args, "filter").filter(|f| !f.is_empty()) {
                lines.retain(|l| l.contains(&f));
            }
            if errors {
                let d = dev::digest(&lines, 0);
                return Ok(if d.contains("── errores ──") {
                    d
                } else {
                    "sin errores en el log".into()
                });
            }
            Ok(lines.join("\n"))
        }
        "restore_backup" => {
            let req = Request {
                profile: p.clone(),
                backup: str_arg(args, "backup")?,
                db: db_arg(args, "database")?,
                overwrite: bool_arg(args, "overwrite", false),
                filestore: bool_arg(args, "filestore", true),
                neutralize: bool_arg(args, "neutralize", true),
                admin_password: opt_str(args, "admin_password").unwrap_or_else(|| "admin".into()),
            };
            restore::run(req, tx, cancel).await
        }
        "backup_database" => {
            let db = db_arg(args, "database")?;
            let format = backup::Format::parse(&opt_str(args, "format").unwrap_or_default())?;
            backup::run(
                p,
                &db,
                format,
                bool_arg(args, "filestore", true),
                tx,
                cancel,
            )
            .await
        }
        "drop_database" => {
            let db = db_arg(args, "database")?;
            tx.log(format!("▶ borrando «{db}»"));
            pg::drop(p, &db).await?;
            filestore::remove(p, &db, tx).await?;
            Ok(format!("«{db}» borrada"))
        }
        "duplicate_database" | "rename_database" => {
            let from = db_arg(args, "source")?;
            let to = db_arg(args, "target")?;
            if pg::exists(p, &to).await? {
                bail!("«{to}» ya existe");
            }
            if tool == "duplicate_database" {
                tx.log(format!("▶ duplicando «{from}» → «{to}»"));
                pg::duplicate(p, &from, &to).await?;
                filestore::copy(p, &from, &to, tx).await?;
                Ok(format!("«{from}» duplicada como «{to}»"))
            } else {
                tx.log(format!("▶ renombrando «{from}» → «{to}»"));
                pg::rename(p, &from, &to).await?;
                filestore::rename(p, &from, &to, tx).await?;
                Ok(format!("«{from}» ahora es «{to}»"))
            }
        }
        "neutralize_database" => {
            let db = db_arg(args, "database")?;
            let pw = opt_str(args, "admin_password").unwrap_or_else(|| "admin".into());
            tx.log(format!("▶ neutralizando «{db}»"));
            neutralize::run(p, &db, &pw, tx).await?;
            Ok(format!("«{db}» neutralizada"))
        }
        "install_modules" | "update_modules" => {
            let db = db_arg(args, "database")?;
            let names = list_arg(args, "modules")?;
            modules::run(p, &db, tool == "install_modules", &names, tx, cancel).await
        }
        "uninstall_modules" => {
            let db = db_arg(args, "database")?;
            let names = list_arg(args, "modules")?;
            modules::uninstall(p, &db, &names, tx, cancel).await
        }
        "run_tests" => {
            let db = db_arg(args, "database")?;
            let names = list_arg(args, "modules")?;
            let tags = opt_str(args, "test_tags").unwrap_or_default();
            dev::run_tests(p, &db, &names, &tags, tx, cancel).await
        }
        "odoo_shell" => {
            let db = db_arg(args, "database")?;
            let code = args
                .get("code")
                .and_then(Value::as_str)
                .context("falta el argumento «code»")?;
            dev::shell(p, &db, code, tx, cancel).await
        }
        "scaffold_module" => {
            let name = str_arg(args, "name")?;
            let path = dev::scaffold(p, &name, tx).await?;
            Ok(format!("módulo creado en {path}"))
        }
        "instance" => {
            let verb = str_arg(args, "action")?;
            instance::action(p, &verb, bool_arg(args, "dev", true), tx).await
        }
        other => bail!("herramienta desconocida: {other}"),
    }
}

/// Texto de `status` común a los dos modos.
pub async fn describe(
    profiles: &[Profile],
    current: Option<usize>,
    busy: Option<&str>,
    tui: bool,
) -> String {
    let mut out = String::new();
    out += if tui {
        "TUI de odotui abierta: el usuario ve en vivo lo que haces.\n"
    } else {
        "odotui sin interfaz (la TUI no está abierta).\n"
    };
    if let Some(b) = busy {
        out += &format!("Tarea en curso: {b}\n");
    }
    let Some(p) = current.and_then(|i| profiles.get(i)) else {
        out += "No hay perfiles: el usuario debe crear uno en la TUI (pestaña Perfiles).\n";
        return out;
    };
    out += &format!(
        "\nPerfil activo: {} ({})\n  postgres: {}@{}:{}{}\n",
        p.name,
        p.mode.as_str(),
        p.pg_user,
        if p.pg_host.is_empty() {
            "socket"
        } else {
            &p.pg_host
        },
        p.pg_port,
        if p.is_docker() && !p.pg_container.is_empty() {
            format!(" (psql en el contenedor {})", p.pg_container)
        } else {
            String::new()
        }
    );
    if p.is_docker() {
        out += &format!("  contenedor odoo: {}\n", p.odoo_container);
    } else {
        out += &format!("  odoo-bin: {}\n  odoo.conf: {}\n", p.odoo_bin, p.odoo_conf);
        out += &format!("  log: {}\n", instance::log_path(p).display());
    }
    out += &format!(
        "  url: {}\n  backups: {}\n  filestore: {}\n  addons propios: {}\n",
        p.url(),
        p.backups_dir,
        p.filestore(),
        if p.addons_dir.is_empty() {
            "—"
        } else {
            &p.addons_dir
        }
    );
    out += &format!("  instancia: {}\n", instance::status(p).await);
    let others: Vec<_> = profiles
        .iter()
        .filter(|x| x.name != p.name)
        .map(|x| format!("{} ({})", x.name, x.mode.as_str()))
        .collect();
    if !others.is_empty() {
        out += &format!("\nOtros perfiles: {}\n", others.join(", "));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_tool_has_schema_and_sink_rules() {
        let defs = definitions();
        assert_eq!(defs.len(), 22);
        for d in &defs {
            let name = d["name"].as_str().unwrap();
            assert!(d["inputSchema"]["type"] == "object", "{name}");
            let ro = d["annotations"]["readOnlyHint"].as_bool().unwrap();
            // Lo que escribe tiene log propio; lo de solo lectura, no.
            // status, switch_profile, show y cancel los resuelve quien aloja.
            let host = ["status", "switch_profile", "show", "cancel"].contains(&name);
            if !host && name != "sql_query" {
                assert_eq!(sink(name).is_none(), ro, "{name}");
            }
        }
    }

    #[test]
    fn module_lists_accept_array_or_csv() {
        let a = json!({"modules": ["sale", " crm "]});
        assert_eq!(list_arg(&a, "modules").unwrap(), ["sale", "crm"]);
        let b = json!({"modules": "sale,crm"});
        assert_eq!(list_arg(&b, "modules").unwrap(), ["sale", "crm"]);
        assert!(list_arg(&json!({"modules": "Bad-Name"}), "modules").is_err());
        assert!(list_arg(&json!({}), "modules").is_err());
    }
}
