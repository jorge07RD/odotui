//! Portapapeles: `wl-copy` si hay Wayland; si no, OSC 52 a través de la terminal
//! (funciona también por ssh). Igual que en tuisentry.

use std::{
    io::{self, Write},
    process::{Command, Stdio},
};

use base64::{Engine, engine::general_purpose::STANDARD};

/// Copia y devuelve el error, si lo hubo, para el toast.
pub fn copy(text: &str) -> Result<(), String> {
    // Sin sesión de Wayland, `wl-copy` arranca pero no copia nada.
    if std::env::var_os("WAYLAND_DISPLAY").is_some()
        && let Ok(mut child) = Command::new("wl-copy")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
    {
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(text.as_bytes());
        }
        std::thread::spawn(move || child.wait());
        return Ok(());
    }
    let mut out = io::stdout();
    write!(out, "\x1b]52;c;{}\x07", STANDARD.encode(text))
        .and_then(|_| out.flush())
        .map_err(|e| e.to_string())
}

/// Aviso del escritorio (`notify-send`); si no está, no pasa nada.
pub fn notify(summary: &str, body: &str) {
    let _ = Command::new("notify-send")
        .args(["-a", "odotui", summary, body])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|mut c| std::thread::spawn(move || c.wait()));
}
