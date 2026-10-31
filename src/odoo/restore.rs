//! Restauración de una carpeta `<backups_dir>/<nombre>/dump.sql` (+ `filestore/`).
//! Port del flujo de `rdo` con borrado previo, progreso real, filestore y
//! neutralización.

use std::{path::PathBuf, time::SystemTime};

use anyhow::{Context, Result, bail};
use tokio_util::sync::CancellationToken;

use crate::{
    config::Profile,
    exec::{self, Stop},
    jobs::Tx,
    odoo::{filestore, neutralize, pg},
};

#[derive(Debug, Clone, PartialEq)]
pub struct Backup {
    pub name: String,
    /// Tamaño de `dump.sql`; `None` = la carpeta no lo tiene.
    pub dump: Option<u64>,
    pub filestore: bool,
    pub modified: Option<SystemTime>,
}

/// Carpetas de backup en el host, la más reciente primero.
pub async fn list(dir: &str) -> Result<Vec<Backup>> {
    if dir.is_empty() {
        bail!("el perfil no tiene backups_dir");
    }
    let mut rd = tokio::fs::read_dir(dir)
        .await
        .with_context(|| format!("no pude leer {dir}"))?;
    let mut out = vec![];
    while let Some(e) = rd.next_entry().await? {
        if !e.file_type().await?.is_dir() {
            continue;
        }
        let path = e.path();
        let dump = tokio::fs::metadata(path.join("dump.sql")).await.ok();
        out.push(Backup {
            name: e.file_name().to_string_lossy().into_owned(),
            modified: dump
                .as_ref()
                .and_then(|m| m.modified().ok())
                .or_else(|| std::fs::metadata(&path).and_then(|m| m.modified()).ok()),
            dump: dump.map(|m| m.len()),
            filestore: path.join("filestore").is_dir(),
        });
    }
    out.sort_by(|a, b| b.modified.cmp(&a.modified).then(a.name.cmp(&b.name)));
    Ok(out)
}

#[derive(Debug, Clone)]
pub struct Request {
    pub profile: Profile,
    pub backup: String,
    pub db: String,
    /// Borrar la BD si ya existe (si no, falla).
    pub overwrite: bool,
    pub filestore: bool,
    pub neutralize: bool,
    pub admin_password: String,
}

pub fn human(bytes: u64) -> String {
    const U: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = bytes as f64;
    let mut i = 0;
    while v >= 1024.0 && i < U.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{bytes} B")
    } else {
        format!("{v:.1} {}", U[i])
    }
}

pub async fn run(req: Request, tx: &Tx, cancel: &CancellationToken) -> Result<String> {
    let p = &req.profile;
    pg::valid_name(&req.db)?;
    let folder = PathBuf::from(&p.backups_dir).join(&req.backup);
    let dump = folder.join("dump.sql");
    let size = tokio::fs::metadata(&dump)
        .await
        .with_context(|| format!("no existe {}", dump.display()))?
        .len();
    let steps = 2 + usize::from(req.filestore) + usize::from(req.neutralize);
    let mut step = 0;
    // Cada paso empieza comprobando si se canceló mientras corría el anterior.
    let mut next = |msg: &str| -> Result<()> {
        if cancel.is_cancelled() {
            bail!("cancelado antes de «{msg}»: la BD «{}» quedó a medias", req.db);
        }
        step += 1;
        tx.log(format!("▶ {step}/{steps} {msg}"));
        Ok(())
    };

    next("Preparando la base de datos")?;
    if pg::exists(p, &req.db).await? {
        if !req.overwrite {
            bail!("la BD «{}» ya existe", req.db);
        }
        tx.log(format!("borrando «{}» (existía)", req.db));
        pg::drop(p, &req.db).await?;
    }
    pg::create(p, &req.db).await?;
    tx.log(format!("BD «{}» creada", req.db));

    next(&format!("Cargando dump.sql ({})", human(size)))?;
    tx.progress(0.0);
    // Sin ON_ERROR_STOP, igual que Odoo: un dump trae errores inocuos
    // (p. ej. COMMENT ON EXTENSION sin ser superusuario).
    let errors = exec::stream(
        // Los resultados (p. ej. los `setval`) no interesan; errores y avisos
        // siguen saliendo por stderr.
        &pg::psql(p, &req.db).args(["-q", "-o", "/dev/null"]),
        Some(dump),
        tx,
        cancel,
        Stop::Kill,
    )
    .await
    .map_err(|e| {
        if cancel.is_cancelled() {
            anyhow::anyhow!("cancelado: la BD «{}» quedó incompleta", req.db)
        } else {
            e
        }
    })?;
    tx.progress(1.0);
    if errors > 0 {
        tx.log(format!("⚠ psql reportó {errors} error(es); revisa el log (filtra con /)"));
    }

    if req.filestore {
        next("Copiando filestore")?;
        let src = folder.join("filestore");
        if src.is_dir() {
            filestore::import(p, &src, &req.db, tx).await?;
        } else {
            tx.log("el backup no trae filestore/");
        }
    }

    if req.neutralize {
        next("Neutralizando")?;
        tokio::select! {
            r = neutralize::run(p, &req.db, &req.admin_password, tx) => r?,
            _ = cancel.cancelled() => {
                bail!("cancelado al neutralizar: «{}» quedó sin neutralizar", req.db)
            }
        }
    }

    Ok(if errors > 0 {
        format!("«{}» restaurada, con {errors} error(es) de psql", req.db)
    } else {
        format!("«{}» restaurada", req.db)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn human_sizes() {
        assert_eq!(human(512), "512 B");
        assert_eq!(human(1536), "1.5 KB");
        assert_eq!(human(3 * 1024 * 1024 * 1024), "3.0 GB");
    }

    #[tokio::test]
    async fn lists_backup_folders() {
        let dir = std::env::temp_dir().join(format!("odotui-bk-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("a/filestore")).unwrap();
        std::fs::write(dir.join("a/dump.sql"), "select 1;").unwrap();
        std::fs::create_dir_all(dir.join("b")).unwrap();
        std::fs::write(dir.join("suelto.sql"), "").unwrap();
        let mut l = list(dir.to_str().unwrap()).await.unwrap();
        l.sort_by(|x, y| x.name.cmp(&y.name));
        assert_eq!(l.len(), 2);
        assert_eq!(l[0].dump, Some(9));
        assert!(l[0].filestore);
        assert_eq!(l[1].dump, None);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
