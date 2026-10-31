//! Addons propios en el disco: lee cada `__manifest__.py` sin Python (es un
//! literal de diccionario) para saber su versión y dependencias, y así ver qué
//! módulos instalados tienen código más nuevo que la BD.

use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

#[derive(Debug, Clone, PartialEq)]
pub struct Addon {
    pub name: String,
    pub path: PathBuf,
    /// El manifest entero, como JSON.
    pub manifest: Value,
}

impl Addon {
    /// Versión tal como la escribió el autor (`1.0`, `17.0.1.2.0`…).
    pub fn version(&self) -> &str {
        self.manifest
            .get("version")
            .and_then(Value::as_str)
            .unwrap_or("1.0")
    }

    pub fn depends(&self) -> Vec<String> {
        self.manifest
            .get("depends")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(String::from)
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// Como `odoo.modules.module.adapt_version`: `1.0` → `17.0.1.0` en la serie 17.0.
pub fn adapt(version: &str, serie: &str) -> String {
    if serie.is_empty() || version != serie && version.starts_with(&format!("{serie}.")) {
        version.to_string()
    } else {
        format!("{serie}.{version}")
    }
}

/// Carpetas con `__manifest__.py` dentro de `dir` (un nivel), por nombre.
pub fn scan(dir: &str) -> Vec<Addon> {
    if dir.is_empty() {
        return vec![];
    }
    let Ok(rd) = std::fs::read_dir(dir) else {
        return vec![];
    };
    let mut out: Vec<Addon> = rd.flatten().filter_map(|e| read(&e.path())).collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// El addon de la carpeta `path`, si tiene un manifest legible.
pub fn read(path: &Path) -> Option<Addon> {
    let raw = std::fs::read_to_string(path.join("__manifest__.py")).ok()?;
    Some(Addon {
        name: path.file_name()?.to_string_lossy().into_owned(),
        path: path.to_path_buf(),
        manifest: parse(&raw)?,
    })
}

/// Un literal de Python (dict, list, tuple, str, número, True/False/None)
/// como JSON. Ignora comentarios y admite cadenas pegadas (`'a' 'b'`).
pub fn parse(src: &str) -> Option<Value> {
    let mut p = Parser {
        s: src.as_bytes(),
        i: 0,
    };
    let v = p.value()?;
    p.ws();
    Some(v)
}

struct Parser<'a> {
    s: &'a [u8],
    i: usize,
}

impl Parser<'_> {
    fn peek(&self) -> Option<u8> {
        self.s.get(self.i).copied()
    }

    /// Espacios y comentarios.
    fn ws(&mut self) {
        while let Some(c) = self.peek() {
            match c {
                b' ' | b'\t' | b'\n' | b'\r' => self.i += 1,
                b'#' => {
                    while self.peek().is_some_and(|c| c != b'\n') {
                        self.i += 1;
                    }
                }
                _ => break,
            }
        }
    }

    fn value(&mut self) -> Option<Value> {
        self.ws();
        match self.peek()? {
            b'{' => self.dict(),
            b'[' => self.seq(b']'),
            b'(' => self.seq(b')'),
            b'\'' | b'"' => {
                // Cadenas pegadas se concatenan, como en Python.
                let mut s = self.string()?;
                loop {
                    self.ws();
                    match self.peek() {
                        Some(b'\'' | b'"') => s += &self.string()?,
                        _ => return Some(Value::String(s)),
                    }
                }
            }
            _ => self.word(),
        }
    }

    fn dict(&mut self) -> Option<Value> {
        self.i += 1;
        let mut m = Map::new();
        loop {
            self.ws();
            if self.peek()? == b'}' {
                self.i += 1;
                return Some(Value::Object(m));
            }
            let k = match self.value()? {
                Value::String(s) => s,
                other => other.to_string(),
            };
            self.ws();
            if self.peek()? != b':' {
                return None;
            }
            self.i += 1;
            let v = self.value()?;
            m.insert(k, v);
            self.ws();
            if self.peek()? == b',' {
                self.i += 1;
            }
        }
    }

    fn seq(&mut self, close: u8) -> Option<Value> {
        self.i += 1;
        let mut v = vec![];
        loop {
            self.ws();
            if self.peek()? == close {
                self.i += 1;
                return Some(Value::Array(v));
            }
            v.push(self.value()?);
            self.ws();
            if self.peek()? == b',' {
                self.i += 1;
            }
        }
    }

    fn string(&mut self) -> Option<String> {
        let q = self.peek()?;
        let triple = self.s[self.i..].starts_with(&[q, q, q]);
        self.i += if triple { 3 } else { 1 };
        let mut out = Vec::new();
        loop {
            let c = self.peek()?;
            if c == q && (!triple || self.s[self.i..].starts_with(&[q, q, q])) {
                self.i += if triple { 3 } else { 1 };
                return String::from_utf8(out).ok();
            }
            self.i += 1;
            if c == b'\\' {
                let e = self.peek()?;
                self.i += 1;
                match e {
                    b'n' => out.push(b'\n'),
                    b't' => out.push(b'\t'),
                    // Continuación de línea.
                    b'\n' => {}
                    other => out.push(other),
                }
            } else {
                out.push(c);
            }
        }
    }

    /// Números y las palabras `True`, `False` y `None`.
    fn word(&mut self) -> Option<Value> {
        let start = self.i;
        while self
            .peek()
            .is_some_and(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'.' | b'-' | b'+'))
        {
            self.i += 1;
        }
        let w = std::str::from_utf8(&self.s[start..self.i]).ok()?;
        Some(match w {
            "True" => Value::Bool(true),
            "False" => Value::Bool(false),
            "None" => Value::Null,
            _ => {
                if let Ok(n) = w.parse::<i64>() {
                    Value::from(n)
                } else {
                    Value::from(w.parse::<f64>().ok()?)
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MANIFEST: &str = r#"
# -*- coding: utf-8 -*-
{
    'name': "Ventas ACME",
    'summary': 'Ajustes ' 'de ventas',  # pegadas
    'version': '17.0.1.2.0',
    'depends': ['sale', "stock",],
    'data': (
        'views/sale.xml',
    ),
    'installable': True,
    'auto_install': False,
    'sequence': 10,
    'description': """
Línea 1
Línea 'dos'
""",
}
"#;

    #[test]
    fn reads_odoo_manifests() {
        let m = parse(MANIFEST).unwrap();
        assert_eq!(m["name"], "Ventas ACME");
        assert_eq!(m["summary"], "Ajustes de ventas");
        assert_eq!(m["depends"], serde_json::json!(["sale", "stock"]));
        assert_eq!(m["data"][0], "views/sale.xml");
        assert_eq!(m["installable"], true);
        assert_eq!(m["sequence"], 10);
        assert!(m["description"].as_str().unwrap().contains("Línea 'dos'"));
        assert!(parse("{'a': }").is_none());
    }

    #[test]
    fn versions_like_odoo() {
        assert_eq!(adapt("1.0", "17.0"), "17.0.1.0");
        assert_eq!(adapt("17.0.1.2.0", "17.0"), "17.0.1.2.0");
        assert_eq!(adapt("17.0", "17.0"), "17.0.17.0");
        assert_eq!(adapt("16.0.1.0", "17.0"), "17.0.16.0.1.0");
    }

    #[test]
    fn scans_a_folder() {
        let dir = std::env::temp_dir().join(format!("odotui-addons-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("acme/models")).unwrap();
        std::fs::create_dir_all(dir.join("no_es_addon")).unwrap();
        std::fs::write(dir.join("acme/__manifest__.py"), MANIFEST).unwrap();
        let a = scan(dir.to_str().unwrap());
        assert_eq!(a.len(), 1);
        assert_eq!((a[0].name.as_str(), a[0].version()), ("acme", "17.0.1.2.0"));
        assert_eq!(a[0].depends(), ["sale", "stock"]);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
