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
    odoo::{
        backup::{self, Format},
        dev, pg, restore,
    },
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

fn request(p: &Profile, backup: &str, db: &str) -> restore::Request {
    restore::Request {
        profile: p.clone(),
        backup: backup.into(),
        db: db.into(),
        overwrite: true,
        filestore: true,
        neutralize: false,
        admin_password: String::new(),
    }
}

async fn counts(p: &Profile, db: &str) -> String {
    one(
        p,
        db,
        "SELECT (SELECT count(*) FROM ir_module_module) || '/' || (SELECT count(*) FROM ir_cron) \
         || '/' || (SELECT count(*) FROM ir_config_parameter)",
    )
    .await
}

#[tokio::test]
async fn backup_round_trips_in_every_format() {
    let Some(mut p) = profile() else { return };
    let backups = tmp("rt_backups");
    let filestore = tmp("rt_filestore");
    p.backups_dir = backups.display().to_string();
    p.filestore_dir = filestore.display().to_string();
    let src = name("rt_src");
    pg::drop(&p, &src).await.unwrap();
    pg::create(&p, &src).await.unwrap();
    pg::execute(&p, &src, FIXTURE).await.unwrap();
    std::fs::create_dir_all(filestore.join(&src).join("ab")).unwrap();
    std::fs::write(filestore.join(&src).join("ab/abc"), "adjunto").unwrap();
    std::fs::write(filestore.join(&src).join("ab/abd"), "otro").unwrap();
    let want = counts(&p, &src).await;
    let cancel = CancellationToken::new();

    // zip: dump.sql + filestore + manifest, y de vuelta con otro nombre.
    let (tx, lines) = Tx::detached().capturing();
    let msg = backup::run(&p, &src, Format::Zip, true, &tx, &cancel)
        .await
        .unwrap_or_else(|e| panic!("{e:#}\n{:?}", lines.lock().unwrap()));
    assert!(msg.contains("2 archivo(s) del filestore"), "{msg}");
    let listed = restore::list(&p.backups_dir).await.unwrap();
    let zip = listed
        .iter()
        .find(|b| b.kind == restore::Kind::Zip)
        .expect("el zip aparece en la lista");
    assert!(zip.filestore);
    let m = zip.manifest.as_ref().unwrap();
    assert_eq!(
        (m.db_name.as_str(), m.version.as_str(), m.modules),
        (src.as_str(), "17.0", 2)
    );
    let from_zip = name("rt_zip");
    let (tx, lines) = Tx::detached().capturing();
    restore::run(request(&p, &zip.name, &from_zip), &tx, &cancel)
        .await
        .unwrap_or_else(|e| panic!("{e:#}\n{:?}", lines.lock().unwrap()));
    assert_eq!(counts(&p, &from_zip).await, want);
    assert_eq!(
        std::fs::read_to_string(filestore.join(&from_zip).join("ab/abd")).unwrap(),
        "otro"
    );
    // La carpeta temporal del filestore no queda.
    assert_eq!(restore::list(&p.backups_dir).await.unwrap().len(), 1);

    // dump (pg_dump -Fc) → pg_restore.
    backup::run(&p, &src, Format::Dump, false, &Tx::detached(), &cancel)
        .await
        .unwrap();
    let dump = restore::list(&p.backups_dir)
        .await
        .unwrap()
        .into_iter()
        .find(|b| b.kind == restore::Kind::PgCustom)
        .expect("el .dump aparece en la lista");
    let from_dump = name("rt_dump");
    let (tx, lines) = Tx::detached().capturing();
    restore::run(request(&p, &dump.name, &from_dump), &tx, &cancel)
        .await
        .unwrap_or_else(|e| panic!("{e:#}\n{:?}", lines.lock().unwrap()));
    assert_eq!(counts(&p, &from_dump).await, want);

    // .sql.gz hecho a mano.
    {
        use std::io::Write;
        let f = std::fs::File::create(backups.join("a_mano.sql.gz")).unwrap();
        let mut gz = flate2::write::GzEncoder::new(f, flate2::Compression::default());
        gz.write_all(FIXTURE.as_bytes()).unwrap();
        gz.finish().unwrap();
    }
    let from_gz = name("rt_gz");
    restore::run(
        request(&p, "a_mano.sql.gz", &from_gz),
        &Tx::detached(),
        &cancel,
    )
    .await
    .unwrap();
    assert_eq!(counts(&p, &from_gz).await, want);

    // Cancelado: no queda ningún archivo nuevo, ni siquiera oculto.
    let before = std::fs::read_dir(&backups).unwrap().count();
    let stop = CancellationToken::new();
    stop.cancel();
    let err = backup::run(&p, &src, Format::Zip, true, &Tx::detached(), &stop)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("cancelado"), "{err}");
    assert_eq!(std::fs::read_dir(&backups).unwrap().count(), before);

    for db in [&src, &from_zip, &from_dump, &from_gz] {
        pg::drop(&p, db).await.unwrap();
    }
    let _ = std::fs::remove_dir_all(backups);
    let _ = std::fs::remove_dir_all(filestore);
}

