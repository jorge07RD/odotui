//! «Conectar» como en Odoo.sh: abre Odoo en el navegador ya dentro, como el
//! administrador o como el usuario que elijas, sin tocar contraseñas.
//!
//! 1. `odoo shell` crea una sesión de verdad con `Session.finalize(env)`, la
//!    misma que hace Odoo al iniciar sesión, y la guarda en su almacén.
//! 2. Un servidor HTTP de un solo uso en `localhost:<puerto>` pone la cookie
//!    `session_id` y redirige a `/web`. Las cookies no distinguen puertos, así
//!    que el navegador la manda luego a Odoo.

use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};
use tokio_util::sync::CancellationToken;

use crate::{
    config::Profile,
    exec::{self, Input, Stop},
    jobs::Tx,
    odoo::{modules, pg},
};

/// Cuánto espera el enlace a que el navegador lo abra.
const LINK_TTL: Duration = Duration::from_secs(120);

#[derive(Debug, Clone, PartialEq)]
pub struct OdooUser {
    pub id: i64,
    pub login: String,
    pub name: String,
    /// Usuario de portal / público (`share`).
    pub portal: bool,
    /// En «Administración / Ajustes» (`base.group_system`).
    pub admin: bool,
}

impl OdooUser {
    /// Etiqueta para el selector.
    pub fn label(&self) -> String {
        let kind = if self.admin {
            "admin"
        } else if self.portal {
            "portal"
        } else {
            "interno"
        };
        format!("{}  ·  {}  [{kind}]", self.name, self.login)
    }
}

/// Usuarios activos: primero los administradores, luego los internos y al
/// final los de portal.
pub async fn users(p: &Profile, db: &str) -> Result<Vec<OdooUser>> {
    let sql = "SELECT u.id, u.login, COALESCE(p.name, u.login), u.share::int, \
               EXISTS (SELECT 1 FROM res_groups_users_rel r \
                       JOIN ir_model_data d ON d.model = 'res.groups' AND d.res_id = r.gid \
                       WHERE r.uid = u.id AND d.module = 'base' AND d.name = 'group_system')::int \
               FROM res_users u LEFT JOIN res_partner p ON p.id = u.partner_id \
               WHERE u.active \
               ORDER BY 5 DESC, 4, lower(COALESCE(p.name, u.login))";
    let rows = pg::query(p, db, sql).await?;
    Ok(rows
        .into_iter()
        .filter_map(|r| {
            Some(OdooUser {
                id: r.first()?.parse().ok()?,
                login: r.get(1)?.clone(),
                name: r.get(2)?.clone(),
                portal: r.get(3)? == "1",
                admin: r.get(4)? == "1",
            })
        })
        .collect())
}

#[derive(Debug, Deserialize)]
struct Created {
    sid: String,
    login: String,
    name: String,
}

const MARK: &str = "ODOTUI_SESSION ";

/// Código para `odoo shell`: crea la sesión de `uid` (o del administrador).
fn shell_code(uid: Option<i64>) -> String {
    let user = match uid {
        Some(id) => format!("env['res.users'].browse({id}).exists()"),
        None => "env.ref('base.user_admin', raise_if_not_found=False)".into(),
    };
    format!(
        "import json\n\
         from odoo.http import root, get_default_session\n\
         u = {user}\n\
         if not u or not u.active:\n    \
             print({mark:?} + json.dumps({{'error': 'el usuario no existe o está archivado'}}))\n\
         else:\n    \
             s = root.session_store.new()\n    \
             s.update(get_default_session())\n    \
             s['pre_login'] = u.login\n    \
             s['pre_uid'] = u.id\n    \
             s.finalize(env)\n    \
             root.session_store.save(s)\n    \
             print({mark:?} + json.dumps({{'sid': s.sid, 'login': u.login, 'name': u.name}}))\n",
        mark = MARK,
    )
}

/// La línea que deja `shell_code`.
fn parse_created(lines: &[String]) -> Result<Created> {
    let Some(json) = lines
        .iter()
        .rev()
        .find_map(|l| l.split_once(MARK).map(|x| x.1))
    else {
        let tail: Vec<&str> = lines
            .iter()
            .rev()
            .take(8)
            .rev()
            .map(String::as_str)
            .collect();
        bail!("odoo shell no creó la sesión:\n{}", tail.join("\n"));
    };
    let v: serde_json::Value = serde_json::from_str(json.trim())?;
    if let Some(e) = v.get("error").and_then(|e| e.as_str()) {
        bail!("{e}");
    }
    let c: Created = serde_json::from_value(v)?;
    if c.sid.is_empty()
        || !c
            .sid
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        bail!("sesión inválida");
    }
    Ok(c)
}

/// Host y puerto de `odoo_url` si es local (el navegador debe mandar la
/// cookie al mismo host).
fn local_host(odoo_url: &str) -> Result<String> {
    let rest = odoo_url
        .strip_prefix("http://")
        .or_else(|| odoo_url.strip_prefix("https://"))
        .unwrap_or(odoo_url);
    let authority = rest.split('/').next().unwrap_or_default();
    let host = if let Some(v6) = authority.strip_prefix('[') {
        v6.split(']').next().unwrap_or_default()
    } else {
        authority.split(':').next().unwrap_or_default()
    };
    match host {
        "localhost" | "127.0.0.1" => Ok(host.to_string()),
        _ => bail!("«Conectar» solo funciona con Odoo en este equipo (odoo_url = {odoo_url})"),
    }
}

