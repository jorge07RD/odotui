//! odotui: herramientas de desarrollo para Odoo en la terminal, usables
//! igual con el mouse que con el teclado.

mod app;
mod config;
mod exec;
mod form;
mod history;
mod input;
mod jobs;
mod log;
mod odoo;
mod theme;
mod ui;

use std::{io::stdout, path::PathBuf, time::Duration};

use anyhow::Result;
use ratatui::crossterm::{
    event::{self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture},
    execute,
};
use tokio::sync::mpsc;

use crate::{app::App, config::Config, input::HitMap};

const USAGE: &str = "\
odotui — herramientas de desarrollo para Odoo

uso: odotui [-c config.toml] [--init]

  -c, --config RUTA   config a usar (por defecto ~/.config/odotui/config.toml)
      --init          escribe una config de ejemplo y sale
  -h, --help          esta ayuda";

#[tokio::main]
async fn main() -> Result<()> {
    let mut cfg_path = config::default_path();
    let mut init = false;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "-c" | "--config" => match args.next() {
                Some(p) => cfg_path = PathBuf::from(p),
                None => anyhow::bail!("falta la ruta tras {a}"),
            },
            "--init" => init = true,
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
    let cfg = Config::load(&cfg_path)?;

    let (job_tx, mut jobs) = mpsc::unbounded_channel();
    let mut app = App::new(cfg, cfg_path, job_tx);

    let mut terminal = ratatui::init();
    execute!(stdout(), EnableMouseCapture, EnableBracketedPaste)?;
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = execute!(stdout(), DisableMouseCapture, DisableBracketedPaste);
        hook(info);
    }));

    // crossterm bloquea al leer; un hilo aparte reenvía los eventos.
    let (ev_tx, mut events) = mpsc::unbounded_channel();
    std::thread::spawn(move || {
        loop {
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
    };

    execute!(stdout(), DisableMouseCapture, DisableBracketedPaste)?;
    ratatui::restore();
    result
}
