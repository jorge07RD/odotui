//! El servidor de Odoo: contenedor (docker) o proceso `odoo-bin` (local).

use anyhow::{Result, bail};

use crate::{
    config::Profile,
    exec::{self, Spec},
    odoo::modules,
};

fn container(p: &Profile) -> Result<&str> {
    if p.odoo_container.is_empty() {
        bail!("el perfil no tiene contenedor de Odoo");
    }
    Ok(&p.odoo_container)
}

/// `running`, `exited`, … o el error.
pub async fn docker_status(p: &Profile) -> String {
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
pub async fn docker(p: &Profile, verb: &str) -> Result<()> {
    let c = container(p)?;
    exec::capture(&Spec::new("docker").args([verb, c])).await?;
    Ok(())
}

pub fn logs(p: &Profile) -> Result<Spec> {
    Ok(Spec::new("docker").args(["logs", "-f", "--tail", "300", container(p)?]))
}

/// `odoo-bin` en primer plano para el modo local.
pub fn server(p: &Profile, db: Option<&str>, dev: bool) -> Result<Spec> {
    let mut s = modules::odoo(p)?;
    if let Some(db) = db {
        s = s.args(["-d", db]);
    }
    if dev {
        s = s.arg("--dev=all");
    }
    Ok(s)
}
