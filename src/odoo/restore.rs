//! Restauración de backups de `<backups_dir>`: carpetas `dump.sql` (+
//! `filestore/`), `.zip` del gestor de Odoo / Odoo.sh, `.dump` de
//! `pg_dump -Fc` y `.sql` / `.sql.gz`. Port del flujo de `rdo` con borrado
//! previo, progreso real, filestore y neutralización.

use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    time::SystemTime,
};

use anyhow::{Context, Result, bail};
use flate2::read::{DeflateDecoder, GzDecoder};
use serde_json::Value;
use tokio_util::sync::CancellationToken;
use zip::{CompressionMethod, ZipArchive};

use crate::{
    config::Profile,
    exec::{self, Counted, Input, Meter, Stop},
    jobs::Tx,
    odoo::{filestore, neutralize, pg},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// `<carpeta>/dump.sql` (+ `filestore/`).
    Folder,
    /// `.zip` del gestor de Odoo u Odoo.sh: `dump.sql`, `filestore/` y `manifest.json`.
    Zip,
    /// `pg_dump -Fc` (`.dump` / `.backup`), sin filestore.
    PgCustom,
    Sql,
    SqlGz,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Kind::Folder => "carpeta",
            Kind::Zip => "zip",
            Kind::PgCustom => "dump",
            Kind::Sql => "sql",
            Kind::SqlGz => "sql.gz",
        }
    }

    /// Por la extensión; `None` = no es un backup.
    fn of_file(name: &str) -> Option<Kind> {
        let n = name.to_ascii_lowercase();
        Some(if n.ends_with(".zip") {
            Kind::Zip
        } else if n.ends_with(".dump") || n.ends_with(".backup") {
            Kind::PgCustom
        } else if n.ends_with(".sql.gz") {
            Kind::SqlGz
        } else if n.ends_with(".sql") {
            Kind::Sql
        } else {
            return None;
        })
    }
}

/// Lo que dice el `manifest.json` de un zip de Odoo.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Manifest {
    pub version: String,
    pub db_name: String,
    pub pg_version: String,
    pub modules: usize,
}

