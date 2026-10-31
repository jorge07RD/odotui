//! Backups de una BD en `<backups_dir>`: `.zip` compatible con el gestor de
//! Odoo (`dump.sql`, `filestore/` y `manifest.json`) o `.dump` de
//! `pg_dump -Fc`. Lo que queda a medias se borra.

use std::{
    fs::File,
    io::{BufWriter, Read, Write},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use serde_json::{Map, Value, json};
use tokio_util::sync::CancellationToken;
use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};

use crate::{
    config::Profile,
    exec::{self, Output, Stop},
    jobs::Tx,
    odoo::{
        filestore, pg,
        restore::{human, major},
    },
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Zip,
    Dump,
}

impl Format {
    pub fn parse(s: &str) -> Result<Self> {
        match s {
            "" | "zip" => Ok(Format::Zip),
            "dump" => Ok(Format::Dump),
            other => bail!("formato desconocido: {other} (zip o dump)"),
        }
    }

    fn ext(self) -> &'static str {
        match self {
            Format::Zip => "zip",
            Format::Dump => "dump",
        }
    }
}

/// `<bd>_<AAAA-MM-DD_HHMM>.<ext>`
pub fn file_name(db: &str, format: Format, at: chrono::DateTime<chrono::Local>) -> String {
    format!("{db}_{}.{}", at.format("%Y-%m-%d_%H%M"), format.ext())
}

/// Lo que Odoo pone en `manifest.json` (`odoo.service.db.dump_db_manifest`).
async fn manifest(p: &Profile, db: &str, tx: &Tx) -> Value {
    let pg_version = pg::version(p)
        .await
        .ok()
        .and_then(|v| v.split_whitespace().next().map(String::from))
        .unwrap_or_default();
    let rows = pg::query(
        p,
        db,
        "SELECT name, COALESCE(latest_version, '') FROM ir_module_module WHERE state = 'installed'",
    )
    .await
    .unwrap_or_else(|_| {
        tx.log("⚠ no parece una BD de Odoo: el manifest va sin módulos");
        vec![]
    });
    let mut modules = Map::new();
    let mut base = String::new();
    for r in rows.into_iter().filter(|r| r.len() >= 2) {
        if r[0] == "base" {
            base = r[1].clone();
        }
        modules.insert(r[0].clone(), Value::String(r[1].clone()));
    }
    let version = major(&base);
    let info: Vec<Value> = version
        .split('.')
        .filter_map(|n| n.parse::<u64>().ok())
        .map(Value::from)
        .chain([json!(0), json!("final"), json!(0)])
        .collect();
    json!({
        "odoo_dump": "1",
        "db_name": db,
        "version": version,
        "version_info": info,
        "major_version": version,
        "pg_version": pg_version,
        "modules": modules,
    })
}

/// Archivos de una carpeta, con su ruta relativa (con `/`) y tamaño.
fn walk(dir: &Path) -> Result<Vec<(PathBuf, String, u64)>> {
    let mut out = vec![];
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d)? {
            let e = e?;
            let path = e.path();
            let meta = e.metadata()?;
            if meta.is_dir() {
                stack.push(path);
            } else if meta.is_file() {
                let rel = path
                    .strip_prefix(dir)?
                    .components()
                    .map(|c| c.as_os_str().to_string_lossy())
                    .collect::<Vec<_>>()
                    .join("/");
                out.push((path, rel, meta.len()));
            }
        }
    }
    out.sort_by(|a, b| a.1.cmp(&b.1));
    Ok(out)
}

