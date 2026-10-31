//! Estado de los logs (restauración, instancia, módulos…) y de las listas.

use std::collections::VecDeque;

const MAX_LINES: usize = 5000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Error,
    Warn,
    Info,
    Debug,
    /// `$ comando`
    Cmd,
    /// `▶ paso`
    Step,
    Plain,
}

/// Nivel de una línea de Odoo (`… 1 INFO db odoo.modules: …`), de psql o propia.
pub fn level(line: &str) -> Level {
    if line.starts_with("$ ") {
        return Level::Cmd;
    }
    if line.starts_with('▶') {
        return Level::Step;
    }
    if line.starts_with('⚠') {
        return Level::Warn;
    }
    if line.starts_with('✗') {
        return Level::Error;
    }
    // Odoo: fecha hora pid NIVEL …; se mira solo el principio.
    let head: String = line.chars().take(48).collect();
    let has = |w: &str| head.split_whitespace().any(|t| t == w);
    if has("ERROR") || has("CRITICAL") || head.contains("ERROR:") || head.contains("FATAL:") {
        Level::Error
    } else if has("WARNING") || head.contains("WARNING:") {
        Level::Warn
    } else if has("INFO") || head.contains("NOTICE:") {
        Level::Info
    } else if has("DEBUG") {
        Level::Debug
    } else if line.starts_with("Traceback") || line.starts_with("    ") && line.contains("File \"") {
        Level::Error
    } else {
        Level::Plain
    }
}

#[derive(Debug, Clone)]
pub struct LogView {
    lines: VecDeque<String>,
    /// Primera línea visible (sobre las filtradas) cuando no se sigue el final.
    pub offset: usize,
    /// Pegado al final: las líneas nuevas se ven solas.
    pub follow: bool,
    pub filter: String,
}

impl Default for LogView {
    fn default() -> Self {
        Self {
            lines: VecDeque::new(),
            offset: 0,
            follow: true,
            filter: String::new(),
        }
    }
}

impl LogView {
    pub fn push(&mut self, line: String) {
        if self.lines.len() == MAX_LINES {
            self.lines.pop_front();
            self.offset = self.offset.saturating_sub(1);
        }
        self.lines.push_back(line);
    }

    pub fn clear(&mut self) {
        self.lines.clear();
        self.offset = 0;
        self.follow = true;
    }

    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    pub fn visible(&self) -> Vec<&str> {
        let f = self.filter.to_lowercase();
        self.lines
            .iter()
            .filter(|l| f.is_empty() || l.to_lowercase().contains(&f))
            .map(String::as_str)
            .collect()
    }

    /// Mueve el scroll; llegar al final vuelve a seguir.
    pub fn scroll(&mut self, delta: i32, max: usize) {
        let cur = if self.follow { max } else { self.offset.min(max) };
        let next = (cur as i64 + i64::from(delta)).clamp(0, max as i64) as usize;
        self.offset = next;
        self.follow = next >= max;
    }
}

/// Selección, scroll y filtro de una lista.
#[derive(Debug, Clone, Default)]
pub struct ListState {
    pub sel: usize,
    pub offset: usize,
    pub filter: String,
}

impl ListState {
    /// Mueve la selección y la mantiene a la vista (`rows` = filas visibles).
    pub fn step(&mut self, delta: i32, len: usize, rows: usize) {
        if len == 0 {
            self.sel = 0;
            return;
        }
        self.sel = (self.sel as i64 + i64::from(delta)).clamp(0, len as i64 - 1) as usize;
        self.reveal(rows);
    }

    pub fn reveal(&mut self, rows: usize) {
        let rows = rows.max(1);
        if self.sel < self.offset {
            self.offset = self.sel;
        } else if self.sel >= self.offset + rows {
            self.offset = self.sel + 1 - rows;
        }
    }

    pub fn clamp(&mut self, len: usize) {
        self.sel = self.sel.min(len.saturating_sub(1));
    }

    pub fn matches(&self, text: &str) -> bool {
        self.filter.is_empty() || text.to_lowercase().contains(&self.filter.to_lowercase())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn odoo_levels() {
        let l = "2025-04-28 10:00:01,123 7 INFO prod odoo.modules.loading: loading 1 modules...";
        assert_eq!(level(l), Level::Info);
        let w = "2025-04-28 10:00:01,123 7 WARNING prod odoo.models: algo";
        assert_eq!(level(w), Level::Warn);
        let e = "2025-04-28 10:00:01,123 7 ERROR prod odoo.sql_db: bad query";
        assert_eq!(level(e), Level::Error);
        assert_eq!(level("psql:<stdin>:12: ERROR:  must be owner of extension"), Level::Error);
        assert_eq!(level("$ psql -X"), Level::Cmd);
        assert_eq!(level("▶ 1/4 Preparando"), Level::Step);
        // «INFO» en el mensaje, no en la cabecera, no cuenta… salvo al principio.
        assert_eq!(level("hola mundo"), Level::Plain);
    }

    #[test]
    fn follow_and_scroll() {
        let mut v = LogView::default();
        for i in 0..20 {
            v.push(format!("l{i}"));
        }
        v.scroll(-3, 10);
        assert!(!v.follow);
        assert_eq!(v.offset, 7);
        v.scroll(5, 10);
        assert!(v.follow);
        v.filter = "l1".into();
        assert_eq!(v.visible().len(), 11);
    }

    #[test]
    fn list_keeps_selection_visible() {
        let mut s = ListState::default();
        s.step(7, 10, 5);
        assert_eq!((s.sel, s.offset), (7, 3));
        s.step(-100, 10, 5);
        assert_eq!((s.sel, s.offset), (0, 0));
        s.step(100, 10, 5);
        assert_eq!(s.sel, 9);
    }
}
