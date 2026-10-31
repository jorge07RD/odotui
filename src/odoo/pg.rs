//! Operaciones de Postgres con `psql` (en el host o en el contenedor de la BD).

use anyhow::{Result, bail};
use tokio::task::JoinSet;

use crate::{config::Profile, exec, exec::Spec};

#[derive(Debug, Clone, PartialEq)]
pub struct DbInfo {
    pub name: String,
    pub size: u64,
    pub owner: String,
    /// Versión del módulo `base`; `None` = no es una BD de Odoo.
    pub version: Option<String>,
}

/// Nombres que Odoo acepta y que se pueden citar sin sorpresas.
pub fn valid_name(name: &str) -> Result<()> {
    if name.is_empty() {
        bail!("el nombre está vacío");
    }
    if name.len() > 63 {
        bail!("el nombre pasa de 63 caracteres");
    }
    if name.starts_with(['-', '.']) {
        bail!("el nombre no puede empezar con «-» ni «.»");
    }
    if let Some(c) = name
        .chars()
        .find(|c| !(c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.')))
    {
        bail!("carácter no permitido: «{c}» (usa letras, números, _ - .)");
    }
    Ok(())
}

pub fn ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

pub fn literal(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// `psql` conectado a `db`, con la contraseña por entorno.
pub fn psql(p: &Profile, db: &str) -> Spec {
    client(p, "psql", &["-X"]).args(["-d", db])
}

/// Un cliente de Postgres (`psql`, `pg_dump`, `pg_restore`) con los datos de
/// conexión del perfil; en docker corre en el contenedor de la BD.
pub fn client(p: &Profile, program: &str, first: &[&str]) -> Spec {
    let in_container = p.is_docker() && !p.pg_container.is_empty();
    let host = if in_container {
        "127.0.0.1"
    } else {
        &p.pg_host
    };
    let mut s = Spec::new(program).args(first.iter().copied());
    if !host.is_empty() {
        s = s.args(["-h", host]);
    }
    s = s.args(["-p", &p.pg_port.to_string()]);
    if !p.pg_user.is_empty() {
        s = s.args(["-U", &p.pg_user]);
    }
    s.env("PGPASSWORD", &p.pg_password)
        .inside(if in_container { &p.pg_container } else { "" })
}

const SEP: &str = "\u{1f}";

/// Filas de una consulta, columnas separadas.
pub async fn query(p: &Profile, db: &str, sql: &str) -> Result<Vec<Vec<String>>> {
    let out = exec::capture(&psql(p, db).args(["-A", "-t", "-q", "-F", SEP, "-c", sql])).await?;
    Ok(out
        .lines()
        .filter(|l| !l.is_empty())
        .map(|l| l.split(SEP).map(String::from).collect())
        .collect())
}

pub async fn execute(p: &Profile, db: &str, sql: &str) -> Result<()> {
    exec::capture(&psql(p, db).args(["-q", "-v", "ON_ERROR_STOP=1", "-c", sql])).await?;
    Ok(())
}

pub async fn version(p: &Profile) -> Result<String> {
    let rows = query(p, "postgres", "SHOW server_version").await?;
    Ok(rows
        .first()
        .and_then(|r| r.first())
        .cloned()
        .unwrap_or_default())
}

pub async fn list(p: &Profile) -> Result<Vec<DbInfo>> {
    let rows = query(
        p,
        "postgres",
        "SELECT datname, pg_database_size(datname), pg_get_userbyid(datdba) \
         FROM pg_database WHERE NOT datistemplate AND datallowconn ORDER BY datname",
    )
    .await?;
    let mut dbs: Vec<DbInfo> = rows
        .into_iter()
        .filter(|r| r.len() >= 3)
        .map(|r| DbInfo {
            size: r[1].parse().unwrap_or(0),
            owner: r[2].clone(),
            name: r[0].clone(),
            version: None,
        })
        .collect();

    // Una consulta por BD para saber si es de Odoo; en paralelo.
    let mut set = JoinSet::new();
    for (i, db) in dbs.iter().enumerate() {
        if db.name == "postgres" {
            continue;
        }
        let (p, name) = (p.clone(), db.name.clone());
        set.spawn(async move {
            let v = query(
                &p,
                &name,
                "SELECT latest_version FROM ir_module_module WHERE name = 'base'",
            )
            .await
            .ok()
            .and_then(|rows| rows.into_iter().next())
            .and_then(|r| r.into_iter().next());
            (i, v)
        });
    }
    while let Some(Ok((i, v))) = set.join_next().await {
        dbs[i].version = v;
    }
    Ok(dbs)
}

pub async fn exists(p: &Profile, db: &str) -> Result<bool> {
    let rows = query(
        p,
        "postgres",
        &format!("SELECT 1 FROM pg_database WHERE datname = {}", literal(db)),
    )
    .await?;
    Ok(!rows.is_empty())
}

pub async fn create(p: &Profile, db: &str) -> Result<()> {
    let mut sql = format!(
        "CREATE DATABASE {} ENCODING 'UTF8' TEMPLATE template0",
        ident(db)
    );
    if !p.pg_user.is_empty() {
        sql += &format!(" OWNER {}", ident(&p.pg_user));
    }
    execute(p, "postgres", &sql).await
}

/// Cierra las conexiones abiertas (Odoo deja varias) para poder copiar o renombrar.
pub async fn disconnect(p: &Profile, db: &str) -> Result<()> {
    query(
        p,
        "postgres",
        &format!(
            "SELECT pg_terminate_backend(pid) FROM pg_stat_activity \
             WHERE datname = {} AND pid <> pg_backend_pid()",
            literal(db)
        ),
    )
    .await?;
    Ok(())
}

pub async fn drop(p: &Profile, db: &str) -> Result<()> {
    disconnect(p, db).await?;
    execute(
        p,
        "postgres",
        &format!("DROP DATABASE IF EXISTS {}", ident(db)),
    )
    .await
}

pub async fn duplicate(p: &Profile, from: &str, to: &str) -> Result<()> {
    disconnect(p, from).await?;
    let mut sql = format!(
        "CREATE DATABASE {} WITH TEMPLATE {}",
        ident(to),
        ident(from)
    );
    if !p.pg_user.is_empty() {
        sql += &format!(" OWNER {}", ident(&p.pg_user));
    }
    execute(p, "postgres", &sql).await?;
    // Como hace Odoo al duplicar: la copia no comparte el uuid de la original.
    let _ = execute(
        p,
        to,
        "UPDATE ir_config_parameter SET value = uuid_in(md5(random()::text || clock_timestamp()::text)::cstring)::text \
         WHERE key = 'database.uuid'",
    )
    .await;
    Ok(())
}

pub async fn rename(p: &Profile, from: &str, to: &str) -> Result<()> {
    disconnect(p, from).await?;
    execute(
        p,
        "postgres",
        &format!("ALTER DATABASE {} RENAME TO {}", ident(from), ident(to)),
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Mode;

    #[test]
    fn names() {
        assert!(valid_name("prod_2025-04.copia").is_ok());
        assert!(valid_name("").is_err());
        assert!(valid_name("a b").is_err());
        assert!(valid_name("x\";DROP").is_err());
        assert!(valid_name("-x").is_err());
        assert_eq!(ident("a\"b"), "\"a\"\"b\"");
        assert_eq!(literal("o'k"), "'o''k'");
    }

    #[test]
    fn psql_runs_in_db_container() {
        let p = Profile {
            mode: Mode::Docker,
            pg_host: "db".into(),
            pg_port: 5432,
            pg_user: "odoo".into(),
            pg_container: "pg".into(),
            ..Default::default()
        };
        assert_eq!(
            psql(&p, "x").display(),
            "docker exec pg psql -X -h 127.0.0.1 -p 5432 -U odoo -d x"
        );
        let local = Profile {
            mode: Mode::Local,
            pg_port: 5432,
            ..Default::default()
        };
        assert_eq!(psql(&local, "x").display(), "psql -X -p 5432 -d x");
    }
}
