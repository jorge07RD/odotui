//! El filestore de cada BD (`<filestore_dir>/<bd>`): en el host o en el
//! contenedor de Odoo.

use std::path::Path;

use anyhow::{Result, bail};

use crate::{config::Profile, exec, exec::Spec, jobs::Tx};

/// Ruta del filestore de `db`, o `None` si el perfil no permite tocarlo.
fn path(p: &Profile, db: &str, tx: &Tx) -> Option<String> {
    let base = p.filestore();
    if base.is_empty() || db.is_empty() {
        tx.log("⚠ sin filestore_dir: se omite el filestore");
        return None;
    }
    if p.is_docker() && p.odoo_container.is_empty() {
        tx.log("⚠ sin contenedor de Odoo: se omite el filestore");
        return None;
    }
    Some(format!("{}/{db}", base.trim_end_matches('/')))
}

fn sh(p: &Profile, program: &str) -> Spec {
    let s = Spec::new(program);
    if p.is_docker() {
        s.inside(&p.odoo_container).root()
    } else {
        s
    }
}

async fn run(spec: Spec, tx: &Tx) -> Result<()> {
    tx.log(format!("$ {}", spec.display()));
    exec::capture(&spec).await?;
    Ok(())
}

/// En docker los archivos deben ser del usuario `odoo` del contenedor.
async fn chown(p: &Profile, path: &str, tx: &Tx) {
    if p.is_docker()
        && let Err(e) = run(sh(p, "chown").args(["-R", "odoo:odoo", path]), tx).await
    {
        tx.log(format!("⚠ chown: {e}"));
    }
}

pub async fn remove(p: &Profile, db: &str, tx: &Tx) -> Result<()> {
    let Some(path) = path(p, db, tx) else {
        return Ok(());
    };
    run(sh(p, "rm").args(["-rf", &path]), tx).await
}

pub async fn copy(p: &Profile, from: &str, to: &str, tx: &Tx) -> Result<()> {
    let (Some(src), Some(dst)) = (path(p, from, tx), path(p, to, tx)) else {
        return Ok(());
    };
    // Una BD sin adjuntos no tiene carpeta: no es un error.
    if exec::capture(&sh(p, "test").args(["-d", &src]))
        .await
        .is_err()
    {
        tx.log("sin filestore que copiar");
        return Ok(());
    }
    run(sh(p, "rm").args(["-rf", &dst]), tx).await?;
    run(sh(p, "cp").args(["-a", &src, &dst]), tx).await?;
    chown(p, &dst, tx).await;
    Ok(())
}

pub async fn rename(p: &Profile, from: &str, to: &str, tx: &Tx) -> Result<()> {
    let (Some(src), Some(dst)) = (path(p, from, tx), path(p, to, tx)) else {
        return Ok(());
    };
    if exec::capture(&sh(p, "test").args(["-d", &src]))
        .await
        .is_err()
    {
        return Ok(());
    }
    run(sh(p, "mv").args(["-T", &src, &dst]), tx).await
}

/// El filestore de `db` como carpeta del host, para leerlo (p. ej. al hacer un
/// backup). En local es la carpeta misma; en docker se copia con `docker cp` a
/// `scratch`, que el llamador borra al terminar. `None` = no tiene.
pub async fn export(
    p: &Profile,
    db: &str,
    scratch: &Path,
    tx: &Tx,
) -> Result<Option<std::path::PathBuf>> {
    let Some(src) = path(p, db, tx) else {
        return Ok(None);
    };
    if !p.is_docker() {
        let src = std::path::PathBuf::from(src);
        return Ok(src.is_dir().then_some(src));
    }
    if exec::capture(&sh(p, "test").args(["-d", &src]))
        .await
        .is_err()
    {
        return Ok(None);
    }
    let _ = std::fs::remove_dir_all(scratch);
    run(
        Spec::new("docker").args([
            "cp",
            &format!("{}:{src}", p.odoo_container),
            &scratch.display().to_string(),
        ]),
        tx,
    )
    .await?;
    Ok(Some(scratch.to_path_buf()))
}

/// Copia `<backup>/filestore` (en el host) como filestore de `db`.
pub async fn import(p: &Profile, src: &Path, db: &str, tx: &Tx) -> Result<()> {
    let Some(dst) = path(p, db, tx) else {
        return Ok(());
    };
    if !src.is_dir() {
        bail!("{} no es una carpeta", src.display());
    }
    let base = p.filestore();
    run(sh(p, "rm").args(["-rf", &dst]), tx).await?;
    run(sh(p, "mkdir").args(["-p", &base]), tx).await?;
    let src = src.display().to_string();
    if p.is_docker() {
        run(
            Spec::new("docker").args(["cp", &src, &format!("{}:{dst}", p.odoo_container)]),
            tx,
        )
        .await?;
    } else {
        run(Spec::new("cp").args(["-a", &src, &dst]), tx).await?;
    }
    chown(p, &dst, tx).await;
    Ok(())
}