/// Lo justo de Odoo 17 para entender modelos, vistas y módulos (las columnas
/// traducibles son jsonb, como desde Odoo 16).
const SCHEMA: &str = r#"
CREATE TABLE ir_module_module (id serial primary key, name varchar unique, state varchar,
    latest_version varchar, application boolean, author varchar);
INSERT INTO ir_module_module (name, state, latest_version, author) VALUES
    ('base', 'installed', '17.0.1.3', 'Odoo S.A.'), ('acme', 'installed', '17.0.1.0', 'ACME'),
    ('acme_extra', 'installed', '17.0.1.0', 'ACME');
CREATE TABLE ir_module_module_dependency (id serial primary key, module_id int, name varchar);
INSERT INTO ir_module_module_dependency (module_id, name) VALUES (2, 'base'), (3, 'acme');
CREATE TABLE ir_model (id serial primary key, model varchar, name jsonb);
INSERT INTO ir_model (model, name) VALUES ('acme.nota', '{"en_US": "Note", "es_ES": "Nota"}');
CREATE TABLE ir_model_fields (id serial primary key, model varchar, name varchar, ttype varchar,
    relation varchar, required bool, store bool, readonly bool, compute text, related varchar,
    field_description jsonb);
INSERT INTO ir_model_fields (model, name, ttype, relation, required, store, readonly, compute, related, field_description) VALUES
    ('acme.nota', 'name', 'char', null, true, true, false, null, null, '{"en_US": "Name"}'),
    ('acme.nota', 'partner_id', 'many2one', 'res.partner', false, true, false, null, null, '{"en_US": "Partner"}'),
    ('acme.nota', 'largo', 'integer', null, false, true, true, 'for n in self: ...', null, '{"en_US": "Length"}'),
    ('acme.nota', 'estado', 'selection', null, false, true, false, null, null, '{"en_US": "State"}');
CREATE TABLE ir_model_fields_selection (id serial primary key, field_id int, value varchar,
    name jsonb, sequence int);
INSERT INTO ir_model_fields_selection (field_id, value, name, sequence) VALUES
    (4, 'borrador', '{"en_US": "Draft"}', 1), (4, 'hecho', '{"en_US": "Done"}', 2);
CREATE TABLE ir_model_data (id serial primary key, module varchar, name varchar, model varchar,
    res_id int);
INSERT INTO ir_model_data (module, name, model, res_id) VALUES
    ('acme', 'model_acme_nota', 'ir.model', 1),
    ('acme', 'field_acme_nota__name', 'ir.model.fields', 1),
    ('acme', 'field_acme_nota__partner_id', 'ir.model.fields', 2),
    ('acme_extra', 'field_acme_nota__largo', 'ir.model.fields', 3),
    ('acme', 'nota_view_form', 'ir.ui.view', 1),
    ('acme_extra', 'nota_view_form_extra', 'ir.ui.view', 2);
CREATE TABLE ir_ui_view (id serial primary key, name varchar, model varchar, type varchar,
    priority int, mode varchar, inherit_id int, active bool, arch_db jsonb);
