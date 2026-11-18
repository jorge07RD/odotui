//! Módulos de una BD: listado e instalación/actualización con `odoo -i/-u`.

use anyhow::{Result, bail};
use tokio_util::sync::CancellationToken;

use crate::{
    config::Profile,
    exec::{self, Spec, Stop},
    jobs::Tx,
    log::fold,
    odoo::{
        addons::{self, Addon},
        dev, pg, restore,
    },
};

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ModuleInfo {
    pub name: String,
    pub state: String,
    pub version: String,
    pub application: bool,
    /// Nombre visible («Sales», «Invoicing»), en español si está.
    pub title: String,
    pub category: String,
    /// Una línea que dice para qué sirve.
    pub summary: String,
    /// Versión del `__manifest__.py` en el disco (ya con la serie), si es un
    /// addon propio.
    pub disk: Option<String>,
}

impl ModuleInfo {
    /// Si coincide con la búsqueda (todas las palabras, sin tildes ni
    /// mayúsculas, en nombre, título, categoría, resumen o estado), qué tan
    /// bien (menos es mejor): 0 nombre exacto, 1 título exacto, 2 el nombre
    /// empieza así, 3 el título empieza así, 4 todas en el nombre, 5 todas en
    /// el título, 6 en otra parte.
    pub fn score(&self, words: &[String]) -> Option<u8> {
        if words.is_empty() {
            return Some(0);
        }
        let name = fold(&self.name);
        let title = fold(&self.title);
        let mut extra = format!("{} {} {}", self.category, self.summary, self.state);
        if self.changed() {
            extra += " cambió cambiado";
        }
        if self.disk.is_some() {
            extra += " propio";
        }
        let all = format!("{name} {title} {}", fold(&extra));
        if !words.iter().all(|w| all.contains(w.as_str())) {
            return None;
        }
        let q = words.join(" ");
        let all_in = |s: &str| words.iter().all(|w| s.contains(w.as_str()));
        Some(if name == q {
            0
        } else if title == q {
            1
        } else if name.starts_with(&q) {
            2
        } else if title.starts_with(&q) {
            3
        } else if all_in(&name) {
            4
        } else if all_in(&title) {
            5
        } else {
            6
        })
    }

    /// Instalado, pero el código en el disco es de otra versión: falta `-u`.
    pub fn changed(&self) -> bool {
        self.state == "installed"
            && self
                .disk
                .as_deref()
                .is_some_and(|d| !self.version.is_empty() && d != self.version)
    }
}

pub async fn list(p: &Profile, db: &str) -> Result<Vec<ModuleInfo>> {
    // shortdesc, summary y el nombre de la categoría son jsonb traducidos desde
    // Odoo 16 y texto antes: to_jsonb sirve para los dos. Sin saltos de línea,
    // que partirían la fila.
    let tr = |col: &str| {
        format!(
            "regexp_replace(COALESCE(to_jsonb({col}) ->> 'es_ES', to_jsonb({col}) ->> 'es_419', \
             to_jsonb({col}) ->> 'en_US', to_jsonb({col}) #>> '{{}}', ''), '\\s+', ' ', 'g')"
        )
    };
    let sql = format!(
        "SELECT m.name, m.state, COALESCE(m.latest_version, ''), COALESCE(m.application, false), \
         {}, {}, {} \
         FROM ir_module_module m LEFT JOIN ir_module_category c ON c.id = m.category_id \
         ORDER BY m.name",
        tr("m.shortdesc"),
        tr("c.name"),
        tr("m.summary"),
    );
    let rows = pg::query(p, db, &sql).await?;
    let one_line = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut mods: Vec<ModuleInfo> = rows
        .into_iter()
        .filter(|r| r.len() >= 7)
        .map(|r| ModuleInfo {
            name: r[0].clone(),
            state: r[1].clone(),
            version: r[2].clone(),
            application: r[3] == "t",
            title: one_line(&r[4]),
            category: one_line(&r[5]),
            summary: one_line(&r[6]),
            disk: None,
        })
        .collect();
    let dir = p.addons_host().to_string();
    let found = tokio::task::spawn_blocking(move || addons::scan(&dir)).await?;
    merge(&mut mods, &found);
    Ok(mods)
}

/// Junta la BD con los addons del disco. Los que la BD no conoce todavía
/// (sin «actualizar lista») aparecen como no instalados: `-i` los encuentra.
pub fn merge(mods: &mut Vec<ModuleInfo>, found: &[Addon]) {
    let serie = mods
        .iter()
        .find(|m| m.name == "base")
        .map(|m| restore::major(&m.version))
        .unwrap_or_default();
    for a in found {
        let disk = addons::adapt(a.version(), &serie);
        match mods.iter_mut().find(|m| m.name == a.name) {
            Some(m) => m.disk = Some(disk),
            None => mods.push(ModuleInfo {
                name: a.name.clone(),
                state: "uninstalled".into(),
                application: a.manifest.get("application") == Some(&serde_json::Value::Bool(true)),
                title: manifest_str(&a.manifest, "name"),
                category: manifest_str(&a.manifest, "category"),
                summary: manifest_str(&a.manifest, "summary"),
                disk: Some(disk),
                ..Default::default()
            }),
        }
    }
    mods.sort_by(|a, b| a.name.cmp(&b.name));
}

