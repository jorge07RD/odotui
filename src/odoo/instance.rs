//! El servidor de Odoo: contenedor (docker) o `odoo-bin` (local).
//!
//! En local Odoo corre como proceso aparte, con su pid y su log en
//! `~/.local/state/odotui/`: sobrevive a odotui y lo ven igual la TUI y la IA.
//! Puede correr con una sola BD (`-d <bd> --db-filter=^<bd>$`): las demás ni
//! se ven ni cargan, así que tampoco corren sus crons. Cuál es se guarda junto
//! al pid (`odoo-<perfil>.db`).

use std::{io::Write, path::PathBuf, time::Duration};

use anyhow::{Context, Result, bail};
use tokio_util::sync::CancellationToken;

use crate::{
    config::Profile,
    exec::{self, Spec, Stop},
    jobs::Tx,
    odoo::modules,
};

fn container(p: &Profile) -> Result<&str> {
    if p.odoo_container.is_empty() {
        bail!("el perfil no tiene contenedor de Odoo");
    }
    Ok(&p.odoo_container)
}

pub fn state_dir() -> PathBuf {
    dirs::state_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("odotui")
}

fn file(p: &Profile, ext: &str) -> PathBuf {
    let name: String = p
        .name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    state_dir().join(format!("odoo-{name}.{ext}"))
}

pub fn log_path(p: &Profile) -> PathBuf {
    file(p, "log")
}

/// Pid del Odoo local si sigue vivo (y es de verdad un Odoo).
pub fn local_pid(p: &Profile) -> Option<i32> {
    let pid: i32 = std::fs::read_to_string(file(p, "pid"))
        .ok()?
        .trim()
        .parse()
        .ok()?;
    let cmdline = std::fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    let cmdline = String::from_utf8_lossy(&cmdline);
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // El estado va tras el nombre entre paréntesis; Z = zombi.
    let zombie = stat
        .rsplit_once(')')
        .is_some_and(|(_, rest)| rest.trim_start().starts_with('Z'));
    let ours = cmdline.contains(&p.odoo_bin) || cmdline.contains("odoo");
    (ours && !zombie).then_some(pid)
}

/// La BD con la que corre Odoo local, si se lanzó con una sola.
pub fn running_db(p: &Profile) -> Option<String> {
    if p.is_docker() {
        return None;
    }
    local_pid(p)?;
    let db = std::fs::read_to_string(file(p, "db")).ok()?;
    let db = db.trim();
    (!db.is_empty()).then(|| db.to_string())
}

