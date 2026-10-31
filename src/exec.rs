//! Ejecución de comandos en esta máquina o dentro de un contenedor.
//!
//! A diferencia de `rdo`, nada pasa por `bash -c`: el programa y sus argumentos
//! van separados, así un nombre de BD no puede inyectar nada.

use std::{
    path::PathBuf,
    process::Stdio,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use anyhow::{Result, bail};
use tokio::{
    io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWriteExt, BufReader},
    process::Command,
};
use tokio_util::sync::CancellationToken;

use crate::jobs::Tx;

#[derive(Debug, Clone, Default)]
pub struct Spec {
    pub program: String,
    pub args: Vec<String>,
    envs: Vec<(String, String)>,
    container: Option<String>,
    /// Usuario dentro del contenedor (`root`, `1000:1000`…).
    user: Option<String>,
}

/// Lo que se manda por stdin.
#[derive(Debug, Clone)]
pub enum Input {
    /// Un archivo, con progreso (p. ej. `dump.sql`).
    File(PathBuf),
    /// Texto corto (p. ej. código para `odoo shell`).
    Bytes(Vec<u8>),
}

impl Spec {
    pub fn new(program: impl Into<String>) -> Self {
        Self {
            program: program.into(),
            ..Default::default()
        }
    }

    pub fn arg(mut self, a: impl Into<String>) -> Self {
        self.args.push(a.into());
        self
    }

    pub fn args<I, S>(mut self, xs: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.args.extend(xs.into_iter().map(Into::into));
        self
    }

    /// Variables de entorno; en docker se reenvían con `-e NOMBRE` para que el
    /// valor no aparezca en la lista de procesos.
    pub fn env(mut self, k: &str, v: &str) -> Self {
        if !v.is_empty() {
            self.envs.push((k.into(), v.into()));
        }
        self
    }

    /// Corre dentro del contenedor; vacío = en esta máquina.
    pub fn inside(mut self, container: &str) -> Self {
        self.container = (!container.is_empty()).then(|| container.to_string());
        self
    }

    pub fn root(self) -> Self {
        self.user("root")
    }

    pub fn user(mut self, user: &str) -> Self {
        self.user = Some(user.into());
        self
    }

    /// Comando de esta máquina que no muere con odotui (Odoo en segundo plano).
    pub fn detached(&self) -> std::process::Command {
        use std::os::unix::process::CommandExt;
        let mut cmd = std::process::Command::new(&self.program);
        cmd.args(&self.args)
            .envs(self.envs.iter().map(|(k, v)| (k, v)))
            .stdin(Stdio::null())
            // Grupo propio: un Ctrl+C en la terminal de odotui no le llega.
            .process_group(0);
        cmd
    }

    pub fn command(&self, stdin: bool) -> Command {
        let mut cmd = match &self.container {
            Some(c) => {
                let mut cmd = Command::new("docker");
                cmd.arg("exec");
                if stdin {
                    cmd.arg("-i");
                }
                if let Some(u) = &self.user {
                    cmd.args(["-u", u]);
                }
                for (k, _) in &self.envs {
                    cmd.args(["-e", k]);
                }
                cmd.arg(c).arg(&self.program).args(&self.args);
                cmd
            }
            None => {
                let mut cmd = Command::new(&self.program);
                cmd.args(&self.args);
                cmd
            }
        };
        cmd.envs(self.envs.iter().map(|(k, v)| (k, v)))
            .stdin(if stdin { Stdio::piped() } else { Stdio::null() })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        cmd
    }

    /// Para el log: el comando tal como se lanza (las contraseñas van por entorno).
    pub fn display(&self) -> String {
        let mut parts = vec![];
        if let Some(c) = &self.container {
            parts.push(format!("docker exec {c}"));
        }
        parts.push(self.program.clone());
        parts.extend(self.args.iter().map(|a| {
            if a.contains(' ') || a.is_empty() {
                format!("'{a}'")
            } else {
                a.clone()
            }
        }));
        parts.join(" ")
    }
}

fn clean(raw: &[u8]) -> String {
    String::from_utf8_lossy(raw)
        .trim_end_matches(['\n', '\r'])
        .chars()
        .filter(|c| *c == '\t' || !c.is_control())
        .collect()
}

fn not_found(spec: &Spec, e: std::io::Error) -> anyhow::Error {
    if e.kind() == std::io::ErrorKind::NotFound {
        let prog = if spec.container.is_some() {
            "docker"
        } else {
            &spec.program
        };
        anyhow::anyhow!("no encontré `{prog}` en el PATH")
    } else {
        e.into()
    }
}

/// Ejecuta y devuelve stdout; si falla, el error lleva el final de stderr.
pub async fn capture(spec: &Spec) -> Result<String> {
    let out = spec
        .command(false)
        .output()
        .await
        .map_err(|e| not_found(spec, e))?;
    if out.status.success() {
        return Ok(String::from_utf8_lossy(&out.stdout).into_owned());
    }
    let err = String::from_utf8_lossy(&out.stderr);
    let msg: Vec<&str> = err.lines().filter(|l| !l.trim().is_empty()).collect();
    let tail = msg[msg.len().saturating_sub(3)..].join(" · ");
    if tail.is_empty() {
        bail!("{} terminó con {}", spec.program, out.status)
    }
    bail!("{tail}")
}

async fn pump(r: impl AsyncRead + Unpin, tx: Tx, errors: Arc<AtomicUsize>) {
    let mut r = BufReader::new(r);
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match r.read_until(b'\n', &mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(_) => {
                let line = clean(&buf);
                if line.contains("ERROR") || line.contains("FATAL") {
                    errors.fetch_add(1, Ordering::Relaxed);
                }
                tx.log(line);
            }
        }
    }
}

