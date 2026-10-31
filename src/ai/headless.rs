//! Las herramientas sin la TUI: mismo núcleo, estado mínimo en el proceso.

use std::{path::PathBuf, sync::Mutex};

use serde_json::Value;
use tokio::sync::mpsc::UnboundedSender;
use tokio_util::sync::CancellationToken;

use crate::{
    ai::tools,
    config::{Config, Profile},
    history::{self, Entry},
    jobs::{Progress, Tx},
};

#[derive(Default)]
struct State {
    /// Perfil elegido con `switch_profile` (si no, el por defecto).
    current: Option<String>,
    busy: Option<(String, CancellationToken)>,
}

pub struct Headless {
    cfg_path: PathBuf,
    state: Mutex<State>,
}

/// Índice del perfil pedido en `args.profile`, o el actual.
pub fn resolve(
    profiles: &[Profile],
    current: Option<usize>,
    args: &Value,
) -> Result<usize, String> {
    match tools::opt_str(args, "profile").filter(|s| !s.is_empty()) {
        Some(name) => profiles
            .iter()
            .position(|p| p.name == name)
            .ok_or_else(|| format!("no existe el perfil «{name}»")),
        None => current
            .filter(|&i| i < profiles.len())
            .ok_or_else(|| "no hay perfiles: créalo en la TUI (pestaña Perfiles)".into()),
    }
}

impl Headless {
    pub fn new(cfg_path: PathBuf) -> Self {
        Self {
            cfg_path,
            state: Mutex::new(State::default()),
        }
    }

    /// La config se relee en cada llamada: así se ven los perfiles editados en la TUI.
    fn load(&self) -> Result<(Vec<Profile>, Option<usize>), String> {
        let cfg = Config::load(&self.cfg_path).map_err(|e| format!("{e:#}"))?;
        let st = self.state.lock().map_err(|_| "estado bloqueado")?;
        let current = st
            .current
            .as_ref()
            .and_then(|n| cfg.profiles.iter().position(|p| &p.name == n))
            .or_else(|| (!cfg.profiles.is_empty()).then(|| cfg.default_index()));
        Ok((cfg.profiles, current))
    }

    pub async fn call(
        &self,
        tool: &str,
        args: &Value,
        progress: Option<UnboundedSender<Progress>>,
    ) -> Result<String, String> {
        let (profiles, current) = self.load()?;
        match tool {
            "status" => {
                let busy = self
                    .state
                    .lock()
                    .ok()
                    .and_then(|s| s.busy.as_ref().map(|b| b.0.clone()));
                return Ok(tools::describe(&profiles, current, busy.as_deref(), false).await);
            }
            "switch_profile" => {
                let i = resolve(&profiles, current, args)?;
                if let Ok(mut s) = self.state.lock() {
                    s.current = Some(profiles[i].name.clone());
                }
                return Ok(format!("perfil activo: {}", profiles[i].name));
            }
            "show" => {
                return Ok(
                    "la TUI no está abierta; si el usuario abre `odotui` verá los cambios".into(),
                );
            }
            "cancel" => {
                let st = self.state.lock().map_err(|_| "estado bloqueado")?;
                return match &st.busy {
                    Some((label, c)) => {
                        c.cancel();
                        Ok(format!("cancelando: {label}"))
                    }
                    None => Ok("no hay nada en curso".into()),
                };
            }
            _ => {}
        }
        let p = profiles[resolve(&profiles, current, args)?].clone();
        if tools::sink(tool).is_none() {
            return tools::call(tool, args, &p, &Tx::detached(), &CancellationToken::new()).await;
        }

        // Una sola tarea de escritura a la vez, igual que en la TUI.
        let cancel = CancellationToken::new();
        {
            let mut st = self.state.lock().map_err(|_| "estado bloqueado")?;
            if let Some((label, _)) = &st.busy {
                return Err(format!("ocupado: {label} en curso (usa cancel o espera)"));
            }
            st.busy = Some((tool.to_string(), cancel.clone()));
        }
        let tx = Tx::detached().reporting(progress);
        let r = tools::call(tool, args, &p, &tx, &cancel).await;
        if let Ok(mut st) = self.state.lock() {
            st.busy = None;
        }
        if tool == "restore_backup" {
            let mut h = history::load();
            history::push(
                &mut h,
                Entry {
                    at: chrono::Local::now(),
                    profile: p.name.clone(),
                    backup: tools::opt_str(args, "backup").unwrap_or_default(),
                    db: tools::opt_str(args, "database").unwrap_or_default(),
                    ok: r.is_ok(),
                    message: format!("(IA) {}", first_line(&r)),
                },
            );
        }
        r
    }
}

pub fn first_line(r: &Result<String, String>) -> String {
    match r {
        Ok(s) | Err(s) => s.lines().next().unwrap_or_default().to_string(),
    }
}
