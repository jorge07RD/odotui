//! Perfiles de conexión en `~/.config/odotui/config.toml`.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// Odoo y/o Postgres en contenedores: los comandos van por `docker exec`.
    #[default]
    Docker,
    /// Odoo con `odoo-bin` y Postgres en el sistema.
    Local,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Docker => "docker",
            Mode::Local => "local",
        }
    }
}

fn is_empty(s: &str) -> bool {
    s.is_empty()
}

fn default_port() -> u16 {
    5432
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    pub name: String,
    #[serde(default)]
    pub mode: Mode,
    /// Host de Postgres tal como lo ve Odoo (en docker suele ser `db`).
    #[serde(default, skip_serializing_if = "is_empty")]
    pub pg_host: String,
    #[serde(default = "default_port")]
    pub pg_port: u16,
    #[serde(default, skip_serializing_if = "is_empty")]
    pub pg_user: String,
    #[serde(default, skip_serializing_if = "is_empty")]
    pub pg_password: String,
    /// Contenedor de Postgres; si hay, `psql` corre dentro (contra 127.0.0.1).
    #[serde(default, skip_serializing_if = "is_empty")]
    pub pg_container: String,
    #[serde(default, skip_serializing_if = "is_empty")]
    pub odoo_container: String,
    #[serde(default, skip_serializing_if = "is_empty")]
    pub odoo_url: String,
    /// Carpetas `<nombre>/dump.sql` en esta máquina.
    #[serde(default, skip_serializing_if = "is_empty")]
    pub backups_dir: String,
    /// En docker es la ruta dentro del contenedor de Odoo.
    #[serde(default, skip_serializing_if = "is_empty")]
    pub filestore_dir: String,
    #[serde(default, skip_serializing_if = "is_empty")]
    pub odoo_bin: String,
    #[serde(default, skip_serializing_if = "is_empty")]
    pub python: String,
    #[serde(default, skip_serializing_if = "is_empty")]
    pub odoo_conf: String,
}

impl Default for Profile {
    fn default() -> Self {
        Self {
            name: String::new(),
            mode: Mode::Docker,
            pg_host: String::new(),
            pg_port: default_port(),
            pg_user: String::new(),
            pg_password: String::new(),
            pg_container: String::new(),
            odoo_container: String::new(),
            odoo_url: String::new(),
            backups_dir: String::new(),
            filestore_dir: String::new(),
            odoo_bin: String::new(),
            python: String::new(),
            odoo_conf: String::new(),
        }
    }
}

impl Profile {
    pub fn is_docker(&self) -> bool {
        self.mode == Mode::Docker
    }

    pub fn url(&self) -> &str {
        if self.odoo_url.is_empty() {
            "http://localhost:8069"
        } else {
            &self.odoo_url
        }
    }

    pub fn filestore(&self) -> String {
        if !self.filestore_dir.is_empty() {
            return self.filestore_dir.clone();
        }
        match self.mode {
            Mode::Docker => "/var/lib/odoo/filestore".into(),
            Mode::Local => dirs::data_local_dir()
                .map(|d| d.join("Odoo/filestore").display().to_string())
                .unwrap_or_default(),
        }
    }

}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub theme: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_profile: Option<String>,
    #[serde(default, rename = "profile")]
    pub profiles: Vec<Profile>,
}

pub fn default_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("odotui/config.toml")
}

impl Config {
    /// Sin archivo = config vacía (se crea al guardar el primer perfil).
    pub fn load(path: &Path) -> Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(raw) => toml::from_str(&raw).with_context(|| format!("{}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e).with_context(|| format!("{}", path.display())),
        }
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let raw = toml::to_string_pretty(self)?;
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, raw)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    pub fn default_index(&self) -> usize {
        self.default_profile
            .as_ref()
            .and_then(|n| self.profiles.iter().position(|p| &p.name == n))
            .unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let cfg = Config {
            theme: Some("gruvbox".into()),
            default_profile: Some("dev".into()),
            profiles: vec![
                Profile {
                    name: "dev".into(),
                    mode: Mode::Docker,
                    pg_host: "db".into(),
                    pg_port: 5432,
                    pg_user: "odoo".into(),
                    pg_password: "odoo".into(),
                    pg_container: "odoo-db-1".into(),
                    odoo_container: "odoo-web-1".into(),
                    ..Default::default()
                },
                Profile {
                    name: "local".into(),
                    mode: Mode::Local,
                    pg_host: "localhost".into(),
                    pg_port: 5433,
                    pg_user: "jorge".into(),
                    odoo_bin: "/opt/odoo/odoo-bin".into(),
                    ..Default::default()
                },
            ],
        };
        let raw = toml::to_string_pretty(&cfg).unwrap();
        assert!(raw.contains("[[profile]]"));
        let back: Config = toml::from_str(&raw).unwrap();
        assert_eq!(back, cfg);
        assert_eq!(back.default_index(), 0);
    }

    #[test]
    fn example_parses() {
        let cfg: Config = toml::from_str(include_str!("../config.example.toml")).unwrap();
        assert_eq!(cfg.profiles.len(), 2);
        assert_eq!(cfg.profiles[1].mode, Mode::Local);
    }
}
