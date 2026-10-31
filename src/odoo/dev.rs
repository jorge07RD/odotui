//! Herramientas de desarrollo: tests, `odoo shell`, `scaffold` y SQL.

use anyhow::{Result, bail};
use tokio_util::sync::CancellationToken;

use crate::{
    config::Profile,
    exec::{self, Input, Stop},
    jobs::Tx,
    log,
    odoo::{modules, pg},
};

/// `-u <módulos> --test-enable`; los tests fallidos hacen que Odoo salga con
/// error, así que el resultado se lee del log y no del código de salida.
pub async fn run_tests(
    p: &Profile,
    db: &str,
    names: &[String],
    tags: &str,
    tx: &Tx,
    cancel: &CancellationToken,
) -> Result<String> {
    if names.is_empty() {
        bail!("indica al menos un módulo");
    }
    let list = names.join(",");
    let tags = if tags.is_empty() {
        names
            .iter()
            .map(|n| format!("/{n}"))
            .collect::<Vec<_>>()
            .join(",")
    } else {
        tags.to_string()
    };
    let spec = tests_spec(p, db, &list, &tags)?;
    let (tx, lines) = tx.capturing();
    modules::warn_problems(p, names, &tx).await;
    let r = exec::stream(&spec, None, &tx, cancel, Stop::Interrupt).await;
    if cancel.is_cancelled() {
        bail!("cancelado");
    }
    let lines = lines.lock().map(|l| l.clone()).unwrap_or_default();
    let failed = lines
        .iter()
        .filter(|l| l.contains("FAIL:") || (l.contains("ERROR:") && l.contains(".tests")))
        .count();
    let summary = lines
        .iter()
        .rev()
        .find(|l| l.contains(" tests") && (l.contains("failed") || l.contains("error")))
        .cloned();
    let head = match (&r, failed) {
        (Ok(_), 0) => format!("tests de {list}: OK"),
        (_, 0) => format!("tests de {list}: Odoo terminó con error"),
        (_, n) => format!("tests de {list}: {n} fallo(s)"),
    };
    let text = match summary {
        Some(s) => format!("{head}\n{s}"),
        None => head,
    };
    // Con fallos es un error: la TUI lo marca en rojo y la IA lo ve como tal
    // (el informe con los tracebacks va igual).
    if r.is_err() || failed > 0 {
        bail!("{text}");
    }
    Ok(text)
}

/// Puerto HTTP para los tests: con `--test-enable` Odoo levanta su servidor
/// web (para los `HttpCase`) aunque se pida `--no-http`, y el 8069 suele
/// estar ocupado por la instancia en marcha.
fn test_port() -> u16 {
    18069 + (std::process::id() % 1000) as u16
}

fn tests_spec(p: &Profile, db: &str, list: &str, tags: &str) -> Result<exec::Spec> {
    let port = test_port().to_string();
    Ok(modules::odoo(p)?.args([
        "-d",
        db,
        "-u",
        list,
        "--test-enable",
        "--test-tags",
        tags,
        "--stop-after-init",
        "--http-port",
        &port,
    ]))
}

/// Ejecuta código Python con `env` del ORM. Odoo hace rollback al final:
/// para guardar cambios el código debe llamar a `env.cr.commit()`.
pub async fn shell(
    p: &Profile,
    db: &str,
    code: &str,
    tx: &Tx,
    cancel: &CancellationToken,
) -> Result<String> {
    let spec =
        modules::odoo_sub(p, Some("shell"))?.args(["-d", db, "--no-http", "--log-level=warn"]);
    exec::stream(
        &spec,
        Some(Input::Bytes(code.as_bytes().to_vec())),
        tx,
        cancel,
        Stop::Interrupt,
    )
    .await?;
    Ok(format!("código ejecutado en «{db}»"))
}

pub fn valid_module(name: &str) -> Result<()> {
    let ok = !name.is_empty()
        && name.starts_with(|c: char| c.is_ascii_lowercase())
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
    if !ok {
        bail!("nombre de módulo inválido: «{name}» (minúsculas, números y _)");
    }
    Ok(())
}

