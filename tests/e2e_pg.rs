//! Pruebas contra un Postgres de verdad. Se saltan si no está
//! `ODOTUI_E2E_PG`, p. ej.:
//!
//! ```bash
//! ODOTUI_E2E_PG="host=127.0.0.1 port=5432 user=odoo password=odoo" cargo test --test e2e_pg
//! ```

use std::path::PathBuf;

use odotui::{
    ai::tools,
    config::{Mode, Profile},
    jobs::Tx,
    odoo::{dev, pg, restore},
};
use serde_json::json;
use tokio_util::sync::CancellationToken;

/// Una BD «de Odoo» mínima: las tablas que tocan la lista y la neutralización.
const FIXTURE: &str = "
CREATE TABLE ir_module_module (id serial primary key, name varchar unique, state varchar,
    latest_version varchar, application boolean);
INSERT INTO ir_module_module (name, state, latest_version, application)
    VALUES ('base', 'installed', '17.0.1.3', false), ('sale', 'installed', '17.0.1.2', true);
CREATE TABLE ir_cron (id serial primary key, name varchar, active boolean);
INSERT INTO ir_cron (name, active) VALUES ('mail queue', true);
CREATE TABLE ir_mail_server (id serial primary key, name varchar, active boolean);
INSERT INTO ir_mail_server (name, active) VALUES ('smtp', true);
CREATE TABLE ir_config_parameter (id serial primary key, key varchar not null unique,
    value text not null, create_uid int, write_uid int, create_date timestamp, write_date timestamp);
INSERT INTO ir_config_parameter (key, value) VALUES
    ('database.uuid', 'viejo'), ('web.base.url', 'https://prod.example.com'),
    ('database.enterprise_code', 'M123');
CREATE TABLE res_users (id serial primary key, login varchar, password varchar);
INSERT INTO res_users (login, password) VALUES ('admin', 'x');
";

fn profile() -> Option<Profile> {
    let raw = std::env::var("ODOTUI_E2E_PG").ok()?;
    let mut p = Profile {
        name: "e2e".into(),
        mode: Mode::Local,
        ..Default::default()
    };
    for kv in raw.split_whitespace() {
        match kv.split_once('=') {
            Some(("host", v)) => p.pg_host = v.into(),
            Some(("port", v)) => p.pg_port = v.parse().expect("puerto"),
            Some(("user", v)) => p.pg_user = v.into(),
            Some(("password", v)) => p.pg_password = v.into(),
            _ => panic!("ODOTUI_E2E_PG: no entiendo «{kv}»"),
        }
    }
    Some(p)
}

/// Nombre único por proceso para no chocar entre pruebas en paralelo.
fn name(base: &str) -> String {
    format!("e2e_{base}_{}", std::process::id())
}