impl Manifest {
    fn parse(raw: &str) -> Option<Self> {
        let v: Value = serde_json::from_str(raw).ok()?;
        let s = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").to_string();
        Some(Self {
            version: s("version"),
            db_name: s("db_name"),
            pg_version: s("pg_version"),
            modules: v
                .get("modules")
                .and_then(Value::as_object)
                .map_or(0, |m| m.len()),
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Backup {
    pub name: String,
    pub kind: Kind,
    /// Tamaño de `dump.sql` (carpetas) o del archivo; `None` = no trae dump.
    pub dump: Option<u64>,
    pub filestore: bool,
    pub modified: Option<SystemTime>,
    pub manifest: Option<Manifest>,
}

impl Backup {
    /// El nombre sin la extensión (para sugerir el de la BD).
    pub fn stem(&self) -> &str {
        let n = self.name.as_str();
        [".sql.gz", ".zip", ".dump", ".backup", ".sql"]
            .iter()
            .find_map(|ext| {
                n.len()
                    .checked_sub(ext.len())
                    .filter(|&i| n.is_char_boundary(i) && n[i..].eq_ignore_ascii_case(ext))
                    .map(|i| &n[..i])
            })
            .unwrap_or(n)
    }
}

/// «17.0» de «17.0.1.3», «saas~17.2» o «17.0+e».
pub fn major(version: &str) -> String {
    let v = version.trim_start_matches("saas~");
    v.split(['.', '+', '-'])
        .take(2)
        .collect::<Vec<_>>()
        .join(".")
}

/// Contenido de un zip: si trae `dump.sql` (tamaño), si trae filestore y el manifest.
fn inspect_zip(path: &Path) -> Result<(Option<u64>, bool, Option<Manifest>)> {
    let mut z = ZipArchive::new(File::open(path)?)?;
    let dump = z.by_name("dump.sql").ok().map(|f| f.size());
    let fs = z
        .file_names()
        .any(|n| n.is_ok_and(|n| n.starts_with("filestore/")));
    let manifest = z.by_name("manifest.json").ok().and_then(|mut f| {
        let mut raw = String::new();
        f.read_to_string(&mut raw).ok()?;
        Manifest::parse(&raw)
    });
    Ok((dump, fs, manifest))
}

/// Un `.dump` que no empieza con `PGDMP` es SQL plano con otro nombre.
fn is_pg_custom(path: &Path) -> bool {
    let mut magic = [0u8; 5];
    File::open(path)
        .and_then(|mut f| f.read_exact(&mut magic))
        .is_ok_and(|_| &magic == b"PGDMP")
}

fn list_blocking(dir: &str) -> Result<Vec<Backup>> {
    let mut out = vec![];
    for e in std::fs::read_dir(dir).with_context(|| format!("no pude leer {dir}"))? {
        let e = e?;
        let name = e.file_name().to_string_lossy().into_owned();
        // Lo oculto (p. ej. lo temporal de odotui) no se lista.
        if name.starts_with('.') {
            continue;
        }
        let path = e.path();
        let meta = std::fs::metadata(&path)?;
        if meta.is_dir() {
            let dump = std::fs::metadata(path.join("dump.sql")).ok();
            out.push(Backup {
                name,
                kind: Kind::Folder,
                modified: dump
                    .as_ref()
                    .and_then(|m| m.modified().ok())
                    .or_else(|| meta.modified().ok()),
                dump: dump.map(|m| m.len()),
                filestore: path.join("filestore").is_dir(),
                manifest: std::fs::read_to_string(path.join("manifest.json"))
                    .ok()
                    .and_then(|raw| Manifest::parse(&raw)),
            });
            continue;
        }
        let Some(mut kind) = Kind::of_file(&name) else {
            continue;
        };
        let (mut dump, mut filestore, mut manifest) = (Some(meta.len()), false, None);
        match kind {
            // Un zip que no se puede leer se lista igual, sin dump.
            Kind::Zip => (dump, filestore, manifest) = inspect_zip(&path).unwrap_or_default(),
            Kind::PgCustom if !is_pg_custom(&path) => kind = Kind::Sql,
            _ => {}
        }
        out.push(Backup {
            name,
            kind,
            dump,
            filestore,
            modified: meta.modified().ok(),
            manifest,
        });
    }
    out.sort_by(|a, b| b.modified.cmp(&a.modified).then(a.name.cmp(&b.name)));
    Ok(out)
}

/// Backups en el host, el más reciente primero.
pub async fn list(dir: &str) -> Result<Vec<Backup>> {
    if dir.is_empty() {
        bail!("el perfil no tiene backups_dir");
    }
    let dir = dir.to_string();
    tokio::task::spawn_blocking(move || list_blocking(&dir)).await?
}

/// Lector de una entrada del zip que no toma prestado el archivo: se lee
/// directo del disco desde donde empiezan sus datos.
fn zip_entry(path: &Path, name: &str) -> Result<(Box<dyn Read + Send>, u64)> {
    let mut z = ZipArchive::new(File::open(path)?)?;
    let f = z
        .by_name(name)
        .with_context(|| format!("el zip no trae {name}"))?;
    let (size, packed, method) = (f.size(), f.compressed_size(), f.compression());
    let start = f
        .data_start()
        .context("no pude ubicar los datos en el zip")?;
    drop(f);
    let mut file = File::open(path)?;
    file.seek(SeekFrom::Start(start))?;
    let raw = file.take(packed);
    let r: Box<dyn Read + Send> = match method {
        CompressionMethod::Stored => Box::new(raw),
        CompressionMethod::Deflated => Box::new(DeflateDecoder::new(raw)),
        other => bail!("compresión no soportada en el zip: {other}"),
    };
    Ok((r, size))
}

/// Extrae `filestore/` del zip en `dst`; devuelve cuántos archivos.
fn extract_filestore(path: &Path, dst: &Path, cancel: &CancellationToken) -> Result<usize> {
    let mut z = ZipArchive::new(File::open(path)?)?;
    let mut n = 0;
    for i in 0..z.len() {
        if cancel.is_cancelled() {
            bail!("cancelado");
        }
        let mut f = z.by_index(i)?;
        // `enclosed_name` descarta rutas con `..` o absolutas.
        let Some(rel) = f.enclosed_name() else {
            continue;
        };
        let Ok(rel) = rel.strip_prefix("filestore") else {
            continue;
        };
        if rel.as_os_str().is_empty() {
            continue;
        }
        let out = dst.join(rel);
        if f.is_dir() {
            std::fs::create_dir_all(&out)?;
            continue;
        }
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::io::copy(&mut f, &mut File::create(&out)?)?;
        n += 1;
    }
    Ok(n)
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

/// Carpeta temporal, oculta, junto a los backups (mismo disco).
fn scratch(p: &Profile, db: &str) -> PathBuf {
    PathBuf::from(&p.backups_dir).join(format!(".odotui-tmp-{db}"))
}

pub async fn run(req: Request, tx: &Tx, cancel: &CancellationToken) -> Result<String> {
    let p = &req.profile;
    pg::valid_name(&req.db)?;
    let src = PathBuf::from(&p.backups_dir).join(&req.backup);
    let kind = if src.is_dir() {
        Kind::Folder
    } else {
        match Kind::of_file(&req.backup) {
            Some(Kind::PgCustom) if !is_pg_custom(&src) => Kind::Sql,
            Some(k) => k,
            None => bail!("«{}» no es un backup que entienda", req.backup),
        }
    };
    let dump = if kind == Kind::Folder {
        src.join("dump.sql")
    } else {
        src.clone()
    };
    let size = tokio::fs::metadata(&dump)
        .await
        .with_context(|| format!("no existe {}", dump.display()))?
        .len();
    // pg_restore -Fc no trae filestore.
    let filestore = req.filestore && kind != Kind::PgCustom;
    let steps = 2 + usize::from(filestore) + usize::from(req.neutralize);
    let mut step = 0;
    // Cada paso empieza comprobando si se canceló mientras corría el anterior.
    let mut next = |msg: &str| -> Result<()> {
        if cancel.is_cancelled() {
            bail!(
                "cancelado antes de «{msg}»: la BD «{}» quedó a medias",
                req.db
            );
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

    // Los resultados (p. ej. los `setval`) no interesan; errores y avisos
    // siguen saliendo por stderr.
    let psql = || pg::psql(p, &req.db).args(["-q", "-o", "/dev/null"]);
    let (spec, input, what) = match kind {
        Kind::Folder | Kind::Sql => (
            psql(),
            Input::File(dump.clone()),
            format!("Cargando {} ({})", file_name(&dump), human(size)),
        ),
        Kind::SqlGz => {
            // El progreso cuenta los bytes comprimidos: es el total que se conoce.
            let meter = Meter::new(size);
            let gz = GzDecoder::new(Counted::new(File::open(&dump)?, &meter));
            (
                psql(),
                Input::Reader(Box::new(gz), meter),
                format!("Cargando {} ({} comprimido)", req.backup, human(size)),
            )
        }
        Kind::Zip => {
            let (r, total) = zip_entry(&dump, "dump.sql")?;
            let meter = Meter::new(total);
            (
                psql(),
                Input::Reader(Box::new(Counted::new(r, &meter)), meter),
                format!("Cargando dump.sql del zip ({})", human(total)),
            )
        }
        Kind::PgCustom => (
            pg::client(p, "pg_restore", &["--no-owner", "--no-privileges"]).args(["-d", &req.db]),
            Input::File(dump.clone()),
            format!("pg_restore de {} ({})", req.backup, human(size)),
        ),
    };
    next(&what)?;
    tx.progress(0.0);
    // Sin ON_ERROR_STOP, igual que Odoo: un dump trae errores inocuos
    // (p. ej. COMMENT ON EXTENSION sin ser superusuario).
    let (ltx, lines) = tx.capturing();
    let r = exec::stream(&spec, Some(input), &ltx, cancel, Stop::Kill).await;
    // Errores que no indican nada roto (p. ej. un pg_dump más nuevo que el
    // servidor escribe parámetros que este no conoce).
    let harmless = lines
        .lock()
        .map(|l| l.iter().filter(|l| is_harmless(l)).count())
        .unwrap_or(0);
    if harmless > 0 {
        tx.log(format!(
            "({harmless} error(es) inocuo(s): el dump es de un pg_dump más nuevo que el servidor)"
        ));
    }
    let errors = match r {
        Ok(n) => n.saturating_sub(harmless),
        Err(_) if cancel.is_cancelled() => {
            bail!("cancelado: la BD «{}» quedó incompleta", req.db)
        }
        // pg_restore sale con error si hubo alguno, aunque haya cargado todo.
        Err(e) => {
            let lines = lines.lock().map(|l| l.clone()).unwrap_or_default();
            let ignored = lines
                .iter()
                .find(|l| l.contains("errors ignored on restore"));
            match (kind, ignored) {
                (Kind::PgCustom, Some(l)) => l
                    .rsplit(':')
                    .next()
                    .and_then(|n| n.trim().parse().ok())
                    .unwrap_or(1),
                _ => return Err(e),
            }
        }
    };
    tx.progress(1.0);
    if errors > 0 {
        tx.log(format!(
            "⚠ hubo {errors} error(es) al cargar; revisa el log (filtra con /)"
        ));
    }

    if filestore {
        next("Copiando filestore")?;
        match kind {
            Kind::Folder if src.join("filestore").is_dir() => {
                filestore::import(p, &src.join("filestore"), &req.db, tx).await?;
            }
            Kind::Zip => {
                let tmp = scratch(p, &req.db);
                let _ = std::fs::remove_dir_all(&tmp);
                let (zip, dst, c) = (dump.clone(), tmp.clone(), cancel.clone());
                let n =
                    tokio::task::spawn_blocking(move || extract_filestore(&zip, &dst, &c)).await?;
                let r = match n {
                    Ok(0) => {
                        tx.log("el zip no trae filestore/");
                        Ok(())
                    }
                    Ok(n) => {
                        tx.log(format!("{n} archivo(s) del filestore extraídos"));
                        filestore::import(p, &tmp, &req.db, tx).await
                    }
                    Err(e) => Err(e),
                };
                let _ = std::fs::remove_dir_all(&tmp);
                r?;
            }
            _ => tx.log("el backup no trae filestore/"),
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
        format!("«{}» restaurada, con {errors} error(es) al cargar", req.db)
    } else {
        format!("«{}» restaurada", req.db)
    })
}

fn is_harmless(line: &str) -> bool {
    line.contains("ERROR")
        && line.contains("unrecognized configuration parameter")
        && line.contains("transaction_timeout")
}

fn file_name(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn majors_and_manifest() {
        assert_eq!(major("17.0.1.3"), "17.0");
        assert_eq!(major("saas~17.2"), "17.2");
        assert_eq!(major("16.0+e"), "16.0");
        let m = Manifest::parse(
            r#"{"odoo_dump": "1", "db_name": "acme", "version": "17.0", "pg_version": "16.4",
                "modules": {"base": "17.0.1.3", "sale": "17.0.1.2"}}"#,
        )
        .unwrap();
        assert_eq!((m.db_name.as_str(), m.modules), ("acme", 2));
    }

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
        std::fs::write(dir.join("suelto.sql"), "select 2;").unwrap();
        std::fs::write(dir.join("plano.dump"), "select 3;").unwrap();
        std::fs::write(dir.join("notas.txt"), "").unwrap();
        std::fs::create_dir_all(dir.join(".odotui-tmp-x")).unwrap();
        let mut l = list(dir.to_str().unwrap()).await.unwrap();
        l.sort_by(|x, y| x.name.cmp(&y.name));
        let names: Vec<_> = l.iter().map(|b| (b.name.as_str(), b.kind)).collect();
        assert_eq!(
            names,
            [
                ("a", Kind::Folder),
                ("b", Kind::Folder),
                ("plano.dump", Kind::Sql),
                ("suelto.sql", Kind::Sql),
            ]
        );
        assert_eq!(l[0].dump, Some(9));
        assert!(l[0].filestore);
        assert_eq!(l[1].dump, None);
        assert_eq!(l[3].stem(), "suelto");
        std::fs::remove_dir_all(dir).unwrap();
    }
}