/// Arma el zip: primero `dump.sql` (como Odoo), luego el manifest y el filestore.
fn write_zip(
    out: &Path,
    dump: &Path,
    manifest: &Value,
    fs: Option<&Path>,
    tx: &Tx,
    cancel: &CancellationToken,
) -> Result<usize> {
    let files = match fs {
        Some(d) => walk(d)?,
        None => vec![],
    };
    let dump_size = std::fs::metadata(dump)?.len();
    let total = dump_size + files.iter().map(|f| f.2).sum::<u64>();
    let mut zip = ZipWriter::new(BufWriter::new(File::create(out)?));
    let opts = |size: u64| {
        SimpleFileOptions::default()
            .compression_method(CompressionMethod::Deflated)
            .large_file(size >= u32::MAX as u64)
    };
    let (mut done, mut last) = (0u64, -1i64);
    let mut chunk = vec![0u8; 256 * 1024];
    let mut copy = |zip: &mut ZipWriter<_>, src: &Path| -> Result<()> {
        let mut f = File::open(src)?;
        loop {
            if cancel.is_cancelled() {
                bail!("cancelado");
            }
            let n = f.read(&mut chunk)?;
            if n == 0 {
                return Ok(());
            }
            zip.write_all(&chunk[..n])?;
            done += n as u64;
            let ratio = done as f64 / total.max(1) as f64;
            if (ratio * 1000.0) as i64 != last {
                last = (ratio * 1000.0) as i64;
                tx.progress(ratio);
            }
        }
    };
    zip.start_file("dump.sql", opts(dump_size))?;
    copy(&mut zip, dump)?;
    zip.start_file("manifest.json", opts(0))?;
    zip.write_all(serde_json::to_string_pretty(manifest)?.as_bytes())?;
    for (path, rel, size) in &files {
        zip.start_file(format!("filestore/{rel}"), opts(*size))?;
        copy(&mut zip, path)?;
    }
    zip.finish()?.flush()?;
    Ok(files.len())
}