/// `^bd$` con los caracteres especiales escapados (los nombres válidos solo
/// pueden traer `.` y `-`).
pub fn db_filter(db: &str) -> String {
    let mut out = String::from("^");
    for c in db.chars() {
        if !c.is_ascii_alphanumeric() && c != '_' {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('$');
    out
}

/// Con qué BD arrancar en un `start`/`restart`.
#[derive(Debug, Clone, PartialEq)]
pub enum Dbs {
    /// Todas (como siempre).
    All,
    /// Solo esta.
    Only(String),
    /// Las mismas que ahora (en `restart`); `All` si no corría.
    Same,
}

/// `running`, `exited`, `detenido`… o el error.
pub async fn status(p: &Profile) -> String {
    if !p.is_docker() {
        return match local_pid(p) {
            Some(_) => "running".into(),
            None => "detenido".into(),
        };
    }
    let Ok(c) = container(p) else {
        return "sin contenedor".into();
    };
    match exec::capture(&Spec::new("docker").args(["inspect", "-f", "{{.State.Status}}", c])).await
    {
        Ok(s) => s.trim().to_string(),
        Err(e) => format!("error: {e}"),
    }
}

/// `start`, `stop` o `restart`.
pub async fn action(p: &Profile, verb: &str, dev: bool, dbs: Dbs, tx: &Tx) -> Result<String> {
    if !matches!(verb, "start" | "stop" | "restart") {
        bail!("acción desconocida: {verb} (start, stop o restart)");
    }
    if let Dbs::Only(db) = &dbs {
        if p.is_docker() {
            bail!(
                "correr una sola BD solo se puede en perfiles local: en docker los argumentos \
                 de Odoo van en el compose (command: -d {db} --db-filter=^{db}$)"
            );
        }
        crate::odoo::pg::valid_name(db)?;
    }
    if p.is_docker() {
        let c = container(p)?;
        tx.log(format!("$ docker {verb} {c}"));
        exec::capture(&Spec::new("docker").args([verb, c])).await?;
        return Ok(format!("docker {verb} {c}"));
    }
    let only = match dbs {
        Dbs::All => None,
        Dbs::Only(db) => Some(db),
        Dbs::Same => running_db(p),
    };
    if verb != "start" {
        match local_pid(p) {
            Some(pid) => stop_local(p, pid, tx).await?,
            None if verb == "stop" => bail!("Odoo no está corriendo"),
            None => {}
        }
        if verb == "stop" {
            return Ok("Odoo detenido".into());
        }
    } else if let Some(pid) = local_pid(p) {
        bail!("Odoo ya está corriendo (pid {pid})");
    }
    let pid = start_local(p, dev, only.as_deref(), tx).await?;
    let which = match &only {
        Some(db) => format!(" solo con «{db}»"),
        None => String::new(),
    };
    Ok(format!(
        "Odoo iniciado{which} (pid {pid}), log en {}",
        log_path(p).display()
    ))
}

/// `odoo-bin` con el log a archivo; con `db`, solo esa BD.
pub fn server(p: &Profile, dev: bool, db: Option<&str>) -> Result<Spec> {
    let mut s = modules::odoo(p)?;
    if let Some(db) = db {
        s = s
            .args(["-d", db])
            .arg(format!("--db-filter={}", db_filter(db)));
    }
    if dev {
        s = s.arg("--dev=all");
    }
    Ok(s)
}

async fn start_local(p: &Profile, dev: bool, db: Option<&str>, tx: &Tx) -> Result<i32> {
    let spec = server(p, dev, db)?;
    std::fs::create_dir_all(state_dir())?;
    let log = log_path(p);
    let mut out = std::fs::File::create(&log).with_context(|| format!("{}", log.display()))?;
    let _ = writeln!(out, "$ {}", spec.display());
    tx.log(format!("$ {}", spec.display()));
    let mut child = spec
        .detached()
        .stdout(out.try_clone()?)
        .stderr(out)
        .spawn()
        .with_context(|| format!("no pude lanzar {}", spec.program))?;
    let pid = child.id() as i32;
    std::fs::write(file(p, "pid"), pid.to_string())?;
    match db {
        Some(db) => std::fs::write(file(p, "db"), db)?,
        None => {
            let _ = std::fs::remove_file(file(p, "db"));
        }
    }
    // Alguien tiene que recoger el proceso al morir para que no quede zombi.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    // Si la config está mal, Odoo muere enseguida: mejor decirlo ya.
    tokio::time::sleep(Duration::from_millis(1500)).await;
    if local_pid(p).is_none() {
        let tail = read_local(p, 15).unwrap_or_default().join("\n");
        bail!("Odoo terminó al arrancar:\n{tail}");
    }
    Ok(pid)
}

async fn stop_local(p: &Profile, pid: i32, tx: &Tx) -> Result<()> {
    tx.log(format!("deteniendo Odoo (pid {pid}) con SIGINT"));
    // SAFETY: kill(2) con un pid verificado en /proc.
    unsafe { libc::kill(pid, libc::SIGINT) };
    for _ in 0..50 {
        tokio::time::sleep(Duration::from_millis(200)).await;
        if local_pid(p).is_none() {
            let _ = std::fs::remove_file(file(p, "pid"));
            return Ok(());
        }
    }
    tx.log("⚠ no cerró en 10 s: SIGKILL");
    unsafe { libc::kill(pid, libc::SIGKILL) };
    let _ = std::fs::remove_file(file(p, "pid"));
    Ok(())
}

/// Seguir el log en vivo.
pub fn follow(p: &Profile) -> Result<Spec> {
    if p.is_docker() {
        return Ok(Spec::new("docker").args(["logs", "-f", "--tail", "300", container(p)?]));
    }
    Ok(Spec::new("tail").args(["-n", "300", "-F", &log_path(p).display().to_string()]))
}

fn read_local(p: &Profile, n: usize) -> Result<Vec<String>> {
    let raw = std::fs::read(log_path(p)).context("todavía no hay log de Odoo")?;
    let text = String::from_utf8_lossy(&raw);
    let lines: Vec<String> = text.lines().map(String::from).collect();
    Ok(lines[lines.len().saturating_sub(n)..].to_vec())
}

/// Las últimas `n` líneas del log de Odoo.
pub async fn read(p: &Profile, n: usize) -> Result<Vec<String>> {
    Ok(read_since(p, None, n).await?.0)
}

/// Como `read`, pero solo lo nuevo desde `since` (en local, cuántas líneas ya
/// se leyeron; en docker, lo que acepta `docker logs --since`). Devuelve
/// también el `since` para la próxima vez.
pub async fn read_since(
    p: &Profile,
    since: Option<&str>,
    n: usize,
) -> Result<(Vec<String>, String)> {
    if !p.is_docker() {
        let raw = std::fs::read(log_path(p)).context("todavía no hay log de Odoo")?;
        let text = String::from_utf8_lossy(&raw);
        let all: Vec<&str> = text.lines().collect();
        let from = match since {
            Some(s) => s
                .trim()
                .parse::<usize>()
                .map_err(|_| anyhow::anyhow!("en local, since es un n.º de línea"))?
                // Odoo se reinició: el log empezó de nuevo.
                .min(all.len()),
            None => 0,
        };
        let new = &all[from..];
        let lines = new[new.len().saturating_sub(n)..]
            .iter()
            .map(|l| l.to_string())
            .collect();
        return Ok((lines, all.len().to_string()));
    }
    // La marca se toma antes de leer: así no se pierde nada entre llamadas.
    let next = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    // docker logs escribe la salida de error del contenedor por stderr.
    let (tx, buf) = Tx::detached().capturing();
    let mut spec = Spec::new("docker").args(["logs", "--tail", &n.to_string()]);
    if let Some(s) = since {
        spec = spec.args(["--since", s.trim()]);
    }
    let spec = spec.arg(container(p)?);
    exec::stream(&spec, None, &tx, &CancellationToken::new(), Stop::Kill).await?;
    let lines = buf.lock().map(|l| l.clone()).unwrap_or_default();
    // La primera es el `$ docker logs…` del propio stream.
    Ok((lines.into_iter().skip(1).collect(), next))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Mode;

    #[test]
    fn filter_matches_only_that_db() {
        assert_eq!(db_filter("test_6404"), "^test_6404$");
        assert_eq!(db_filter("pro-12.05"), "^pro\\-12\\.05$");
    }

    #[test]
    fn server_with_one_db() {
        let p = Profile {
            name: "local".into(),
            mode: Mode::Local,
            odoo_bin: "/o/odoo-bin".into(),
            python: "/o/py".into(),
            odoo_conf: "/o/o.conf".into(),
            ..Default::default()
        };
        let all = server(&p, false, None).unwrap().display();
        assert!(!all.contains(" -d "), "{all}");
        let one = server(&p, true, Some("test_6404")).unwrap().display();
        assert!(
            one.contains("-d test_6404 '--db-filter=^test_6404$'")
                || one.contains("-d test_6404 --db-filter=^test_6404$"),
            "{one}"
        );
        assert!(one.ends_with("--dev=all"), "{one}");
    }

    #[tokio::test]
    async fn one_db_is_local_only() {
        let p = Profile {
            mode: Mode::Docker,
            odoo_container: "web".into(),
            ..Default::default()
        };
        let e = action(&p, "start", false, Dbs::Only("x".into()), &Tx::detached())
            .await
            .unwrap_err();
        assert!(e.to_string().contains("perfiles local"), "{e}");
    }
}