INSERT INTO ir_ui_view (name, model, type, priority, mode, inherit_id, active, arch_db) VALUES
    ('acme.nota.form', 'acme.nota', 'form', 16, 'primary', null, true,
     '{"en_US": "<form>\n  <field name=\"name\"/>\n</form>"}'),
    ('acme.nota.form.extra', 'acme.nota', 'form', 20, 'extension', 1, true,
     '{"en_US": "<field name=\"name\" position=\"after\">\n  <field name=\"largo\"/>\n</field>"}');
"#;

#[tokio::test]
async fn ai_understands_models_and_views() {
    let Some(mut p) = profile() else { return };
    let addons = tmp("inspect_addons");
    std::fs::create_dir_all(addons.join("acme")).unwrap();
    std::fs::write(
        addons.join("acme/__manifest__.py"),
        "{'name': 'ACME', 'version': '1.1', 'depends': ['base']}",
    )
    .unwrap();
    p.addons_dir = addons.display().to_string();
    let db = name("inspect");
    pg::drop(&p, &db).await.unwrap();
    pg::create(&p, &db).await.unwrap();
    pg::execute(&p, &db, SCHEMA).await.unwrap();
    let (tx, cancel) = (Tx::detached(), CancellationToken::new());
    let call = |tool: &'static str, args: serde_json::Value| {
        let (p, tx, cancel) = (p.clone(), tx.clone(), cancel.clone());
        async move { tools::call(tool, &args, &p, &tx, &cancel).await }
    };

    let m = call(
        "describe_model",
        json!({"database": db, "model": "acme.nota"}),
    )
    .await
    .unwrap();
    assert!(m.starts_with("acme.nota — Note (módulos: acme)"), "{m}");
    assert!(
        m.contains("partner_id\tmany2one→res.partner\tguardado\t«Partner»\tacme"),
        "{m}"
    );
    assert!(
        m.contains("largo\tinteger\tcalculado·guardado·solo lectura\t«Length»\tacme_extra"),
        "{m}"
    );
    assert!(m.contains("[borrador, hecho]"), "{m}");
    assert!(m.contains("name\tchar\trequerido·guardado"), "{m}");
    let bad = call(
        "describe_model",
        json!({"database": db, "model": "no.existe"}),
    )
    .await;
    assert!(bad.unwrap_err().contains("no existe el modelo"));

    let v = call("find_views", json!({"database": db, "model": "acme.nota"}))
        .await
        .unwrap();
    assert!(
        v.contains("acme.nota_view_form\tform\t16\tprimary\tacme.nota"),
        "{v}"
    );
    assert!(
        v.contains("acme_extra.nota_view_form_extra\tform\t20\textension\t← acme.nota_view_form"),
        "{v}"
    );
    let chain = call(
        "find_views",
        json!({"database": db, "xml_id": "acme_extra.nota_view_form_extra"}),
    )
    .await
    .unwrap();
    assert!(chain.starts_with("2 vista(s)"), "{chain}");

    let arch = call(
        "get_view",
        json!({"database": db, "xml_id": "acme.nota_view_form"}),
    )
    .await
    .unwrap();
    let (a, b) = (
        arch.find("<form>").unwrap(),
        arch.find("position=\"after\"").unwrap(),
    );
    assert!(a < b, "la base va antes que la heredada:\n{arch}");
    assert!(
        arch.contains("<form>\n  <field name=\"name\"/>\n</form>"),
        "{arch}"
    );

    let info = call("module_info", json!({"database": db, "name": "acme"}))
        .await
        .unwrap();
    assert!(info.contains("installed (versión 17.0.1.0)"), "{info}");
    assert!(info.contains("lo usan (instalados): acme_extra"), "{info}");
    assert!(info.contains("⟳ el código es 17.0.1.1"), "{info}");
    assert!(info.contains("\"depends\""), "{info}");

    pg::drop(&p, &db).await.unwrap();
    let _ = std::fs::remove_dir_all(addons);
}
