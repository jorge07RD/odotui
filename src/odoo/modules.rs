//! Módulos de una BD: listado e instalación/actualización con `odoo -i/-u`.

use anyhow::{Result, bail};
use tokio_util::sync::CancellationToken;

use crate::{
    config::Profile,
    exec::{self, Spec, Stop},
    jobs::Tx,
    odoo::{
        addons::{self, Addon},
        dev, pg, restore,
    },
};

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ModuleInfo {
    pub name: String,
    pub state: String,
    pub version: String,
    pub application: bool,
    /// Versión del `__manifest__.py` en el disco (ya con la serie), si es un
    /// addon propio.
    pub disk: Option<String>,
}

impl ModuleInfo {
    /// Instalado, pero el código en el disco es de otra versión: falta `-u`.
    pub fn changed(&self) -> bool {
        self.state == "installed"
            && self
                .disk
                .as_deref()
                .is_some_and(|d| !self.version.is_empty() && d != self.version)
    }
}

pub async fn list(p: &Profile, db: &str) -> Result<Vec<ModuleInfo>> {
    let rows = pg::query(
        p,
        db,
        "SELECT name, state, COALESCE(latest_version, ''), COALESCE(application, false) \
         FROM ir_module_module ORDER BY name",
    )
    .await?;
    let mut mods: Vec<ModuleInfo> = rows
        .into_iter()
        .filter(|r| r.len() >= 4)
        .map(|r| ModuleInfo {
            name: r[0].clone(),
            state: r[1].clone(),
            version: r[2].clone(),
            application: r[3] == "t",
            disk: None,
        })
        .collect();
    let dir = p.addons_host().to_string();
    let found = tokio::task::spawn_blocking(move || addons::scan(&dir)).await?;
    merge(&mut mods, &found);
    Ok(mods)
}

/// Junta la BD con los addons del disco. Los que la BD no conoce todavía
/// (sin «actualizar lista») aparecen como no instalados: `-i` los encuentra.
pub fn merge(mods: &mut Vec<ModuleInfo>, found: &[Addon]) {
    let serie = mods
        .iter()
        .find(|m| m.name == "base")
        .map(|m| restore::major(&m.version))
        .unwrap_or_default();
    for a in found {
        let disk = addons::adapt(a.version(), &serie);
        match mods.iter_mut().find(|m| m.name == a.name) {
            Some(m) => m.disk = Some(disk),
            None => mods.push(ModuleInfo {
                name: a.name.clone(),
                state: "uninstalled".into(),
                application: a.manifest.get("application") == Some(&serde_json::Value::Bool(true)),
                disk: Some(disk),
                ..Default::default()
            }),
        }
    }
    mods.sort_by(|a, b| a.name.cmp(&b.name));
}

/// Desinstala con el ORM (no hay opción de línea de comandos para esto).
pub async fn uninstall(
    p: &Profile,
    db: &str,
    names: &[String],
    tx: &Tx,
    cancel: &CancellationToken,
) -> Result<String> {
    if names.is_empty() {
        bail!("no hay módulos marcados");
    }
    let list = names
        .iter()
        .map(|n| format!("{n:?}"))
        .collect::<Vec<_>>()
        .join(", ");
    let code = format!(
        "mods = env['ir.module.module'].search([('name', 'in', [{list}]), ('state', 'in', ['installed', 'to upgrade'])])\n\
         print('ODOTUI', ','.join(mods.mapped('name')) or '-')\n\
         mods.button_immediate_uninstall()\n\
         env.cr.commit()\n"
    );
    let (tx, lines) = tx.capturing();
    dev::shell(p, db, &code, &tx, cancel).await?;
    let lines = lines.lock().map(|l| l.clone()).unwrap_or_default();
    let done = lines
        .iter()
        .find_map(|l| l.strip_prefix("ODOTUI "))
        .unwrap_or("-")
        .to_string();
    if lines.iter().any(|l| l.starts_with("Traceback")) {
        bail!("Odoo falló al desinstalar {done}");
    }
    if done == "-" {
        bail!("ninguno de {} estaba instalado", names.join(","));
    }
    Ok(format!("{done} desinstalado(s)"))
}

/// El ejecutable de Odoo del perfil, ya apuntando a su Postgres.
pub fn odoo(p: &Profile) -> Result<Spec> {
    odoo_sub(p, None)
}