fn manifest_str(m: &serde_json::Value, k: &str) -> String {
    m.get(k)
        .and_then(|v| v.as_str())
        .map(|s| s.split_whitespace().collect::<Vec<_>>().join(" "))
        .unwrap_or_default()
}

/// Las palabras de una búsqueda, sin tildes ni mayúsculas.
pub fn search_words(q: &str) -> Vec<String> {
    fold(q).split_whitespace().map(String::from).collect()
}

/// Desinstala con el ORM (no hay opción de línea de comandos para esto).
pub async fn uninstall(
    p: &Profile,
    db: &str,
    names: &[String],
    tx: &Tx,
    cancel: &CancellationToken,
) -> Result<String> {
    if names.is_empty() {
        bail!("no hay módulos marcados");
    }
    let list = names
        .iter()
        .map(|n| format!("{n:?}"))
        .collect::<Vec<_>>()
        .join(", ");
    let code = format!(
        "mods = env['ir.module.module'].search([('name', 'in', [{list}]), ('state', 'in', ['installed', 'to upgrade'])])\n\
         print('ODOTUI', ','.join(mods.mapped('name')) or '-')\n\
         mods.button_immediate_uninstall()\n\
         env.cr.commit()\n"
    );
    let (tx, lines) = tx.capturing();
    dev::shell(p, db, &code, &tx, cancel).await?;
    let lines = lines.lock().map(|l| l.clone()).unwrap_or_default();
    let done = lines
        .iter()
        .find_map(|l| l.strip_prefix("ODOTUI "))
        .unwrap_or("-")
        .to_string();
    if lines.iter().any(|l| l.starts_with("Traceback")) {
        bail!("Odoo falló al desinstalar {done}");
    }
    if done == "-" {
        bail!("ninguno de {} estaba instalado", names.join(","));
    }
    Ok(format!("{done} desinstalado(s)"))
}

/// El ejecutable de Odoo del perfil, ya apuntando a su Postgres.
pub fn odoo(p: &Profile) -> Result<Spec> {
    odoo_sub(p, None)
}

/// Igual, con un subcomando (`shell`, `scaffold`…), que va antes de las opciones.
pub fn odoo_sub(p: &Profile, sub: Option<&str>) -> Result<Spec> {
    if p.is_docker() {
        if p.odoo_container.is_empty() {
            bail!("el perfil no tiene contenedor de Odoo");
        }
        let mut s = Spec::new("odoo").inside(&p.odoo_container).args(sub);
        if sub == Some("scaffold") {
            return Ok(s);
        }
        // `docker exec` se salta el entrypoint de la imagen, que es quien
        // pasa los datos de la BD: hay que darlos aquí.
        let host = if p.pg_host.is_empty() {
            "db"
        } else {
            &p.pg_host
        };
        s = s.args(["--db_host", host, "--db_port", &p.pg_port.to_string()]);
        if !p.pg_user.is_empty() {
            s = s.args(["--db_user", &p.pg_user]);
        }
        if !p.pg_password.is_empty() {
            s = s.args(["--db_password", &p.pg_password]);
        }
        return Ok(s);
    }
    if p.odoo_bin.is_empty() {
        bail!("el perfil no tiene odoo_bin");
    }
    let python = if p.python.is_empty() {
        "python3"
    } else {
        &p.python
    };
    let mut s = Spec::new(python).arg(&p.odoo_bin).args(sub);
    if sub == Some("scaffold") {
        return Ok(s);
    }
    if !p.odoo_conf.is_empty() {
        // El .conf manda; solo se añade lo que el perfil no puede saber.
        s = s.args(["-c", &p.odoo_conf]);
    } else {
        if !p.pg_host.is_empty() {
            s = s.args(["--db_host", &p.pg_host]);
        }
        s = s.args(["--db_port", &p.pg_port.to_string()]);
        if !p.pg_user.is_empty() {
            s = s.args(["--db_user", &p.pg_user]);
        }
        s = s.env("PGPASSWORD", &p.pg_password);
    }
    Ok(s)
}

/// Avisa en el log de lo que hará fallar a Odoo en los addons propios
/// (p. ej. una licencia que no acepta), antes de que dé un error críptico.
pub async fn warn_problems(p: &Profile, names: &[String], tx: &Tx) {
    let dir = p.addons_host().to_string();
    if dir.is_empty() {
        return;
    }
    let names = names.to_vec();
    if let Ok(found) = tokio::task::spawn_blocking(move || addons::problems_of(&dir, &names)).await
    {
        for problem in found {
            tx.log(format!("⚠ {problem}"));
        }
    }
}