fn nonce() -> String {
    use std::io::Read;
    let mut b = [0u8; 16];
    if std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut b))
        .is_err()
    {
        let t = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        b = t.to_le_bytes();
    }
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Respuesta del enlace: la cookie de Odoo y a `/web`.
fn redirect(odoo_url: &str, sid: &str) -> String {
    let web = format!("{}/web", odoo_url.trim_end_matches('/'));
    format!(
        "HTTP/1.1 302 Found\r\n\
         Location: {web}\r\n\
         Set-Cookie: session_id={sid}; Path=/; Max-Age=7776000; HttpOnly; SameSite=Lax\r\n\
         Cache-Control: no-store\r\n\
         Content-Length: 0\r\n\
         Connection: close\r\n\r\n"
    )
}

/// Sirve una sola vez el enlace que pone la cookie; devuelve su URL.
async fn serve_link(odoo_url: &str, sid: &str) -> Result<String> {
    let host = local_host(odoo_url)?;
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .context("no pude abrir un puerto local")?;
    let port = listener.local_addr()?.port();
    let path = format!("/{}", nonce());
    let url = format!("http://{host}:{port}{path}");
    let response = redirect(odoo_url, sid);
    tokio::spawn(async move {
        let _ = tokio::time::timeout(LINK_TTL, async {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else {
                    return;
                };
                let mut buf = vec![0u8; 8192];
                let mut n = 0;
                while n < buf.len() {
                    match sock.read(&mut buf[n..]).await {
                        Ok(0) | Err(_) => break,
                        Ok(k) => n += k,
                    }
                    if buf[..n].windows(4).any(|w| w == b"\r\n\r\n") {
                        break;
                    }
                }
                let head = String::from_utf8_lossy(&buf[..n]);
                let target = head.split_whitespace().nth(1).unwrap_or_default();
                if target == path {
                    let _ = sock.write_all(response.as_bytes()).await;
                    let _ = sock.shutdown().await;
                    return;
                }
                let _ = sock
                    .write_all(
                        b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    )
                    .await;
            }
        })
        .await;
    });
    Ok(url)
}

/// Abre una URL en el navegador del escritorio.
pub fn open_browser(url: &str) -> Result<()> {
    std::process::Command::new("xdg-open")
        .arg(url)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .context("xdg-open")?;
    Ok(())
}

/// Crea la sesión de `uid` (o del administrador) en `db` y deja listo el
/// enlace que la abre; con `open`, lo abre en el navegador.
pub async fn connect(p: &Profile, db: &str, uid: Option<i64>, open: bool) -> Result<String> {
    local_host(p.url())?;
    let spec =
        modules::odoo_sub(p, Some("shell"))?.args(["-d", db, "--no-http", "--log-level=warn"]);
    let (tx, lines) = Tx::detached().capturing();
    let code = shell_code(uid);
    exec::stream(
        &spec,
        Some(Input::Bytes(code.into_bytes())),
        &tx,
        &CancellationToken::new(),
        Stop::Interrupt,
    )
    .await?;
    let lines = lines.lock().map(|l| l.clone()).unwrap_or_default();
    let c = parse_created(&lines)?;
    let url = serve_link(p.url(), &c.sid).await?;
    if open {
        open_browser(&url)?;
    }
    Ok(format!(
        "conectado como {} ({}) en «{db}»: {url} (vale una vez, {} s)",
        c.name,
        c.login,
        LINK_TTL.as_secs()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_local_odoo() {
        assert_eq!(local_host("http://localhost:8069").unwrap(), "localhost");
        assert_eq!(local_host("http://127.0.0.1:8069/").unwrap(), "127.0.0.1");
        assert!(local_host("https://phoenix-testing.odoo.com").is_err());
    }

    #[test]
    fn parses_the_session_line() {
        let lines = vec![
            "WARNING ruido".to_string(),
            format!("{MARK}{{\"sid\": \"abc123\", \"login\": \"admin\", \"name\": \"Mitchell\"}}"),
        ];
        let c = parse_created(&lines).unwrap();
        assert_eq!((c.sid.as_str(), c.login.as_str()), ("abc123", "admin"));

        let err = vec![format!("{MARK}{{\"error\": \"archivado\"}}")];
        assert!(
            parse_created(&err)
                .unwrap_err()
                .to_string()
                .contains("archivado")
        );
        assert!(parse_created(&["Traceback".into()]).is_err());
    }

    #[test]
    fn shell_code_picks_admin_or_user() {
        assert!(shell_code(None).contains("base.user_admin"));
        assert!(shell_code(Some(7)).contains("browse(7)"));
    }

    #[tokio::test]
    async fn link_sets_cookie_once() {
        let url = serve_link("http://localhost:8069", "abc").await.unwrap();
        let addr = url.trim_start_matches("http://localhost");
        let (port, path) = addr[1..].split_once('/').unwrap();
        let get = |p: String| async move {
            let mut s = tokio::net::TcpStream::connect(format!("127.0.0.1:{port}"))
                .await
                .unwrap();
            s.write_all(format!("GET {p} HTTP/1.1\r\nHost: x\r\n\r\n").as_bytes())
                .await
                .unwrap();
            let mut out = String::new();
            s.read_to_string(&mut out).await.unwrap();
            out
        };
        assert!(get("/otra".into()).await.starts_with("HTTP/1.1 404"));
        let ok = get(format!("/{path}")).await;
        assert!(ok.contains("Set-Cookie: session_id=abc;"));
        assert!(ok.contains("Location: http://localhost:8069/web"));
    }
}