fn tmp(base: &str) -> PathBuf {
    let d = std::env::temp_dir().join(name(base));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

async fn one(p: &Profile, db: &str, sql: &str) -> String {
    pg::query(p, db, sql)
        .await
        .unwrap()
        .into_iter()
        .next()
        .and_then(|r| r.into_iter().next())
        .unwrap_or_default()
}

#[tokio::test]
async fn database_lifecycle() {
    let Some(p) = profile() else { return };
    let (a, b, c) = (name("a"), name("b"), name("c"));
    for db in [&a, &b, &c] {
        pg::drop(&p, db).await.unwrap();
    }
    pg::create(&p, &a).await.unwrap();
    pg::execute(&p, &a, FIXTURE).await.unwrap();
    assert!(pg::exists(&p, &a).await.unwrap());

    let dbs = pg::list(&p).await.unwrap();
    let info = dbs
        .iter()
        .find(|d| d.name == a)
        .expect("aparece en la lista");
    assert_eq!(info.version.as_deref(), Some("17.0.1.3"));

    pg::duplicate(&p, &a, &b).await.unwrap();
    // La copia no comparte el uuid.
    assert_ne!(
        one(
            &p,
            &b,
            "SELECT value FROM ir_config_parameter WHERE key = 'database.uuid'"
        )
        .await,
        "viejo"
    );

    pg::rename(&p, &b, &c).await.unwrap();
    assert!(!pg::exists(&p, &b).await.unwrap());
    assert!(pg::exists(&p, &c).await.unwrap());

    for db in [&a, &c] {
        pg::drop(&p, db).await.unwrap();
        assert!(!pg::exists(&p, db).await.unwrap());
    }
}

#[tokio::test]
async fn restore_folder_with_filestore_and_neutralize() {
    let Some(mut p) = profile() else { return };
    let backups = tmp("backups");
    let filestore = tmp("filestore");
    let folder = backups.join("prod_2025");
    std::fs::create_dir_all(folder.join("filestore/ab")).unwrap();
    std::fs::write(folder.join("dump.sql"), FIXTURE).unwrap();
    std::fs::write(folder.join("filestore/ab/abc"), "adjunto").unwrap();
    p.backups_dir = backups.display().to_string();
    p.filestore_dir = filestore.display().to_string();

    let listed = restore::list(&p.backups_dir).await.unwrap();
    assert_eq!(listed.len(), 1);
    assert!(listed[0].filestore);

    let db = name("restaurada");
    let req = restore::Request {
        profile: p.clone(),
        backup: "prod_2025".into(),
        db: db.clone(),
        overwrite: true,
        filestore: true,
        neutralize: true,
        admin_password: "admin".into(),
    };
    let (tx, lines) = Tx::detached().capturing();
    let msg = restore::run(req, &tx, &CancellationToken::new())
        .await
        .unwrap();
    assert!(msg.contains(&db), "{msg}\n{:?}", lines.lock().unwrap());

    // Neutralizada.
    assert_eq!(
        one(&p, &db, "SELECT bool_or(active)::text FROM ir_cron").await,
        "false"
    );
    assert_eq!(
        one(&p, &db, "SELECT bool_or(active)::text FROM ir_mail_server").await,
        "false"
    );
    assert_eq!(
        one(
            &p,
            &db,
            "SELECT value FROM ir_config_parameter WHERE key = 'web.base.url'"
        )
        .await,
        "http://localhost:8069"
    );
    assert_eq!(
        one(
            &p,
            &db,
            "SELECT count(*) FROM ir_config_parameter WHERE key = 'database.enterprise_code'"
        )
        .await,
        "0"
    );
    assert!(
        one(
            &p,
            &db,
            "SELECT password FROM res_users WHERE login = 'admin'"
        )
        .await
        .starts_with("$pbkdf2-sha512$600000$")
    );
    // Filestore copiado.
    assert_eq!(
        std::fs::read_to_string(filestore.join(&db).join("ab/abc")).unwrap(),
        "adjunto"
    );

    // Sin `overwrite`, una BD existente no se toca.
    let again = restore::Request {
        profile: p.clone(),
        backup: "prod_2025".into(),
        db: db.clone(),
        overwrite: false,
        filestore: false,
        neutralize: false,
        admin_password: String::new(),
    };
    let err = restore::run(again, &Tx::detached(), &CancellationToken::new())
        .await
        .unwrap_err();
    assert!(err.to_string().contains("ya existe"));

    pg::drop(&p, &db).await.unwrap();
    let _ = std::fs::remove_dir_all(backups);
    let _ = std::fs::remove_dir_all(filestore);
}

#[tokio::test]
async fn sql_is_read_only_unless_asked() {
    let Some(p) = profile() else { return };
    let db = name("sql");
    pg::drop(&p, &db).await.unwrap();
    pg::create(&p, &db).await.unwrap();
    pg::execute(&p, &db, FIXTURE).await.unwrap();

    let err = dev::sql(&p, &db, "UPDATE ir_cron SET active = false", false, 50)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("read-only"), "{err}");
    dev::sql(&p, &db, "UPDATE ir_cron SET active = false", true, 50)
        .await
        .unwrap();
    let out = dev::sql(&p, &db, "SELECT name, active FROM ir_cron", false, 50)
        .await
        .unwrap();
    assert!(out.contains("mail queue") && out.contains(" f"), "{out}");

    pg::drop(&p, &db).await.unwrap();
}

#[tokio::test]
async fn mcp_tools_against_postgres() {
    let Some(p) = profile() else { return };
    let db = name("mcp");
    pg::drop(&p, &db).await.unwrap();
    pg::create(&p, &db).await.unwrap();
    pg::execute(&p, &db, FIXTURE).await.unwrap();
    let cancel = CancellationToken::new();
    let tx = Tx::detached();

    let dbs = tools::call("list_databases", &json!({}), &p, &tx, &cancel)
        .await
        .unwrap();
    assert!(dbs.contains(&db));
    let mods = tools::call(
        "list_modules",
        &json!({"database": db, "state": "installed"}),
        &p,
        &tx,
        &cancel,
    )
    .await
    .unwrap();
    assert!(mods.contains("sale\tinstalled"), "{mods}");
    let bad = tools::call(
        "list_modules",
        &json!({"database": "a b"}),
        &p,
        &tx,
        &cancel,
    )
    .await;
    assert!(bad.is_err());

    tools::call("drop_database", &json!({"database": db}), &p, &tx, &cancel)
        .await
        .unwrap();
    assert!(!pg::exists(&p, &db).await.unwrap());
}
