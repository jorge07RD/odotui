//! Favoritas y último uso de cada BD, por perfil, en
//! `~/.local/state/odotui/dbs.json`. La lista de BDs pone primero las
//! favoritas y luego la más reciente, así lo que más usas queda arriba.
//!
//! Lo escriben igual la TUI y la IA (MCP): cada cambio relee el archivo antes
//! de escribir para no pisar lo del otro.

use std::{
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Use {
    pub at: DateTime<Local>,
    /// Qué se hizo: «conectar», «odoo shell», «actualizar módulos»…
    pub what: String,
}

/// Lo de un perfil.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Marks {
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub favorites: BTreeSet<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub used: BTreeMap<String, Use>,
}

impl Marks {
    pub fn is_favorite(&self, db: &str) -> bool {
        self.favorites.contains(db)
    }

    pub fn last_use(&self, db: &str) -> Option<&Use> {
        self.used.get(db)
    }

    /// Anota que se usó `db` para `what`.
    pub fn touch(&mut self, db: &str, what: &str) {
        self.used.insert(
            db.to_string(),
            Use {
                at: Local::now(),
                what: what.to_string(),
            },
        );
    }

    /// Marca o desmarca `db`; devuelve si quedó marcada.
    pub fn toggle_favorite(&mut self, db: &str) -> bool {
        let on = !self.favorites.remove(db);
        if on {
            self.favorites.insert(db.to_string());
        }
        on
    }

    /// La BD cambió de nombre: sus marcas la siguen.
    pub fn rename(&mut self, from: &str, to: &str) {
        if self.favorites.remove(from) {
            self.favorites.insert(to.to_string());
        }
        if let Some(u) = self.used.remove(from) {
            self.used.insert(to.to_string(), u);
        }
    }

    /// La BD ya no existe.
    pub fn forget(&mut self, db: &str) {
        self.favorites.remove(db);
        self.used.remove(db);
    }

    /// Favoritas primero, luego la de uso más reciente, luego por nombre.
    pub fn order(&self, a: &str, b: &str) -> Ordering {
        let fav = |d: &str| !self.is_favorite(d);
        let when = |d: &str| std::cmp::Reverse(self.used.get(d).map(|u| u.at));
        (fav(a), when(a), a).cmp(&(fav(b), when(b), b))
    }
}

type All = BTreeMap<String, Marks>;

fn path() -> Option<PathBuf> {
    Some(dirs::state_dir()?.join("odotui/dbs.json"))
}

fn load_all() -> All {
    path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

/// Escritura atómica; un fallo aquí nunca es fatal.
fn save_all(all: &All) {
    let Some(p) = path() else { return };
    let Ok(raw) = serde_json::to_string_pretty(all) else {
        return;
    };
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let tmp = p.with_extension("json.tmp");
    if std::fs::write(&tmp, raw).is_ok() {
        let _ = std::fs::rename(tmp, p);
    }
}

pub fn load(profile: &str) -> Marks {
    load_all().remove(profile).unwrap_or_default()
}

/// Cuándo cambió el archivo por última vez (para recargarlo si lo tocó otro).
pub fn stamp() -> Option<std::time::SystemTime> {
    std::fs::metadata(path()?).ok()?.modified().ok()
}

/// Relee, cambia lo del perfil, guarda y devuelve cómo quedó.
pub fn edit(profile: &str, f: impl FnOnce(&mut Marks)) -> Marks {
    let mut all = load_all();
    let marks = all.entry(profile.to_string()).or_default();
    f(marks);
    let out = marks.clone();
    if out == Marks::default() {
        all.remove(profile);
    }
    save_all(&all);
    out
}

/// Anota que se usó `db` para `what`.
pub fn touch(profile: &str, db: &str, what: &str) -> Marks {
    edit(profile, |m| m.touch(db, what))
}

/// La BD cambió de nombre: sus marcas la siguen.
pub fn rename(profile: &str, from: &str, to: &str) -> Marks {
    edit(profile, |m| m.rename(from, to))
}

/// La BD ya no existe.
pub fn forget(profile: &str, db: &str) -> Marks {
    edit(profile, |m| m.forget(db))
}

/// «hace 5 min», «hace 3 h», «ayer», «hace 4 d», o la fecha.
pub fn ago(at: DateTime<Local>, now: DateTime<Local>) -> String {
    let s = (now - at).num_seconds().max(0);
    match s {
        0..60 => "ahora".into(),
        60..3600 => format!("hace {} min", s / 60),
        3600..86_400 => format!("hace {} h", s / 3600),
        86_400..172_800 => "ayer".into(),
        172_800..2_592_000 => format!("hace {} d", s / 86_400),
        _ => at.format("%d/%m/%Y").to_string(),
    }
}

#[cfg(test)]
mod tests {
    use chrono::Duration;

    use super::*;

    fn used(at: DateTime<Local>) -> Use {
        Use {
            at,
            what: "conectar".into(),
        }
    }

    #[test]
    fn favorites_then_recent_then_name() {
        let now = Local::now();
        let mut m = Marks::default();
        m.favorites.insert("zeta".into());
        m.used.insert("vieja".into(), used(now - Duration::days(3)));
        m.used.insert("nueva".into(), used(now));
        let mut dbs = vec!["alfa", "vieja", "zeta", "nueva", "beta"];
        dbs.sort_by(|a, b| m.order(a, b));
        assert_eq!(dbs, ["zeta", "nueva", "vieja", "alfa", "beta"]);
    }

    #[test]
    fn marks_follow_renames_and_drops() {
        let mut m = Marks::default();
        assert!(m.toggle_favorite("a"));
        m.touch("a", "conectar");
        m.rename("a", "b");
        assert!(m.is_favorite("b") && m.last_use("b").is_some());
        assert!(!m.is_favorite("a") && m.last_use("a").is_none());
        assert!(!m.toggle_favorite("b"));
        m.forget("b");
        assert_eq!(m, Marks::default());
    }

    #[test]
    fn human_ago() {
        let now = Local::now();
        assert_eq!(ago(now, now), "ahora");
        assert_eq!(ago(now - Duration::minutes(5), now), "hace 5 min");
        assert_eq!(ago(now - Duration::hours(3), now), "hace 3 h");
        assert_eq!(ago(now - Duration::hours(30), now), "ayer");
        assert_eq!(ago(now - Duration::days(4), now), "hace 4 d");
    }
}