/// `odoo scaffold <nombre> <addons_dir>`; devuelve la ruta creada.
pub async fn scaffold(p: &Profile, name: &str, tx: &Tx) -> Result<String> {
    valid_module(name)?;
    if p.addons_dir.is_empty() {
        bail!("el perfil no tiene addons_dir (carpeta de addons propios)");
    }
    let mut spec = modules::odoo_sub(p, Some("scaffold"))?.args([name, &p.addons_dir]);
    if p.is_docker() {
        // Con el uid del usuario, los archivos del bind mount quedan editables.
        // SAFETY: getuid/getgid no fallan ni tocan memoria.
        let (uid, gid) = unsafe { (libc::getuid(), libc::getgid()) };
        spec = spec.user(&format!("{uid}:{gid}"));
    }
    tx.log(format!("$ {}", spec.display()));
    exec::capture(&spec).await?;
    Ok(format!("{}/{name}", p.addons_dir.trim_end_matches('/')))
}

/// Consulta con salida de tabla. Por defecto la transacción es de solo
/// lectura; `write` lo permite todo.
pub async fn sql(p: &Profile, db: &str, sql: &str, write: bool, limit: usize) -> Result<String> {
    let mut spec = pg::psql(p, db).args(["-P", "pager=off", "-v", "ON_ERROR_STOP=1", "-c", sql]);
    if !write {
        spec = spec.env("PGOPTIONS", "-c default_transaction_read_only=on");
    }
    let out = exec::capture(&spec).await?;
    let lines: Vec<&str> = out.lines().collect();
    if lines.len() > limit {
        return Ok(format!(
            "{}\n… {} líneas más (usa LIMIT o sube `limit`)",
            lines[..limit].join("\n"),
            lines.len() - limit
        ));
    }
    Ok(out)
}

/// Resumen de un log para una IA: cada error con su traceback y el final.
pub fn digest(lines: &[String], tail: usize) -> String {
    let mut out: Vec<String> = vec![];
    let mut budget = 200usize;
    for r in log::error_blocks(lines) {
        if budget == 0 {
            break;
        }
        let take = r.len().min(budget);
        out.extend(lines[r.start..r.start + take].iter().cloned());
        out.push(String::new());
        budget -= take;
    }
    let mut s = String::new();
    if !out.is_empty() {
        s += "── errores ──\n";
        s += &out.join("\n");
        s += "\n";
    }
    if tail == 0 {
        return s.trim_end().to_string();
    }
    let start = lines.len().saturating_sub(tail);
    s += &format!("── últimas {} líneas ──\n", lines.len() - start);
    s += &lines[start..].join("\n");
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tests_use_their_own_http_port() {
        let p = Profile {
            mode: crate::config::Mode::Docker,
            odoo_container: "web".into(),
            ..Default::default()
        };
        let s = tests_spec(&p, "db", "x", "/x").unwrap().display();
        // Nada de --no-http: con tests Odoo abre HTTP igual y chocaría con el 8069.
        assert!(!s.contains("--no-http"), "{s}");
        let port: u16 = s.split("--http-port ").nth(1).unwrap().parse().unwrap();
        assert!(port != 8069 && port >= 18069);
    }

    #[test]
    fn module_names() {
        assert!(valid_module("my_mod2").is_ok());
        assert!(valid_module("MyMod").is_err());
        assert!(valid_module("2mod").is_err());
        assert!(valid_module("a-b").is_err());
    }

    #[test]
    fn digest_keeps_tracebacks() {
        let lines: Vec<String> = [
            "2025-01-01 10:00:00,000 1 INFO db odoo: inicio",
            "2025-01-01 10:00:01,000 1 ERROR db odoo.addons.x.tests.test_a: FAIL: TestA.test_b",
            "Traceback (most recent call last):",
            "  File \"/x/test_a.py\", line 3, in test_b",
            "AssertionError: 1 != 2",
            "2025-01-01 10:00:02,000 1 INFO db odoo.tests.result: 1 failed, 0 error(s) of 3 tests",
        ]
        .map(String::from)
        .to_vec();
        let d = digest(&lines, 1);
        assert!(d.contains("AssertionError: 1 != 2"));
        assert!(d.contains("1 failed"));
        assert!(!d.contains("inicio"));

        // Un error de psql no arrastra las líneas propias que le siguen.
        let lines: Vec<String> = ["ERROR:  algo", "▶ 3/4 Copiando", "ok"]
            .map(String::from)
            .to_vec();
        let d = digest(&lines, 0);
        assert_eq!(d, "── errores ──\nERROR:  algo");
    }
}
