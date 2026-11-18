//! Qué Odoo están corriendo en esta máquina, de cualquier perfil o lanzados
//! fuera de odotui (p. ej. con un script propio), y los contenedores de los
//! perfiles docker.

use std::{collections::HashMap, path::Path};

use crate::{
    config::Profile,
    exec::{self, Spec},
    odoo::instance,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Running {
    /// Pid de `odoo-bin` (local) o `None` (docker).
    pub pid: Option<i32>,
    /// Contenedor (docker).
    pub container: Option<String>,
    /// El perfil al que pertenece, si se sabe.
    pub profile: Option<String>,
    /// Lanzado por odotui (tiene su pid guardado); si no, «externo».
    pub managed: bool,
    pub port: u16,
    /// `-d`: la BD con la que corre, si es una sola.
    pub db: Option<String>,
    pub dev: bool,
}

impl Running {
    pub fn url(&self) -> String {
        format!("http://localhost:{}", self.port)
    }

    /// «local-17», «externo» o el contenedor.
    pub fn who(&self) -> String {
        match (&self.profile, self.managed, &self.container) {
            (Some(p), true, _) => p.clone(),
            (_, _, Some(c)) => c.clone(),
            (Some(p), false, None) => format!("{p} (externo)"),
            (None, _, None) => "externo".into(),
        }
    }
}

/// Valor de una opción `--x=v`, `--x v` o `-x v`.
fn opt<'a>(args: &'a [String], names: &[&str]) -> Option<&'a str> {
    for (i, a) in args.iter().enumerate() {
        for n in names {
            if a == n {
                return args.get(i + 1).map(String::as_str);
            }
            if let Some(v) = a.strip_prefix(&format!("{n}=")) {
                return Some(v);
            }
        }
    }
    None
}

/// `clave = valor` de un `odoo.conf`.
fn conf_values(path: &str) -> HashMap<String, String> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| {
            let (k, v) = l.split_once('=')?;
            Some((k.trim().to_string(), v.trim().to_string()))
        })
        .collect()
}

/// Si la línea de comandos es un servidor de Odoo: (ruta de odoo-bin, puerto,
/// BD, --dev). Los subcomandos (`shell`, `scaffold`…) y las tareas con
/// `--stop-after-init` no cuentan.
pub fn parse_server(args: &[String]) -> Option<(String, u16, Option<String>, bool)> {
    let i = args.iter().position(|a| {
        Path::new(a)
            .file_name()
            .is_some_and(|n| n == "odoo-bin" || n == "odoo-bin.py")
    })?;
    let rest = &args[i + 1..];
    if let Some(sub) = rest.first()
        && !sub.starts_with('-')
        && sub != "server"
    {
        return None;
    }
    if rest.iter().any(|a| a == "--stop-after-init") {
        return None;
    }
    let conf = opt(rest, &["-c", "--config"])
        .map(conf_values)
        .unwrap_or_default();
    let port = opt(rest, &["-p", "--http-port", "--xmlrpc-port"])
        .map(str::to_string)
        .or_else(|| conf.get("http_port").cloned())
        .or_else(|| conf.get("xmlrpc_port").cloned())
        .and_then(|p| p.parse().ok())
        .unwrap_or(8069);
    let db = opt(rest, &["-d", "--database"])
        .map(str::to_string)
        .or_else(|| conf.get("db_name").cloned())
        .filter(|d| !d.is_empty() && d != "False" && !d.contains(','));
    let dev = rest.iter().any(|a| a.starts_with("--dev"));
    Some((args[i].clone(), port, db, dev))
}

fn cmdline(pid: i32) -> Option<Vec<String>> {
    let raw = std::fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    let args: Vec<String> = raw
        .split(|b| *b == 0)
        .filter(|a| !a.is_empty())
        .map(|a| String::from_utf8_lossy(a).into_owned())
        .collect();
    (!args.is_empty()).then_some(args)
}

/// Padre y si es zombi, de `/proc/<pid>/stat`.
fn stat(pid: i32) -> Option<(i32, bool)> {
    let s = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let rest = s.rsplit_once(')')?.1;
    let mut f = rest.split_whitespace();
    let zombie = f.next()? == "Z";
    Some((f.next()?.parse().ok()?, zombie))
}

