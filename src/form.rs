//! Formularios: perfiles, restauración y diálogos con un campo. Los campos del
//! perfil dependen del modo (docker pide contenedores, local pide odoo-bin), y
//! cambiar el modo conserva lo ya escrito.

use std::collections::HashMap;

use ratatui::crossterm::event::{KeyCode, KeyEvent};

use crate::config::{Mode, Profile};

#[derive(Debug, Clone, PartialEq)]
pub enum FieldKind {
    Text,
    /// Se muestra con `•`.
    Password,
    Number,
    Select(Vec<String>),
    /// Casilla: `value` es `"1"` o vacío.
    Check,
}

#[derive(Debug, Clone)]
pub struct Field {
    pub key: &'static str,
    pub label: &'static str,
    pub help: &'static str,
    pub kind: FieldKind,
    pub value: String,
    /// Posición del cursor en caracteres.
    pub cursor: usize,
}

impl Field {
    pub fn checked(&self) -> bool {
        !self.value.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Target {
    /// `None` = perfil nuevo; `Some(i)` = editar ese.
    Profile(Option<usize>),
    Restore {
        backup: String,
    },
    Duplicate(String),
    Rename(String),
    Neutralize(String),
    Backup(String),
}

#[derive(Debug, PartialEq)]
pub enum Outcome {
    Continue,
    Save,
    Cancel,
}

#[derive(Debug)]
pub struct Form {
    pub title: String,
    pub target: Target,
    pub fields: Vec<Field>,
    pub focus: usize,
    pub error: Option<String>,
    /// Aviso fijo arriba de los campos (p. ej. la versión de un backup).
    pub note: Option<String>,
    pub scroll: usize,
    stash: HashMap<&'static str, String>,
}

fn yes(b: bool) -> String {
    if b { "1".into() } else { String::new() }
}

impl Form {
    fn new(title: String, target: Target, stash: HashMap<&'static str, String>) -> Self {
        let mut f = Self {
            title,
            target,
            fields: vec![],
            focus: 0,
            error: None,
            note: None,
            scroll: 0,
            stash,
        };
        f.relayout();
        f
    }

    pub fn profile(index: Option<usize>, p: &Profile) -> Self {
        let stash = HashMap::from([
            ("name", p.name.clone()),
            ("mode", p.mode.as_str().to_string()),
            ("pg_host", p.pg_host.clone()),
            ("pg_port", p.pg_port.to_string()),
            ("pg_user", p.pg_user.clone()),
            ("pg_password", p.pg_password.clone()),
            ("pg_container", p.pg_container.clone()),
            ("odoo_container", p.odoo_container.clone()),
            ("odoo_url", p.odoo_url.clone()),
            ("backups_dir", p.backups_dir.clone()),
            ("filestore_dir", p.filestore_dir.clone()),
            ("odoo_bin", p.odoo_bin.clone()),
            ("python", p.python.clone()),
            ("odoo_conf", p.odoo_conf.clone()),
            ("addons_dir", p.addons_dir.clone()),
        ]);
        let title = match index {
            Some(_) => format!("Editar perfil {}", p.name),
            None => "Nuevo perfil".into(),
        };
        Self::new(title, Target::Profile(index), stash)
    }

    pub fn restore(backup: &str, db: &str, overwrite: bool, has_filestore: bool) -> Self {
        let stash = HashMap::from([
            ("db", db.to_string()),
            ("overwrite", yes(overwrite)),
            ("filestore", yes(has_filestore)),
            ("neutralize", yes(true)),
            ("admin_password", "admin".to_string()),
        ]);
        Self::new(
            format!("Restaurar {backup}"),
            Target::Restore {
                backup: backup.into(),
            },
            stash,
        )
    }

    pub fn duplicate(db: &str) -> Self {
        let stash = HashMap::from([("name", format!("{db}_copia"))]);
        Self::new(
            format!("Duplicar {db}"),
            Target::Duplicate(db.into()),
            stash,
        )
    }

    pub fn rename(db: &str) -> Self {
        let stash = HashMap::from([("name", db.to_string())]);
        Self::new(format!("Renombrar {db}"), Target::Rename(db.into()), stash)
    }

    pub fn neutralize(db: &str) -> Self {
        let stash = HashMap::from([("admin_password", "admin".to_string())]);
        Self::new(
            format!("Neutralizar {db}"),
            Target::Neutralize(db.into()),
            stash,
        )
    }

    pub fn backup(db: &str) -> Self {
        let stash = HashMap::from([("format", "zip".to_string()), ("filestore", yes(true))]);
        Self::new(format!("Backup de {db}"), Target::Backup(db.into()), stash)
    }

    /// Reconstruye los campos según el destino (y el modo, en perfiles).
    fn relayout(&mut self) {
        use FieldKind::*;
        let mut specs: Vec<(&'static str, &'static str, &'static str, FieldKind)> = vec![];
        match &self.target {
            Target::Profile(_) => {
                let docker = self.stash.get("mode").is_none_or(|m| m == "docker");
                specs.push(("name", "Nombre", "único", Text));
                specs.push((
                    "mode",
                    "Modo",
                    "",
                    Select(vec!["docker".into(), "local".into()]),
                ));
                specs.push(("pg_host", "Host Postgres", "visto por Odoo", Text));
                specs.push(("pg_port", "Puerto", "5432", Number));
                specs.push(("pg_user", "Usuario", "", Text));
                specs.push(("pg_password", "Contraseña", "", Password));
                if docker {
                    specs.push((
                        "pg_container",
                        "Contenedor Postgres",
                        "psql corre dentro",
                        Text,
                    ));
                    specs.push(("odoo_container", "Contenedor Odoo", "", Text));
                } else {
                    specs.push(("odoo_bin", "odoo-bin", "ruta", Text));
                    specs.push(("python", "Python", "vacío = python3", Text));
                    specs.push(("odoo_conf", "odoo.conf", "opcional", Text));
                }
                specs.push(("odoo_url", "URL de Odoo", "http://localhost:8069", Text));
                specs.push((
                    "addons_dir",
                    "Addons propios",
                    if docker {
                        "ruta en el contenedor"
                    } else {
                        "ruta"
                    },
                    Text,
                ));
                specs.push(("backups_dir", "Carpeta de backups", "en este equipo", Text));
                specs.push((
                    "filestore_dir",
                    "Filestore",
                    if docker {
                        "vacío = /var/lib/odoo/filestore"
                    } else {
                        "vacío = ~/.local/share/Odoo/filestore"
                    },
                    Text,
                ));
            }
            Target::Restore { .. } => {
                specs.push(("db", "BD destino", "letras, números, _ - .", Text));
                specs.push((
                    "overwrite",
                    "Sobrescribir si existe",
                    "la borra antes",
                    Check,
                ));
                specs.push((
                    "filestore",
                    "Copiar filestore",
                    "si el backup lo trae",
                    Check,
                ));
                specs.push(("neutralize", "Neutralizar", "crons, correo, pagos", Check));
                specs.push((
                    "admin_password",
                    "Password de admin",
                    "al neutralizar; vacío = no cambiar",
                    Text,
                ));
            }
            Target::Duplicate(_) | Target::Rename(_) => {
                specs.push(("name", "Nuevo nombre", "letras, números, _ - .", Text));
            }
            Target::Backup(_) => {
                specs.push((
                    "format",
                    "Formato",
                    "",
                    Select(vec!["zip".into(), "dump".into()]),
                ));
                specs.push(("filestore", "Incluir filestore", "solo en zip", Check));
            }
            Target::Neutralize(_) => {
                specs.push((
                    "admin_password",
                    "Password de admin",
                    "vacío = no cambiar",
                    Text,
                ));
            }
        }

        let focus_key = self.fields.get(self.focus).map(|f| f.key);
        self.fields = specs
            .into_iter()
            .map(|(key, label, help, kind)| {
                let mut value = self.stash.get(key).cloned().unwrap_or_default();
                if let Select(options) = &kind
                    && !options.contains(&value)
                {
                    value = options[0].clone();
                }
                Field {
                    key,
                    label,
                    help,
                    cursor: value.chars().count(),
                    kind,
                    value,
                }
            })
            .collect();
        self.focus = focus_key
            .and_then(|k| self.fields.iter().position(|f| f.key == k))
            .unwrap_or(0);
    }

    fn set(&mut self, i: usize, value: String) {
        let key = self.fields[i].key;
        self.stash.insert(key, value.clone());
        self.fields[i].value = value;
        if key == "mode" {
            self.relayout();
        }
    }

    pub fn value(&self, key: &str) -> String {
        self.fields
            .iter()
            .find(|f| f.key == key)
            .map(|f| f.value.trim().to_string())
            .unwrap_or_default()
    }

    pub fn checked(&self, key: &str) -> bool {
        self.fields.iter().any(|f| f.key == key && f.checked())
    }

    /// Cambia la opción de un `Select` (`dir` = 1 siguiente, -1 anterior) o
    /// marca/desmarca una casilla.
    pub fn cycle(&mut self, i: usize, dir: i32) {
        let Some(kind) = self.fields.get(i).map(|f| f.kind.clone()) else {
            return;
        };
        self.focus = i;
        match kind {
            FieldKind::Select(options) => {
                let cur = options
                    .iter()
                    .position(|o| *o == self.fields[i].value)
                    .unwrap_or(0) as i32;
                let next = (cur + dir).rem_euclid(options.len() as i32) as usize;
                self.set(i, options[next].clone());
            }
            FieldKind::Check => {
                let v = yes(!self.fields[i].checked());
                self.set(i, v);
            }
            _ => {}
        }
    }

    pub fn focus_field(&mut self, i: usize) {
        if i < self.fields.len() {
            self.focus = i;
            self.fields[i].cursor = self.fields[i].value.chars().count();
        }
    }

    pub fn paste(&mut self, text: &str) {
        for c in text.chars().filter(|c| !c.is_control()) {
            self.insert(c);
        }
    }

    fn insert(&mut self, c: char) {
        let i = self.focus;
        let Some(f) = self.fields.get(i) else { return };
        match f.kind {
            FieldKind::Text | FieldKind::Password => {}
            FieldKind::Number if c.is_ascii_digit() => {}
            _ => return,
        }
        let mut chars: Vec<char> = f.value.chars().collect();
        let at = f.cursor.min(chars.len());
        chars.insert(at, c);
        self.set(i, chars.into_iter().collect());
        self.fields[i].cursor = at + 1;
    }

    pub fn key(&mut self, k: KeyEvent) -> Outcome {
        let i = self.focus;
        let n = self.fields.len().max(1);
        let kind = self.fields.get(i).map(|f| f.kind.clone());
        match k.code {
            KeyCode::Esc => return Outcome::Cancel,
            KeyCode::Enter => return Outcome::Save,
            KeyCode::Tab | KeyCode::Down => self.focus_field((i + 1) % n),
            KeyCode::BackTab | KeyCode::Up => self.focus_field((i + n - 1) % n),
            _ => match kind {
                Some(FieldKind::Select(_)) => match k.code {
                    KeyCode::Left => self.cycle(i, -1),
                    KeyCode::Right | KeyCode::Char(' ') => self.cycle(i, 1),
                    _ => {}
                },
                Some(FieldKind::Check) => {
                    if matches!(k.code, KeyCode::Char(' ') | KeyCode::Left | KeyCode::Right) {
                        self.cycle(i, 1);
                    }
                }
                Some(_) => self.edit_text(k),
                None => {}
            },
        }
        Outcome::Continue
    }

    fn edit_text(&mut self, k: KeyEvent) {
        let i = self.focus;
        let len = self.fields[i].value.chars().count();
        let cur = self.fields[i].cursor.min(len);
        match k.code {
            KeyCode::Char(c) => self.insert(c),
            KeyCode::Backspace if cur > 0 => {
                let mut chars: Vec<char> = self.fields[i].value.chars().collect();
                chars.remove(cur - 1);
                self.set(i, chars.into_iter().collect());
                self.fields[i].cursor = cur - 1;
            }
            KeyCode::Delete if cur < len => {
                let mut chars: Vec<char> = self.fields[i].value.chars().collect();
                chars.remove(cur);
                self.set(i, chars.into_iter().collect());
                self.fields[i].cursor = cur;
            }
            KeyCode::Left => self.fields[i].cursor = cur.saturating_sub(1),
            KeyCode::Right => self.fields[i].cursor = (cur + 1).min(len),
            KeyCode::Home => self.fields[i].cursor = 0,
            KeyCode::End => self.fields[i].cursor = len,
            _ => {}
        }
    }

    /// El perfil que describe el formulario.
    pub fn to_profile(&self) -> Result<Profile, String> {
        let name = self.value("name");
        if name.is_empty() {
            return Err("el perfil necesita un nombre".into());
        }
        let port = self.value("pg_port");
        let pg_port = if port.is_empty() {
            5432
        } else {
            port.parse().map_err(|_| "puerto inválido".to_string())?
        };
        let mode = if self.value("mode") == "local" {
            Mode::Local
        } else {
            Mode::Docker
        };
        // Lo del otro modo queda en el stash pero no se guarda.
        let pick = |k: &str| self.value(k);
        Ok(Profile {
            name,
            mode,
            pg_host: pick("pg_host"),
            pg_port,
            pg_user: pick("pg_user"),
            pg_password: self
                .fields
                .iter()
                .find(|f| f.key == "pg_password")
                .map(|f| f.value.clone())
                .unwrap_or_default(),
            pg_container: pick("pg_container"),
            odoo_container: pick("odoo_container"),
            odoo_url: pick("odoo_url"),
            backups_dir: pick("backups_dir"),
            filestore_dir: pick("filestore_dir"),
            odoo_bin: pick("odoo_bin"),
            python: pick("python"),
            odoo_conf: pick("odoo_conf"),
            addons_dir: pick("addons_dir"),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::crossterm::event::KeyModifiers;

    fn key(c: KeyCode) -> KeyEvent {
        KeyEvent::new(c, KeyModifiers::NONE)
    }

    #[test]
    fn mode_change_relayouts_and_keeps_values() {
        let mut f = Form::profile(None, &Profile::default());
        for c in "dev".chars() {
            f.key(key(KeyCode::Char(c)));
        }
        assert!(f.fields.iter().any(|x| x.key == "pg_container"));
        f.cycle(1, 1);
        assert_eq!(f.value("mode"), "local");
        assert!(f.fields.iter().any(|x| x.key == "odoo_bin"));
        assert!(!f.fields.iter().any(|x| x.key == "pg_container"));
        assert_eq!(f.value("name"), "dev");
        let p = f.to_profile().unwrap();
        assert_eq!(p.mode, Mode::Local);
        assert_eq!(p.pg_port, 5432);
    }

    #[test]
    fn checkboxes_toggle() {
        let mut f = Form::restore("prod", "prod", false, true);
        assert!(f.checked("filestore"));
        assert!(!f.checked("overwrite"));
        let i = f.fields.iter().position(|x| x.key == "overwrite").unwrap();
        f.focus_field(i);
        f.key(key(KeyCode::Char(' ')));
        assert!(f.checked("overwrite"));
        // En una casilla las letras no escriben nada.
        f.key(key(KeyCode::Char('x')));
        assert_eq!(f.fields[i].value, "1");
    }

    #[test]
    fn numbers_only_accept_digits() {
        let mut f = Form::profile(None, &Profile::default());
        let i = f.fields.iter().position(|x| x.key == "pg_port").unwrap();
        f.focus_field(i);
        assert_eq!(f.value("pg_port"), "5432");
        for _ in 0..4 {
            f.key(key(KeyCode::Backspace));
        }
        f.paste("54a33");
        assert_eq!(f.value("pg_port"), "5433");
    }
}
