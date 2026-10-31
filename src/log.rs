//! Estado de los logs (restauración, instancia, módulos…) y de las listas.

use std::{collections::VecDeque, ops::Range};

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
    } else if line.starts_with("Traceback")
        || line.trim_start().starts_with("File \"")
        || is_exception(line)
    {
        Level::Error
    } else {
        Level::Plain
    }
}

/// Para buscar: minúsculas y sin tildes («Módulos» ≈ «modulos»).
pub fn fold(s: &str) -> String {
    s.chars()
        .flat_map(char::to_lowercase)
        .map(|c| match c {
            'á' | 'à' | 'â' | 'ä' => 'a',
            'é' | 'è' | 'ê' | 'ë' => 'e',
            'í' | 'ì' | 'î' | 'ï' => 'i',
            'ó' | 'ò' | 'ô' | 'ö' => 'o',
            'ú' | 'ù' | 'û' | 'ü' => 'u',
            'ñ' => 'n',
            c => c,
        })
        .collect()
}

/// `ValueError: …`, `odoo.exceptions.UserError: …`: la última línea de un traceback.
pub fn is_exception(line: &str) -> bool {
    let Some((name, _)) = line.split_once(": ") else {
        return false;
    };
    !name.contains(' ')
        && ["Error", "Exception", "Warning", "Interrupt"]
            .iter()
            .any(|s| name.ends_with(s))
}

/// Cada error con su traceback: un `ERROR`/`CRITICAL` (o un `Traceback`
/// suelto) más las líneas con sangría y la excepción final. Lo demás sin fecha
/// (p. ej. de psql) no cuenta. Devuelve los rangos de líneas.
pub fn error_blocks<S: AsRef<str>>(lines: &[S]) -> Vec<Range<usize>> {
    let starts_entry = |l: &str| l.len() > 4 && l.as_bytes()[..4].iter().all(u8::is_ascii_digit);
    let mut out = vec![];
    let mut i = 0;
    while i < lines.len() {
        let l = lines[i].as_ref();
        if !(matches!(level(l), Level::Error) && !l.starts_with('✗')) {
            i += 1;
            continue;
        }
        let mut j = i + 1;
        let mut tb = l.starts_with("Traceback");
        while j < lines.len() && !starts_entry(lines[j].as_ref()) && j - i <= 40 {
            let l = lines[j].as_ref();
            if !(l.starts_with("Traceback") || tb) {
                break;
            }
            tb = true;
            j += 1;
            if !l.starts_with(' ') && !l.starts_with("Traceback") {
                break;
            }
        }
        out.push(i..j);
        i = j;
    }
    out
}

#[derive(Debug, Clone)]
pub struct LogView {
    lines: VecDeque<String>,
    /// Primera línea visible (sobre las filtradas) cuando no se sigue el final.
    pub offset: usize,
    /// Pegado al final: las líneas nuevas se ven solas.
    pub follow: bool,
    pub filter: String,
    /// Solo los errores con sus tracebacks.
    pub errors_only: bool,
}

impl Default for LogView {
    fn default() -> Self {
        Self {
            lines: VecDeque::new(),
            offset: 0,
            follow: true,
            filter: String::new(),
            errors_only: false,
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
        self.visible_indexed().into_iter().map(|(_, l)| l).collect()
    }

    /// Las líneas visibles con su posición en el log completo.
    pub fn visible_indexed(&self) -> Vec<(usize, &str)> {
        let f = fold(&self.filter);
        let keep = self.errors_only.then(|| self.blocks());
        self.lines
            .iter()
            .enumerate()
            .filter(|(i, _)| {
                keep.as_ref()
                    .is_none_or(|b| b.iter().any(|r| r.contains(i)))
            })
            .filter(|(_, l)| f.is_empty() || fold(l).contains(&f))
            .map(|(i, l)| (i, l.as_str()))
            .collect()
    }

    pub fn blocks(&self) -> Vec<Range<usize>> {
        let (a, b) = self.lines.as_slices();
        if b.is_empty() {
            error_blocks(a)
        } else {
            error_blocks(&self.lines.iter().collect::<Vec<_>>())
        }
    }

    /// El texto del bloque de error que contiene la línea `i`.
    pub fn block_at(&self, i: usize) -> Option<String> {
        let r = self.blocks().into_iter().find(|r| r.contains(&i))?;
        Some(self.text(r))
    }

    /// El último error con su traceback.
    pub fn last_block(&self) -> Option<String> {
        let r = self.blocks().pop()?;
        Some(self.text(r))
    }

    fn text(&self, r: Range<usize>) -> String {
        self.lines
            .range(r)
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Mueve el scroll; llegar al final vuelve a seguir.
    pub fn scroll(&mut self, delta: i32, max: usize) {
        let cur = if self.follow {
            max
        } else {
            self.offset.min(max)
        };
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
        self.filter.is_empty() || fold(text).contains(&fold(&self.filter))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_take_the_whole_traceback() {
        let mut v = LogView::default();
        for l in [
            "2025-01-01 10:00:00,000 1 INFO db odoo: inicio",
            "2025-01-01 10:00:01,000 1 ERROR db odoo.sql_db: bad query",
            "Traceback (most recent call last):",
            "  File \"/x.py\", line 3, in f",
            "    g()",
            "psycopg2.errors.UndefinedTable: relation \"x\" does not exist",
            "2025-01-01 10:00:02,000 1 INFO db odoo: sigue",
            "Traceback (most recent call last):",
            "  File \"<console>\", line 1, in <module>",
            "NameError: name 'y' is not defined",
        ] {
            v.push(l.into());
        }
        assert_eq!(v.blocks(), [1..6, 7..10]);
        assert!(v.last_block().unwrap().starts_with("Traceback"));
        assert!(v.block_at(3).unwrap().ends_with("does not exist"));
        assert_eq!(v.block_at(0), None);
        v.errors_only = true;
        assert_eq!(v.visible().len(), 8);
        assert_eq!(v.visible_indexed()[4].0, 5);
    }

    #[test]
    fn odoo_levels() {
        let l = "2025-04-28 10:00:01,123 7 INFO prod odoo.modules.loading: loading 1 modules...";
        assert_eq!(level(l), Level::Info);
        let w = "2025-04-28 10:00:01,123 7 WARNING prod odoo.models: algo";
        assert_eq!(level(w), Level::Warn);
        let e = "2025-04-28 10:00:01,123 7 ERROR prod odoo.sql_db: bad query";
        assert_eq!(level(e), Level::Error);
        assert_eq!(
            level("psql:<stdin>:12: ERROR:  must be owner of extension"),
            Level::Error
        );
        assert_eq!(level("$ psql -X"), Level::Cmd);
        assert_eq!(level("▶ 1/4 Preparando"), Level::Step);
        // «INFO» en el mensaje, no en la cabecera, no cuenta… salvo al principio.
        assert_eq!(level("hola mundo"), Level::Plain);
        assert_eq!(level("  File \"x.py\", line 1"), Level::Error);
        assert_eq!(level("ValueError: boom"), Level::Error);
        assert_eq!(level("odoo.exceptions.UserError: no"), Level::Error);
        assert_eq!(level("nota: nada que ver"), Level::Plain);
        assert_eq!(fold("Ir a Módulos ÑU"), "ir a modulos nu");
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
