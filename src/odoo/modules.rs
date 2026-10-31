//! Módulos de una BD: listado e instalación/actualización con `odoo -i/-u`.

use anyhow::{Result, bail};
use tokio_util::sync::CancellationToken;

use crate::{
    config::Profile,
    exec::{self, Spec, Stop},
    jobs::Tx,
    odoo::pg,
};

#[derive(Debug, Clone, PartialEq)]
pub struct ModuleInfo {
    pub name: String,
    pub state: String,
    pub version: String,
    pub application: bool,
}

pub async fn list(p: &Profile, db: &str) -> Result<Vec<ModuleInfo>> {
    let rows = pg::query(
        p,
        db,
        "SELECT name, state, COALESCE(latest_version, ''), COALESCE(application, false) \
         FROM ir_module_module ORDER BY name",
    )
    .await?;
    Ok(rows
        .into_iter()
        .filter(|r| r.len() >= 4)
        .map(|r| ModuleInfo {
            name: r[0].clone(),
            state: r[1].clone(),
            version: r[2].clone(),
            application: r[3] == "t",
        })
        .collect())
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
