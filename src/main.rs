//! odotui: herramientas de desarrollo para Odoo en la terminal, usables
//! igual con el mouse que con el teclado.

use std::{
    io::stdout,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use anyhow::Result;
use ratatui::crossterm::{
    event::{
        self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
    },
    execute,
};
use tokio::sync::mpsc;

use odotui::{
    ai,
    app::{App, Effect},
    config,
    config::Config,
    exec::Spec,
    input::HitMap,
    ui,
};

const USAGE: &str = "\
odotui — herramientas de desarrollo para Odoo

uso: odotui [mcp] [-c config.toml] [--init]

  mcp                 servidor MCP por stdio para que una IA maneje odotui
                      (si la TUI está abierta, lo hace en ella y se ve en vivo)
  -c, --config RUTA   config a usar (por defecto ~/.config/odotui/config.toml)
      --init          escribe una config de ejemplo y sale
  -h, --help          esta ayuda";

#[tokio::main]
async fn main() -> Result<()> {
    let mut cfg_path = config::default_path();
    let mut init = false;
    let mut mcp = false;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "-c" | "--config" => match args.next() {
                Some(p) => cfg_path = PathBuf::from(p),
                None => anyhow::bail!("falta la ruta tras {a}"),
            },
            "--init" => init = true,
            "mcp" => mcp = true,
            "-h" | "--help" => {
                println!("{USAGE}");
                return Ok(());
            }
            other => anyhow::bail!("argumento desconocido: {other}\n\n{USAGE}"),
        }
    }
    if init {
        if cfg_path.exists() {
            anyhow::bail!("{} ya existe", cfg_path.display());
        }
        if let Some(dir) = cfg_path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&cfg_path, include_str!("../config.example.toml"))?;
        println!("config de ejemplo en {}", cfg_path.display());
        return Ok(());
    }
    if mcp {
        // stdout es del protocolo: nada más puede escribir ahí.
        return ai::mcp::serve(cfg_path).await;
    }
    let cfg = Config::load(&cfg_path)?;

    let (job_tx, mut jobs) = mpsc::unbounded_channel();
    // Mientras la TUI esté abierta, la IA trabaja a través de ella.
    let bridge = ai::bridge::serve(job_tx.clone()).await;
    let mut app = App::new(cfg, cfg_path, job_tx);
    match &bridge {
        Ok(_) => app.ai_listening = true,
        Err(e) => app.toast(format!("IA: {e}")),
    }

    let mut terminal = ratatui::init();
    execute!(stdout(), EnableMouseCapture, EnableBracketedPaste)?;
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = execute!(stdout(), DisableMouseCapture, DisableBracketedPaste);
        hook(info);
    }));

    // crossterm bloquea al leer; un hilo aparte reenvía los eventos. `reading`
    // lo pausa mientras otro programa (odoo shell, psql) usa la terminal.
    let reading = Arc::new(AtomicBool::new(true));
    let (ev_tx, mut events) = mpsc::unbounded_channel();
    let reader = reading.clone();
    std::thread::spawn(move || {
        loop {
            if !reader.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(50));
                continue;
            }
            match event::poll(Duration::from_millis(100)) {
                Ok(true) => {
                    let Ok(ev) = event::read() else { break };
                    if ev_tx.send(ev).is_err() {
                        break;
                    }
                }
                Ok(false) => {}
                Err(_) => break,
            }
        }
    });

    app.start();
    // Spinner, toasts que caducan y sondeo del estado de la instancia.
    let mut tick = tokio::time::interval(Duration::from_millis(250));
    let result: Result<()> = loop {
        let mut hits = HitMap::default();
        if let Err(e) = terminal.draw(|f| ui::draw(f, &app, &mut hits)) {
            break Err(e.into());
        }
        app.hits = hits;

        tokio::select! {
            Some(ev) = events.recv() => app.on_event(ev),
            Some(ev) = jobs.recv() => app.on_ev(ev),
            _ = tick.tick() => app.on_tick(),
        }
        // Agrupa lo que haya llegado mientras tanto en un solo redibujo.
        while let Ok(ev) = events.try_recv() {
            app.on_event(ev);
        }
        while let Ok(ev) = jobs.try_recv() {
            app.on_ev(ev);
        }
        if app.quit {
            break Ok(());
        }
        for effect in std::mem::take(&mut app.effects) {
            match effect {
                Effect::Interactive { spec, label } => {
                    let r = interactive(&spec, &label, &mut terminal, &reading).await;
                    app.toast(match r {
                        Ok(()) => format!("volviste de {label}"),
                        Err(e) => format!("✗ {label}: {e:#}"),
                    });
                    // Lo llegado mientras tanto (teclas de la shell) no es para la TUI.
                    while events.try_recv().is_ok() {}
                }
            }
        }
    };

    execute!(stdout(), DisableMouseCapture, DisableBracketedPaste)?;
    ratatui::restore();
    drop(bridge);
    result
}

extern "C" fn ignore(_: libc::c_int) {}

/// Suspende la TUI, corre `spec` con la terminal heredada y vuelve.
async fn interactive(
    spec: &Spec,
    label: &str,
    terminal: &mut ratatui::DefaultTerminal,
    reading: &AtomicBool,
) -> Result<()> {
    reading.store(false, Ordering::Relaxed);
    // Da tiempo al hilo lector a salir de su `poll`.
    tokio::time::sleep(Duration::from_millis(150)).await;
    execute!(stdout(), DisableMouseCapture, DisableBracketedPaste)?;
    ratatui::restore();
    println!("── {label} ── (sal con Ctrl+D o exit() para volver a odotui)\n");

    // Ctrl+C es para el programa, no para odotui. Un manejador vacío (y no
    // SIG_IGN) porque el hijo lo hereda como el de por defecto.
    // SAFETY: sigaction con un manejador que no hace nada; se restaura después.
    let old = unsafe {
        let mut sa: libc::sigaction = std::mem::zeroed();
        sa.sa_sigaction = ignore as *const () as usize;
        let mut old: libc::sigaction = std::mem::zeroed();
        libc::sigaction(libc::SIGINT, &sa, &mut old);
        old
    };
    let status = tokio::process::Command::from(spec.interactive())
        .status()
        .await;
    // SAFETY: vuelve el manejador que había.
    unsafe { libc::sigaction(libc::SIGINT, &old, std::ptr::null_mut()) };

    *terminal = ratatui::init();
    execute!(stdout(), EnableMouseCapture, EnableBracketedPaste)?;
    terminal.clear()?;
    reading.store(true, Ordering::Relaxed);
    let status = status.map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            anyhow::anyhow!("no encontré `{}`", spec.program)
        } else {
            e.into()
        }
    })?;
    // Salir de psql con un error de la última orden no es un fallo de odotui.
    if !status.success() && status.code().is_none() {
        anyhow::bail!("terminó con {status}");
    }
    Ok(())
}