/// Cómo detener el proceso al cancelar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stop {
    Kill,
    /// SIGINT y, si no termina en unos segundos, SIGKILL (Odoo cierra limpio).
    Interrupt,
}

/// Ejecuta mandando stdout y stderr al log, opcionalmente con algo por stdin
/// (un archivo, con progreso real). Devuelve cuántas líneas parecían errores.
pub async fn stream(
    spec: &Spec,
    stdin: Option<Input>,
    tx: &Tx,
    cancel: &CancellationToken,
    stop: Stop,
) -> Result<usize> {
    tx.log(format!("$ {}", spec.display()));
    let mut child = spec
        .command(stdin.is_some())
        .spawn()
        .map_err(|e| not_found(spec, e))?;
    let pid = child.id();
    let errors = Arc::new(AtomicUsize::new(0));
    let out = child
        .stdout
        .take()
        .map(|o| tokio::spawn(pump(o, tx.clone(), errors.clone())));
    let err = child
        .stderr
        .take()
        .map(|e| tokio::spawn(pump(e, tx.clone(), errors.clone())));
    let mut input = child.stdin.take();

    let finished = tokio::select! {
        r = async {
            match (stdin, input.take()) {
                (Some(Input::Bytes(b)), Some(mut w)) => {
                    // Si el proceso no lee todo, su salida dirá por qué.
                    let _ = w.write_all(&b).await;
                }
                (Some(Input::File(path)), Some(mut w)) => {
                let total = tokio::fs::metadata(&path).await?.len().max(1);
                let mut file = tokio::fs::File::open(&path).await?;
                let mut chunk = vec![0u8; 256 * 1024];
                let (mut sent, mut last) = (0u64, -1i64);
                loop {
                    let n = file.read(&mut chunk).await?;
                    if n == 0 {
                        break;
                    }
                    if w.write_all(&chunk[..n]).await.is_err() {
                        // El proceso cerró su entrada (murió); su salida dirá por qué.
                        break;
                    }
                    sent += n as u64;
                    let pct = (sent * 1000 / total) as i64;
                    if pct != last {
                        last = pct;
                        tx.progress(sent as f64 / total as f64);
                    }
                }
                }
                _ => {}
            }
            // La entrada ya se cerró (al soltar `w`): el proceso ve EOF.
            anyhow::Ok(child.wait().await?)
        } => Some(r),
        _ = cancel.cancelled() => None,
    };

    let status = match finished {
        Some(r) => r?,
        None => {
            if stop == Stop::Interrupt
                && let Some(pid) = pid
            {
                // SAFETY: kill(2) con un pid de un hijo propio no toca memoria.
                unsafe { libc::kill(pid as i32, libc::SIGINT) };
                if tokio::time::timeout(Duration::from_secs(8), child.wait())
                    .await
                    .is_ok()
                {
                    drain(out, err).await;
                    bail!("detenido");
                }
            }
            let _ = child.kill().await;
            drain(out, err).await;
            bail!("cancelado");
        }
    };
    drain(out, err).await;
    if !status.success() {
        bail!("{} terminó con {status}", spec.program);
    }
    Ok(errors.load(Ordering::Relaxed))
}

async fn drain(out: Option<tokio::task::JoinHandle<()>>, err: Option<tokio::task::JoinHandle<()>>) {
    for h in [out, err].into_iter().flatten() {
        let _ = tokio::time::timeout(Duration::from_secs(2), h).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn docker_exec_forwards_env_by_name() {
        let s = Spec::new("psql")
            .args(["-d", "x y"])
            .env("PGPASSWORD", "secreto")
            .inside("db1");
        let cmd = s.command(true);
        let args: Vec<_> = cmd
            .as_std()
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            args,
            ["exec", "-i", "-e", "PGPASSWORD", "db1", "psql", "-d", "x y"]
        );
        assert!(!s.display().contains("secreto"));
        assert_eq!(s.display(), "docker exec db1 psql -d 'x y'");
    }

    #[test]
    fn host_spec_has_no_docker() {
        let s = Spec::new("psql").arg("-l").inside("");
        assert_eq!(s.command(false).as_std().get_program(), "psql");
    }

    #[tokio::test]
    async fn stream_feeds_stdin_and_counts_errors() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let tx = Tx::new(tx, crate::jobs::Sink::Restore);
        let dir = std::env::temp_dir().join(format!("odotui-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("in.txt");
        std::fs::write(&file, "hola\nERROR: algo\n").unwrap();
        let n = stream(
            &Spec::new("cat"),
            Some(Input::File(file)),
            &tx,
            &CancellationToken::new(),
            Stop::Kill,
        )
        .await
        .unwrap();
        assert_eq!(n, 1);
        drop(tx);
        let mut lines = vec![];
        while let Some(ev) = rx.recv().await {
            if let crate::jobs::Ev::Log(_, l) = ev {
                lines.push(l);
            }
        }
        assert!(lines.contains(&"hola".to_string()));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn cancel_kills_child() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let tx = Tx::new(tx, crate::jobs::Sink::Instance);
        let cancel = CancellationToken::new();
        let c2 = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            c2.cancel();
        });
        let r = stream(
            &Spec::new("sleep").arg("30"),
            None,
            &tx,
            &cancel,
            Stop::Interrupt,
        )
        .await;
        assert!(r.unwrap_err().to_string().contains("detenido"));
    }
}