/// Los `odoo-bin` servidores de esta máquina.
pub fn scan_local(profiles: &[Profile]) -> Vec<Running> {
    let managed: HashMap<i32, &Profile> = profiles
        .iter()
        .filter(|p| !p.is_docker())
        .filter_map(|p| Some((instance::local_pid(p)?, p)))
        .collect();
    let Ok(dir) = std::fs::read_dir("/proc") else {
        return vec![];
    };
    let mut out = vec![];
    for e in dir.flatten() {
        let Some(pid) = e.file_name().to_str().and_then(|s| s.parse::<i32>().ok()) else {
            continue;
        };
        let Some(args) = cmdline(pid) else { continue };
        let Some((bin, port, db, dev)) = parse_server(&args) else {
            continue;
        };
        let Some((ppid, zombie)) = stat(pid) else {
            continue;
        };
        // Con workers, los hijos repiten la línea de comandos del padre.
        if zombie || cmdline(ppid).is_some_and(|p| p == args) {
            continue;
        }
        let (profile, is_managed) = match managed.get(&pid) {
            Some(p) => (Some(p.name.clone()), true),
            None => (
                profiles
                    .iter()
                    .find(|p| !p.is_docker() && !p.odoo_bin.is_empty() && p.odoo_bin == bin)
                    .map(|p| p.name.clone()),
                false,
            ),
        };
        out.push(Running {
            pid: Some(pid),
            container: None,
            profile,
            managed: is_managed,
            port,
            db,
            dev,
        });
    }
    out.sort_by_key(|r| (!r.managed, r.port, r.pid));
    out
}

fn port_of(url: &str) -> u16 {
    url.rsplit(':')
        .next()
        .and_then(|p| p.trim_end_matches('/').parse().ok())
        .unwrap_or(8069)
}

/// Locales y contenedores de los perfiles docker que estén corriendo.
pub async fn scan(profiles: &[Profile]) -> Vec<Running> {
    let local = {
        let profiles = profiles.to_vec();
        tokio::task::spawn_blocking(move || scan_local(&profiles))
            .await
            .unwrap_or_default()
    };
    let mut out = local;
    let mut seen = vec![];
    for p in profiles.iter().filter(|p| p.is_docker()) {
        let c = &p.odoo_container;
        if c.is_empty() || seen.contains(c) {
            continue;
        }
        seen.push(c.clone());
        let spec = Spec::new("docker").args(["inspect", "-f", "{{.State.Running}}", c]);
        if exec::capture(&spec).await.is_ok_and(|s| s.trim() == "true") {
            out.push(Running {
                pid: None,
                container: Some(c.clone()),
                profile: Some(p.name.clone()),
                managed: true,
                port: port_of(p.url()),
                db: None,
                dev: false,
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &str) -> Vec<String> {
        s.split_whitespace().map(String::from).collect()
    }

    #[test]
    fn servers_and_not_tasks() {
        let (bin, port, db, dev) = parse_server(&args(
            "/v/bin/python /o/odoo-bin -c /nope.conf -d acme --db-filter=^acme$ --dev=all",
        ))
        .unwrap();
        assert_eq!(
            (bin.as_str(), port, db.as_deref(), dev),
            ("/o/odoo-bin", 8069, Some("acme"), true)
        );
        let (_, port, db, _) =
            parse_server(&args("python3 odoo-bin --http-port=8070 --database x")).unwrap();
        assert_eq!((port, db.as_deref()), (8070, Some("x")));
        assert!(parse_server(&args("python odoo-bin shell -d acme")).is_none());
        assert!(parse_server(&args("python odoo-bin -d acme -u sale --stop-after-init")).is_none());
        assert!(parse_server(&args("vim odoo-bin.conf")).is_none());
        assert!(parse_server(&args("python odoo-bin server -p 9000")).is_some());
    }

    #[test]
    fn port_and_db_from_the_conf() {
        let dir = std::env::temp_dir().join(format!("odotui-run-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let conf = dir.join("o.conf");
        std::fs::write(&conf, "[options]\nhttp_port = 8071\ndb_name = False\n").unwrap();
        let line = format!("python odoo-bin -c {}", conf.display());
        let (_, port, db, _) = parse_server(&args(&line)).unwrap();
        assert_eq!((port, db), (8071, None));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn who_is_it() {
        let r = Running {
            pid: Some(1),
            container: None,
            profile: Some("local-17".into()),
            managed: true,
            port: 8069,
            db: None,
            dev: false,
        };
        assert_eq!(r.who(), "local-17");
        let ext = Running {
            managed: false,
            ..r.clone()
        };
        assert_eq!(ext.who(), "local-17 (externo)");
        assert_eq!(
            Running {
                profile: None,
                ..ext
            }
            .who(),
            "externo"
        );
    }
}