pub async fn run(
    p: &Profile,
    db: &str,
    install: bool,
    names: &[String],
    tx: &Tx,
    cancel: &CancellationToken,
) -> Result<String> {
    if names.is_empty() {
        bail!("no hay módulos marcados");
    }
    warn_problems(p, names, tx).await;
    let list = names.join(",");
    let spec = odoo(p)?.args([
        "-d",
        db,
        if install { "-i" } else { "-u" },
        &list,
        "--stop-after-init",
        "--no-http",
    ]);
    let errors = exec::stream(&spec, None, tx, cancel, Stop::Interrupt).await?;
    let verb = if install {
        "instalado(s)"
    } else {
        "actualizado(s)"
    };
    Ok(if errors > 0 {
        format!("{list} {verb} con {errors} error(es) en el log")
    } else {
        format!("{list} {verb}")
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Mode;

    #[test]
    fn search_ranks_name_before_title_before_rest() {
        let m = |name: &str, title: &str, category: &str| ModuleInfo {
            name: name.into(),
            title: title.into(),
            category: category.into(),
            state: "installed".into(),
            ..Default::default()
        };
        let sale = m("sale", "Sales", "Sales");
        let sale_stock = m("sale_stock", "Sales and Warehouse Management", "Sales");
        let crm = m("crm", "CRM", "Sales");
        let acc = m("account", "Facturación", "Accounting");
        let q = |s: &str| search_words(s);
        assert_eq!(sale.score(&q("sale")), Some(0), "nombre exacto");
        assert_eq!(
            acc.score(&q("FACTURACION")),
            Some(1),
            "título exacto, sin tildes"
        );
        assert_eq!(
            sale_stock.score(&q("sale")),
            Some(2),
            "el nombre empieza así"
        );
        assert_eq!(acc.score(&q("fact")), Some(3), "el título empieza así");
        assert_eq!(sale_stock.score(&q("stock")), Some(4), "en el nombre");
        assert_eq!(sale_stock.score(&q("warehouse")), Some(5), "en el título");
        assert_eq!(
            sale_stock.score(&q("sale warehouse")),
            Some(5),
            "todas en el título"
        );
        assert_eq!(crm.score(&q("sales")), Some(6), "solo en la categoría");
        assert_eq!(
            sale_stock.score(&q("stock management")),
            Some(6),
            "repartidas"
        );
        assert_eq!(sale.score(&q("sale warehouse")), None);
        assert_eq!(crm.score(&[]), Some(0), "sin búsqueda, todos");
    }

    #[test]
    fn merge_marks_changed_and_new_addons() {
        let m = |name: &str, state: &str, version: &str| ModuleInfo {
            name: name.into(),
            state: state.into(),
            version: version.into(),
            ..Default::default()
        };
        let mut mods = vec![
            m("base", "installed", "17.0.1.3"),
            m("acme", "installed", "17.0.1.0"),
            m("viejo", "installed", "17.0.2.0"),
        ];
        let addon = |name: &str, version: &str| Addon {
            name: name.into(),
            path: name.into(),
            manifest: serde_json::json!({"version": version}),
        };
        merge(
            &mut mods,
            &[
                addon("acme", "1.1"),
                addon("viejo", "17.0.2.0"),
                addon("nuevo", "1.0"),
            ],
        );
        let names: Vec<_> = mods
            .iter()
            .map(|m| (m.name.as_str(), m.changed()))
            .collect();
        assert_eq!(
            names,
            [
                ("acme", true),
                ("base", false),
                ("nuevo", false),
                ("viejo", false)
            ]
        );
        assert_eq!(mods[0].disk.as_deref(), Some("17.0.1.1"));
        assert_eq!(mods[2].state, "uninstalled");
    }

    #[test]
    fn docker_odoo_gets_db_args() {
        let p = Profile {
            mode: Mode::Docker,
            odoo_container: "web".into(),
            pg_port: 5432,
            pg_user: "odoo".into(),
            ..Default::default()
        };
        assert_eq!(
            odoo(&p).unwrap().display(),
            "docker exec web odoo --db_host db --db_port 5432 --db_user odoo"
        );
    }

    #[test]
    fn local_odoo_uses_conf() {
        let p = Profile {
            mode: Mode::Local,
            odoo_bin: "/o/odoo-bin".into(),
            odoo_conf: "/o/o.conf".into(),
            pg_port: 5432,
            ..Default::default()
        };
        assert_eq!(
            odoo(&p).unwrap().display(),
            "python3 /o/odoo-bin -c /o/o.conf"
        );
        assert!(odoo(&Profile::default()).is_err());
    }
}