/// Backup de `db`; devuelve un resumen con la ruta del archivo.
pub async fn run(
    p: &Profile,
    db: &str,
    format: Format,
    with_filestore: bool,
    tx: &Tx,
    cancel: &CancellationToken,
) -> Result<String> {
    pg::valid_name(db)?;
    if p.backups_dir.is_empty() {
        bail!("el perfil no tiene backups_dir");
    }
    if !pg::exists(p, db).await? {
        bail!("no existe la BD «{db}»");
    }
    let dir = PathBuf::from(&p.backups_dir);
    std::fs::create_dir_all(&dir).with_context(|| format!("no pude crear {}", dir.display()))?;
    // Dos backups en el mismo minuto: `_2`, `_3`…
    let base = file_name(db, format, chrono::Local::now());
    let mut name = base.clone();
    for n in 2.. {
        if !dir.join(&name).exists() {
            break;
        }
        let (stem, ext) = base.rsplit_once('.').unwrap_or((&base, ""));
        name = format!("{stem}_{n}.{ext}");
    }
    let target = dir.join(&name);
    // Todo se escribe con nombres ocultos (no se listan) y se borra si falla.
    let part = dir.join(format!(".{name}.part"));
    let sql = dir.join(format!(".{name}.sql"));
    let scratch = dir.join(format!(".odotui-tmp-backup-{db}"));
    let r = build(
        p,
        db,
        format,
        with_filestore,
        &part,
        &sql,
        &scratch,
        tx,
        cancel,
    )
    .await;
    for tmp in [&sql, &scratch] {
        let _ = std::fs::remove_file(tmp);
        let _ = std::fs::remove_dir_all(tmp);
    }
    match r {
        Ok(detail) => {
            std::fs::rename(&part, &target)?;
            let size = std::fs::metadata(&target).map(|m| m.len()).unwrap_or(0);
            Ok(format!(
                "backup de «{db}» en {} ({}{detail})",
                target.display(),
                human(size)
            ))
        }
        Err(e) => {
            let _ = std::fs::remove_file(&part);
            if cancel.is_cancelled() {
                bail!("cancelado: no quedó ningún backup a medias");
            }
            Err(e)
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn build(
    p: &Profile,
    db: &str,
    format: Format,
    with_filestore: bool,
    part: &Path,
    sql: &Path,
    scratch: &Path,
    tx: &Tx,
    cancel: &CancellationToken,
) -> Result<String> {
    let dump = |fc: bool| {
        let first: &[&str] = if fc {
            &["--no-owner", "-Fc"]
        } else {
            &["--no-owner"]
        };
        pg::client(p, "pg_dump", first).args(["-d", db])
    };
    if format == Format::Dump {
        tx.log(format!("▶ 1/1 pg_dump -Fc de «{db}»"));
        exec::stream_to(
            &dump(true),
            None,
            Output::File(part.to_path_buf()),
            tx,
            cancel,
            Stop::Kill,
        )
        .await?;
        if with_filestore {
            tx.log("⚠ el formato dump no lleva filestore; usa zip si lo necesitas");
        }
        return Ok(String::new());
    }

    let steps = 2 + usize::from(with_filestore);
    tx.log(format!("▶ 1/{steps} pg_dump de «{db}»"));
    exec::stream_to(
        &dump(false),
        None,
        Output::File(sql.to_path_buf()),
        tx,
        cancel,
        Stop::Kill,
    )
    .await?;
    let manifest = manifest(p, db, tx).await;
    let fs = if with_filestore {
        tx.log(format!("▶ 2/{steps} Leyendo el filestore"));
        let fs = filestore::export(p, db, scratch, tx).await?;
        if fs.is_none() {
            tx.log("la BD no tiene filestore");
        }
        fs
    } else {
        None
    };
    if cancel.is_cancelled() {
        bail!("cancelado");
    }
    tx.log(format!("▶ {steps}/{steps} Comprimiendo"));
    tx.progress(0.0);
    let (out, sql, tx2, c) = (
        part.to_path_buf(),
        sql.to_path_buf(),
        tx.clone(),
        cancel.clone(),
    );
    let files = tokio::task::spawn_blocking(move || {
        write_zip(&out, &sql, &manifest, fs.as_deref(), &tx2, &c)
    })
    .await??;
    tx.progress(1.0);
    Ok(if files > 0 {
        format!(", {files} archivo(s) del filestore")
    } else {
        ", sin filestore".into()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::odoo::restore;

    #[test]
    fn names_and_formats() {
        let at = chrono::DateTime::parse_from_rfc3339("2025-10-30T14:05:00-04:00")
            .unwrap()
            .with_timezone(&chrono::Local);
        let n = file_name("acme", Format::Zip, at);
        assert!(n.starts_with("acme_2025-10-") && n.ends_with(".zip"), "{n}");
        assert_eq!(Format::parse("dump").unwrap(), Format::Dump);
        assert!(Format::parse("tar").is_err());
    }

    #[tokio::test]
    async fn zip_round_trips_through_the_listing() {
        let dir = std::env::temp_dir().join(format!("odotui-zip-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("fs/ab")).unwrap();
        std::fs::write(dir.join("dump.sql"), "select 1;\n".repeat(1000)).unwrap();
        std::fs::write(dir.join("fs/ab/abc"), "adjunto").unwrap();
        let backups = dir.join("backups");
        std::fs::create_dir_all(&backups).unwrap();
        let out = backups.join("acme.zip");
        let m = json!({"db_name": "acme", "version": "17.0", "modules": {"base": "17.0.1.3"}});
        let n = write_zip(
            &out,
            &dir.join("dump.sql"),
            &m,
            Some(&dir.join("fs")),
            &Tx::detached(),
            &CancellationToken::new(),
        )
        .unwrap();
        assert_eq!(n, 1);
        let l = restore::list(backups.to_str().unwrap()).await.unwrap();
        assert_eq!(l[0].kind, restore::Kind::Zip);
        assert_eq!(l[0].dump, Some(10_000));
        assert!(l[0].filestore);
        let man = l[0].manifest.as_ref().unwrap();
        assert_eq!((man.version.as_str(), man.modules), ("17.0", 1));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
