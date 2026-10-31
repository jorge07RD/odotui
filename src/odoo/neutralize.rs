//! Deja una copia de producción segura para desarrollo: sin crons, sin
//! correos salientes ni entrantes, sin pagos reales y con el admin conocido.

use anyhow::Result;
use base64::Engine;
use sha2::Sha512;

use crate::{
    config::Profile,
    jobs::Tx,
    odoo::pg::{self, literal},
};

/// Rondas por defecto de Odoo 17+ para `pbkdf2_sha512`.
pub const ROUNDS: u32 = 600_000;

/// Base64 «adaptado» de passlib: sin relleno y con `.` en lugar de `+`.
fn ab64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD_NO_PAD
        .encode(bytes)
        .replace('+', ".")
}

fn salt() -> [u8; 16] {
    use std::io::Read;
    let mut s = [0u8; 16];
    if std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut s))
        .is_err()
    {
        // Sin /dev/urandom basta con algo distinto en cada llamada.
        let t = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
            .to_le_bytes();
        s.copy_from_slice(&t);
    }
    s
}

/// Hash en el formato que guarda Odoo (`passlib` `pbkdf2_sha512`).
pub fn hash_with(password: &str, salt: &[u8], rounds: u32) -> String {
    let mut out = [0u8; 64];
    pbkdf2::pbkdf2_hmac::<Sha512>(password.as_bytes(), salt, rounds, &mut out);
    format!("$pbkdf2-sha512${rounds}${}${}", ab64(salt), ab64(&out))
}

pub fn hash(password: &str) -> String {
    hash_with(password, &salt(), ROUNDS)
}

/// Un solo bloque; cada tabla opcional va protegida por `to_regclass`, así
/// sirve para cualquier versión de Odoo y cualquier combinación de módulos.
pub fn sql(url: &str, password_hash: Option<&str>) -> String {
    let mut s = String::from(
        "DO $$\nDECLARE admin_id integer;\nBEGIN\n\
         IF to_regclass('ir_cron') IS NOT NULL THEN UPDATE ir_cron SET active = false; END IF;\n\
         IF to_regclass('ir_mail_server') IS NOT NULL THEN UPDATE ir_mail_server SET active = false; END IF;\n\
         IF to_regclass('fetchmail_server') IS NOT NULL THEN UPDATE fetchmail_server SET active = false; END IF;\n\
         IF to_regclass('payment_provider') IS NOT NULL THEN UPDATE payment_provider SET state = 'disabled' WHERE state <> 'disabled'; END IF;\n\
         IF to_regclass('payment_acquirer') IS NOT NULL THEN UPDATE payment_acquirer SET state = 'disabled' WHERE state <> 'disabled'; END IF;\n\
         IF to_regclass('ir_config_parameter') IS NOT NULL THEN\n\
           DELETE FROM ir_config_parameter WHERE key IN ('database.enterprise_code', 'database.expiration_date', 'database.expiration_reason', 'web.base.url.freeze');\n\
           UPDATE ir_config_parameter SET value = uuid_in(md5(random()::text || clock_timestamp()::text)::cstring)::text WHERE key = 'database.uuid';\n",
    );
    s += &format!(
        "  UPDATE ir_config_parameter SET value = {} WHERE key = 'web.base.url';\n",
        literal(url)
    );
    s += "  INSERT INTO ir_config_parameter (key, value, create_uid, write_uid, create_date, write_date) \
          VALUES ('database.is_neutralized', 'true', 1, 1, now(), now()) \
          ON CONFLICT (key) DO UPDATE SET value = 'true';\n\
          END IF;\n";
    if let Some(h) = password_hash {
        // El administrador es `base.user_admin`: en las BDs creadas desde el
        // gestor web su login es el email que se puso, no «admin».
        s += &format!(
            "IF to_regclass('res_users') IS NOT NULL THEN\n\
               IF to_regclass('ir_model_data') IS NOT NULL THEN\n\
                 SELECT res_id INTO admin_id FROM ir_model_data WHERE module = 'base' AND name = 'user_admin';\n\
               END IF;\n\
               IF admin_id IS NULL THEN SELECT id INTO admin_id FROM res_users WHERE login = 'admin'; END IF;\n\
               UPDATE res_users SET password = {} WHERE id = admin_id;\n\
             END IF;\n",
            literal(h)
        );
    }
    s += "END $$;";
    s
}

/// `admin_password` vacío = no tocar la contraseña.
pub async fn run(p: &Profile, db: &str, admin_password: &str, tx: &Tx) -> Result<()> {
    let h = if admin_password.is_empty() {
        None
    } else {
        let pw = admin_password.to_string();
        // 600k rondas tardan un momento: fuera del hilo del runtime.
        Some(tokio::task::spawn_blocking(move || hash(&pw)).await?)
    };
    pg::execute(p, db, &sql(p.url(), h.as_deref())).await?;
    tx.log("crons, servidores de correo y pagos desactivados");
    tx.log(format!("web.base.url = {}", p.url()));
    if h.is_some() {
        let login = admin_login(p, db).await.unwrap_or_else(|| "admin".into());
        tx.log(format!(
            "acceso de administrador: «{login}» / {admin_password}"
        ));
    }
    Ok(())
}

/// Login del administrador (`base.user_admin`), que no siempre es «admin».
pub async fn admin_login(p: &Profile, db: &str) -> Option<String> {
    let by_xmlid = "SELECT u.login FROM res_users u JOIN ir_model_data d \
                    ON d.res_id = u.id AND d.model = 'res.users' \
                    WHERE d.module = 'base' AND d.name = 'user_admin'";
    for q in [
        by_xmlid,
        "SELECT login FROM res_users WHERE login = 'admin'",
    ] {
        if let Ok(rows) = pg::query(p, db, q).await
            && let Some(login) = rows.into_iter().next().and_then(|r| r.into_iter().next())
        {
            return Some(login);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passlib_format() {
        // Referencia: python3 -c "import hashlib,base64; ...pbkdf2_hmac('sha512', b'admin', b'0123456789abcdef', 1000)"
        let h = hash_with("admin", b"0123456789abcdef", 1000);
        let parts: Vec<&str> = h.split('$').collect();
        assert_eq!(parts[1], "pbkdf2-sha512");
        assert_eq!(parts[2], "1000");
        assert_eq!(parts[3], "MDEyMzQ1Njc4OWFiY2RlZg");
        assert_eq!(parts[4].len(), 86);
        assert!(!h.contains('+') && !h.contains('='));
        assert_eq!(h, include_str!("../../tests/pbkdf2_admin.txt").trim());
    }

    #[test]
    fn sql_quotes_values() {
        let s = sql("http://x/'y", Some("$h"));
        assert!(s.contains("'http://x/''y'"));
        assert!(s.contains("password = '$h' WHERE id = admin_id"));
        assert!(s.contains("name = 'user_admin'"));
        assert!(s.starts_with("DO $$\nDECLARE admin_id integer;\nBEGIN"));
        assert!(!sql("u", None).contains("res_users"));
    }
}