/// Igual, con un subcomando (`shell`, `scaffold`…), que va antes de las opciones.
pub fn odoo_sub(p: &Profile, sub: Option<&str>) -> Result<Spec> {
    if p.is_docker() {
        if p.odoo_container.is_empty() {
            bail!("el perfil no tiene contenedor de Odoo");
        }
        let mut s = Spec::new("odoo").inside(&p.odoo_container).args(sub);
        if sub == Some("scaffold") {
            return Ok(s);
        }
        // `docker exec` se salta el entrypoint de la imagen, que es quien
        // pasa los datos de la BD: hay que darlos aquí.
        let host = if p.pg_host.is_empty() {
            "db"
        } else {
            &p.pg_host
        };
        s = s.args(["--db_host", host, "--db_port", &p.pg_port.to_string()]);
        if !p.pg_user.is_empty() {
            s = s.args(["--db_user", &p.pg_user]);
        }
        if !p.pg_password.is_empty() {
            s = s.args(["--db_password", &p.pg_password]);
        }
        return Ok(s);
    }
    if p.odoo_bin.is_empty() {
        bail!("el perfil no tiene odoo_bin");
    }
    let python = if p.python.is_empty() {
        "python3"
    } else {
        &p.python
    };
    let mut s = Spec::new(python).arg(&p.odoo_bin).args(sub);
    if sub == Some("scaffold") {
        return Ok(s);
    }
    if !p.odoo_conf.is_empty() {
        // El .conf manda; solo se añade lo que el perfil no puede saber.
        s = s.args(["-c", &p.odoo_conf]);
    } else {
        if !p.pg_host.is_empty() {
            s = s.args(["--db_host", &p.pg_host]);
        }
        s = s.args(["--db_port", &p.pg_port.to_string()]);
        if !p.pg_user.is_empty() {
            s = s.args(["--db_user", &p.pg_user]);
        }
        s = s.env("PGPASSWORD", &p.pg_password);
    }
    Ok(s)
}

pub async fn run(
    p: &Profile,
    db: &str,
    install: bool,
    names: &[String],
    tx: &Tx,
    cancel: &CancellationToken,
) -> Result<String> {
    if names.is_empty() {
        bail!("no hay módulos marcados");
    }
    let list = names.join(",");
    let spec = odoo(p)?.args([
        "-d",
        db,
        if install { "-i" } else { "-u" },
        &list,
        "--stop-after-init",
        "--no-http",
    ]);
    let errors = exec::stream(&spec, None, tx, cancel, Stop::Interrupt).await?;
    let verb = if install {
        "instalado(s)"
    } else {
        "actualizado(s)"
    };
    Ok(if errors > 0 {
        format!("{list} {verb} con {errors} error(es) en el log")
    } else {
        format!("{list} {verb}")
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Mode;

    #[test]
    fn merge_marks_changed_and_new_addons() {
        let m = |name: &str, state: &str, version: &str| ModuleInfo {
            name: name.into(),
            state: state.into(),
            version: version.into(),
            ..Default::default()
        };
        let mut mods = vec![
            m("base", "installed", "17.0.1.3"),
            m("acme", "installed", "17.0.1.0"),
            m("viejo", "installed", "17.0.2.0"),
        ];
        let addon = |name: &str, version: &str| Addon {
            name: name.into(),
            path: name.into(),
            manifest: serde_json::json!({"version": version}),
        };
        merge(
            &mut mods,
            &[
                addon("acme", "1.1"),
                addon("viejo", "17.0.2.0"),
                addon("nuevo", "1.0"),
            ],
        );
        let names: Vec<_> = mods
            .iter()
            .map(|m| (m.name.as_str(), m.changed()))
            .collect();
        assert_eq!(
            names,
            [
                ("acme", true),
                ("base", false),
                ("nuevo", false),
                ("viejo", false)
            ]
        );
        assert_eq!(mods[0].disk.as_deref(), Some("17.0.1.1"));
        assert_eq!(mods[2].state, "uninstalled");
    }

    #[test]
    fn docker_odoo_gets_db_args() {
        let p = Profile {
            mode: Mode::Docker,
            odoo_container: "web".into(),
            pg_port: 5432,
            pg_user: "odoo".into(),
            ..Default::default()
        };
        assert_eq!(
            odoo(&p).unwrap().display(),
            "docker exec web odoo --db_host db --db_port 5432 --db_user odoo"
        );
    }

    #[test]
    fn local_odoo_uses_conf() {
        let p = Profile {
            mode: Mode::Local,
            odoo_bin: "/o/odoo-bin".into(),
            odoo_conf: "/o/o.conf".into(),
            pg_port: 5432,
            ..Default::default()
        };
        assert_eq!(
            odoo(&p).unwrap().display(),
            "python3 /o/odoo-bin -c /o/o.conf"
        );
        assert!(odoo(&Profile::default()).is_err());
    }
}
