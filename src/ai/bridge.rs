//! Socket local entre `odotui mcp` y la TUI abierta: si la TUI escucha, cada
//! herramienta corre dentro de ella y el usuario la ve en vivo.

use std::path::PathBuf;

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::{UnixListener, UnixStream},
    sync::{mpsc::UnboundedSender, oneshot},
};

use crate::jobs::{AiRequest, Ev, Progress};

#[derive(Serialize, Deserialize)]
struct Req {
    tool: String,
    args: Value,
    /// Mandar el avance antes de la respuesta.
    #[serde(default)]
    progress: bool,
}

#[derive(Serialize, Deserialize)]
struct Resp {
    ok: bool,
    text: String,
}

/// Lo que la TUI manda de vuelta: avances (si se pidieron) y al final la respuesta.
#[derive(Serialize, Deserialize)]
#[serde(untagged)]
enum Frame {
    Resp(Resp),
    Progress { progress: Progress },
}

async fn write_frame(w: &mut (impl AsyncWriteExt + Unpin), f: &Frame) -> std::io::Result<()> {
    let mut out = serde_json::to_string(f).unwrap_or_default();
    out.push('\n');
    w.write_all(out.as_bytes()).await
}

/// `$XDG_RUNTIME_DIR/odotui.sock` (solo accesible por el usuario).
pub fn socket_path() -> PathBuf {
    match dirs::runtime_dir() {
        Some(d) => d.join("odotui.sock"),
        None => {
            // SAFETY: getuid no falla ni toca memoria.
            let uid = unsafe { libc::getuid() };
            std::env::temp_dir().join(format!("odotui-{uid}.sock"))
        }
    }
}

/// Borra el socket al cerrar la TUI.
pub struct Guard(PathBuf);

impl Drop for Guard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Escucha pedidos y los pasa a la TUI como `Ev::Ai`.
pub async fn serve(events: UnboundedSender<Ev>) -> Result<Guard> {
    let path = socket_path();
    if path.exists() {
        if UnixStream::connect(&path).await.is_ok() {
            bail!("otra odotui ya atiende a la IA");
        }
        // Quedó de una ejecución que no cerró bien.
        let _ = std::fs::remove_file(&path);
    }
    let listener = UnixListener::bind(&path)?;
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            tokio::spawn(handle(stream, events.clone()));
        }
    });
    Ok(Guard(path))
}

async fn handle(stream: UnixStream, events: UnboundedSender<Ev>) {
    let (r, mut w) = stream.into_split();
    let mut lines = BufReader::new(r).lines();
    let Ok(Some(line)) = lines.next_line().await else {
        return;
    };
    let resp = match serde_json::from_str::<Req>(&line) {
        Ok(req) => {
            let (reply, mut rx) = oneshot::channel();
            let (ptx, mut prx) = tokio::sync::mpsc::unbounded_channel();
            let sent = events.send(Ev::Ai(AiRequest {
                tool: req.tool,
                args: req.args,
                reply,
                progress: req.progress.then_some(ptx),
            }));
            // El avance sale mientras se espera la respuesta.
            let r = loop {
                tokio::select! {
                    Some(p) = prx.recv() => {
                        let _ = write_frame(&mut w, &Frame::Progress { progress: p }).await;
                    }
                    r = &mut rx => break r,
                }
            };
            match (sent, r) {
                (Ok(()), Ok(r)) => match r {
                    Ok(text) => Resp { ok: true, text },
                    Err(text) => Resp { ok: false, text },
                },
                _ => Resp {
                    ok: false,
                    text: "la TUI se cerró antes de terminar".into(),
                },
            }
        }
        Err(e) => Resp {
            ok: false,
            text: format!("pedido inválido: {e}"),
        },
    };
    let _ = write_frame(&mut w, &Frame::Resp(resp)).await;
}

/// Manda la herramienta a la TUI; `None` si no hay TUI escuchando. El avance,
/// si se pide, llega por `progress`.
pub async fn request(
    tool: &str,
    args: &Value,
    progress: Option<UnboundedSender<Progress>>,
) -> Option<Result<String, String>> {
    let stream = UnixStream::connect(socket_path()).await.ok()?;
    let (r, mut w) = stream.into_split();
    let mut line = serde_json::to_string(&Req {
        tool: tool.into(),
        args: args.clone(),
        progress: progress.is_some(),
    })
    .ok()?;
    line.push('\n');
    if w.write_all(line.as_bytes()).await.is_err() {
        return None;
    }
    let mut lines = BufReader::new(r).lines();
    loop {
        return Some(match lines.next_line().await {
            Ok(Some(l)) => match serde_json::from_str::<Frame>(&l) {
                Ok(Frame::Progress { progress: p }) => {
                    if let Some(tx) = &progress {
                        let _ = tx.send(p);
                    }
                    continue;
                }
                Ok(Frame::Resp(Resp { ok: true, text })) => Ok(text),
                Ok(Frame::Resp(Resp { ok: false, text })) => Err(text),
                Err(e) => Err(format!("respuesta inválida de la TUI: {e}")),
            },
            _ => Err("la TUI se cerró antes de responder".into()),
        });
    }
}
