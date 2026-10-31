//! Para que una IA entienda el modelo de datos sin escribir SQL: campos,
//! vistas, módulos y registros. Todo es de solo lectura; casi todo va por SQL
//! y solo `search_records` usa el ORM (`odoo shell`, que hace rollback).

use anyhow::{Context, Result, bail};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::{
    config::Profile,
    jobs::Tx,
    odoo::{
        addons, dev,
        pg::{self, literal},
    },
};

/// La columna como texto. Los campos traducibles son jsonb desde Odoo 16:
/// se lee el inglés y, si no hay, el JSON entero.
async fn text(p: &Profile, db: &str, table: &str, col: &str, alias: &str) -> Result<String> {
    let rows = pg::query(
        p,
        db,
        &format!(
            "SELECT data_type FROM information_schema.columns \
             WHERE table_name = {} AND column_name = {}",
            literal(table),
            literal(col)
        ),
    )
    .await?;
    let ty = rows.first().and_then(|r| r.first()).map(String::as_str);
    Ok(match ty {
        Some("jsonb") => format!("COALESCE({alias}.{col}->>'en_US', {alias}.{col}::text, '')"),
        Some(_) => format!("COALESCE({alias}.{col}::text, '')"),
        None => "''".into(),
    })
}

/// Nombres técnicos de modelo: `res.partner`, `x_algo`…
fn valid_model(model: &str) -> Result<()> {
    let ok = !model.is_empty()
        && model
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_'));
    if !ok {
        bail!("modelo inválido: «{model}»");
    }
    Ok(())
}

/// `modulo.nombre` de un xml_id.
fn split_xml_id(xml_id: &str) -> Result<(&str, &str)> {
    match xml_id.split_once('.') {
        Some((m, n)) if !m.is_empty() && !n.is_empty() => Ok((m, n)),
        _ => bail!("xml_id inválido: «{xml_id}» (modulo.nombre)"),
    }
}

/// Lo que guarda Postgres en hex, de vuelta a texto (así viajan las
/// columnas con saltos de línea).
fn unhex(s: &str) -> String {
    let bytes: Vec<u8> = (0..s.len() / 2)
        .filter_map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).ok())
        .collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

pub async fn describe_model(p: &Profile, db: &str, model: &str) -> Result<String> {
    valid_model(model)?;
    let m = literal(model);
    let name = text(p, db, "ir_model", "name", "m").await?;
    let head = pg::query(
        p,
        db,
        &format!(
            "SELECT {name}, COALESCE((SELECT string_agg(DISTINCT d.module, ', ' ORDER BY d.module) \
             FROM ir_model_data d WHERE d.model = 'ir.model' AND d.res_id = m.id), '') \
             FROM ir_model m WHERE m.model = {m}"
        ),
    )
    .await?;
    let Some(head) = head.into_iter().next() else {
        bail!("no existe el modelo «{model}» en «{db}»");
    };
    let label = text(p, db, "ir_model_fields", "field_description", "f").await?;
    let sel = text(p, db, "ir_model_fields_selection", "name", "s").await?;
    let sel = if sel == "''" {
        "''".to_string()
    } else {
        "COALESCE((SELECT string_agg(s.value, ', ' ORDER BY s.sequence, s.id) \
         FROM ir_model_fields_selection s WHERE s.field_id = f.id), '')"
            .to_string()
    };
    let rows = pg::query(
        p,
        db,
        &format!(
            "SELECT f.name, f.ttype, COALESCE(f.relation, ''), f.required, f.store, f.readonly, \
             COALESCE(f.compute, '') <> '', COALESCE(f.related, ''), {label}, \
             COALESCE((SELECT string_agg(DISTINCT d.module, ',' ORDER BY d.module) FROM ir_model_data d \
                       WHERE d.model = 'ir.model.fields' AND d.res_id = f.id), ''), {sel} \
             FROM ir_model_fields f WHERE f.model = {m} ORDER BY f.name"
        ),
    )
    .await?;
    let mut out = format!(
        "{model} — {} (módulos: {})\n{} campo(s): nombre · tipo · atributos · etiqueta · módulo(s)\n",
        head[0],
        head.get(1).map_or("", String::as_str),
        rows.len()
    );
    for r in rows.iter().filter(|r| r.len() >= 11) {
        let ty = if r[2].is_empty() {
            r[1].clone()
        } else {
            format!("{}→{}", r[1], r[2])
        };
        let mut flags = vec![];
        if r[3] == "t" {
            flags.push("requerido".to_string());
        }
        if r[6] == "t" {
            flags.push("calculado".into());
        }
        if !r[7].is_empty() {
            flags.push(format!("related={}", r[7]));
        }
        flags.push(
            if r[4] == "t" {
                "guardado"
            } else {
                "no guardado"
            }
            .into(),
        );
        if r[5] == "t" {
            flags.push("solo lectura".into());
        }
        let values = if r[10].is_empty() {
            String::new()
        } else {
            format!("\t[{}]", r[10])
        };
        out += &format!(
            "{}\t{ty}\t{}\t«{}»\t{}{values}\n",
            r[0],
            flags.join("·"),
            r[8],
            r[9]
        );
    }
    Ok(out)
}

/// La fila de una vista con su xml_id y el de la que hereda.
const VIEW_COLS: &str = "v.id, COALESCE(d.module || '.' || d.name, ''), v.name, v.type, v.priority, \
     COALESCE(v.mode, ''), COALESCE(pd.module || '.' || pd.name, ''), v.active, v.model";

const VIEW_FROM: &str = "FROM ir_ui_view v \
     LEFT JOIN LATERAL (SELECT module, name FROM ir_model_data WHERE model = 'ir.ui.view' \
         AND res_id = v.id ORDER BY id LIMIT 1) d ON true \
     LEFT JOIN LATERAL (SELECT module, name FROM ir_model_data WHERE model = 'ir.ui.view' \
         AND res_id = v.inherit_id ORDER BY id LIMIT 1) pd ON true";

async fn view_id(p: &Profile, db: &str, xml_id: &str) -> Result<String> {
    let (module, name) = split_xml_id(xml_id)?;
    let rows = pg::query(
        p,
        db,
        &format!(
            "SELECT res_id FROM ir_model_data WHERE model = 'ir.ui.view' AND module = {} AND name = {}",
            literal(module),
            literal(name)
        ),
    )
    .await?;
    rows.into_iter()
        .next()
        .and_then(|r| r.into_iter().next())
        .with_context(|| format!("no existe la vista «{xml_id}» en «{db}»"))
}

pub async fn find_views(
    p: &Profile,
    db: &str,
    model: Option<&str>,
    xml_id: Option<&str>,
) -> Result<String> {
    let filter = match (model, xml_id) {
        (_, Some(x)) => {
            // La vista, las que hereda y las que la heredan.
            let id = view_id(p, db, x).await?;
            format!(
                "v.id IN (WITH RECURSIVE up AS (SELECT id, inherit_id FROM ir_ui_view WHERE id = {id} \
                 UNION SELECT w.id, w.inherit_id FROM ir_ui_view w JOIN up ON w.id = up.inherit_id), \
                 down AS (SELECT id FROM ir_ui_view WHERE id = {id} \
                 UNION SELECT w.id FROM ir_ui_view w JOIN down ON w.inherit_id = down.id) \
                 SELECT id FROM up UNION SELECT id FROM down)"
            )
        }
        (Some(m), None) => {
            valid_model(m)?;
            format!("v.model = {}", literal(m))
        }
        (None, None) => bail!("indica model o xml_id"),
    };
    let rows = pg::query(
        p,
        db,
        &format!(
            "SELECT {VIEW_COLS} {VIEW_FROM} WHERE {filter} \
             ORDER BY v.type, v.inherit_id IS NOT NULL, v.priority, v.id LIMIT 300"
        ),
    )
    .await?;
    if rows.is_empty() {
        bail!("no hay vistas");
    }
    let mut out = format!(
        "{} vista(s): xml_id · tipo · prioridad · modo · hereda de · modelo\n",
        rows.len()
    );
    for r in rows.iter().filter(|r| r.len() >= 9) {
        let xml = if r[1].is_empty() {
            format!("(id {}, {})", r[0], r[2])
        } else {
            r[1].clone()
        };
        let parent = if r[6].is_empty() {
            String::new()
        } else {
            format!("\t← {}", r[6])
        };
        let off = if r[7] == "t" { "" } else { "\tINACTIVA" };
        out += &format!(
            "{xml}\t{}\t{}\t{}{parent}\t{}{off}\n",
            r[3], r[4], r[5], r[8]
        );
    }
    Ok(out)
}

/// La arquitectura de la vista y de todas las que la heredan, en orden.
pub async fn get_view(p: &Profile, db: &str, xml_id: &str) -> Result<String> {
    let id = view_id(p, db, xml_id).await?;
    let arch = text(p, db, "ir_ui_view", "arch_db", "v").await?;
    let rows = pg::query(
        p,
        db,
        &format!(
            "WITH RECURSIVE down AS (SELECT id, 0 AS depth FROM ir_ui_view WHERE id = {id} \
             UNION SELECT w.id, down.depth + 1 FROM ir_ui_view w JOIN down ON w.inherit_id = down.id) \
             SELECT {VIEW_COLS}, encode(convert_to({arch}, 'UTF8'), 'hex') {VIEW_FROM} \
             JOIN down ON down.id = v.id ORDER BY down.depth, v.priority, v.id LIMIT 60"
        ),
    )
    .await?;
    let mut out = String::new();
    for r in rows.iter().filter(|r| r.len() >= 10) {
        let name = if r[1].is_empty() { &r[2] } else { &r[1] };
        let parent = if r[6].is_empty() {
            String::new()
        } else {
            format!(", hereda de {}", r[6])
        };
        let off = if r[7] == "t" { "" } else { ", INACTIVA" };
        out += &format!(
            "── {name} ({}, prioridad {}{parent}{off}) ──\n{}\n\n",
            r[3],
            r[4],
            unhex(&r[9]).trim_end()
        );
    }
    out += "(la vista final combinada: odoo_shell con print(env.ref('…').get_combined_arch()))";
    Ok(out)
}

pub async fn module_info(p: &Profile, name: &str, db: Option<&str>) -> Result<String> {
    dev::valid_module(name)?;
    let dir = p.addons_host().to_string();
    let addon = tokio::task::spawn_blocking(move || addons::scan(&dir))
        .await?
        .into_iter()
        .find(|a| a.name == name);
    let mut out = String::new();
    let mut found = false;
    let mut installed_version = None;
    if let Some(db) = db {
        let n = literal(name);
        let rows = pg::query(
            p,
            db,
            &format!(
                "SELECT state, COALESCE(latest_version, ''), COALESCE(author, ''), \
                 COALESCE((SELECT string_agg(name, ', ' ORDER BY name) FROM ir_module_module_dependency \
                           WHERE module_id = m.id), ''), \
                 COALESCE((SELECT string_agg(x.name, ', ' ORDER BY x.name) FROM ir_module_module x \
                           JOIN ir_module_module_dependency d ON d.module_id = x.id \
                           WHERE d.name = m.name AND x.state = 'installed'), '') \
                 FROM ir_module_module m WHERE m.name = {n}"
            ),
        )
        .await?;
        match rows.into_iter().next().filter(|r| r.len() >= 5) {
            Some(r) => {
                found = true;
                out += &format!("En «{db}»: {} (versión {})\n", r[0], r[1]);
                if !r[2].is_empty() {
                    out += &format!("  autor: {}\n", r[2]);
                }
                out += &format!(
                    "  depende de: {}\n",
                    if r[3].is_empty() { "—" } else { &r[3] }
                );
                out += &format!(
                    "  lo usan (instalados): {}\n",
                    if r[4].is_empty() { "—" } else { &r[4] }
                );
                installed_version = Some(r[1].clone());
            }
            None => out += &format!("En «{db}»: no está en la lista de módulos\n"),
        }
    }
    if let Some(a) = addon {
        found = true;
        out += &format!("\nAddon propio en {}\n", a.path.display());
        if let Some(v) = installed_version.filter(|v| !v.is_empty()) {
            let serie = crate::odoo::restore::major(&v);
            let disk = addons::adapt(a.version(), &serie);
            if disk != v {
                out +=
                    &format!("  ⟳ el código es {disk} y la BD tiene {v}: falta update_modules\n");
            }
        }
        out += &format!(
            "__manifest__.py:\n{}\n",
            serde_json::to_string_pretty(&a.manifest).unwrap_or_default()
        );
    }
    if !found {
        bail!("no encontré «{name}» (ni en la BD ni en addons_host_dir)");
    }
    Ok(out)
}

/// Código de `odoo shell` para `search_read`; los argumentos viajan como
/// JSON dentro de una cadena, así nada se interpreta como Python.
pub fn search_code(model: &str, domain: &Value, fields: &[String], limit: usize) -> String {
    let args = serde_json::json!({
        "model": model, "domain": domain, "fields": fields, "limit": limit,
    });
    // Una cadena JSON es también una cadena válida de Python.
    let lit = serde_json::to_string(&args.to_string()).unwrap_or_default();
    format!(
        "import json\n\
         _a = json.loads({lit})\n\
         _m = env[_a['model']]\n\
         _r = _m.search_read(_a['domain'], _a['fields'], limit=_a['limit'])\n\
         print('ODOTUI_JSON ' + json.dumps({{'total': _m.search_count(_a['domain']), 'records': _r}}, default=str, ensure_ascii=False))\n"
    )
}

pub async fn search_records(
    p: &Profile,
    db: &str,
    model: &str,
    domain: &Value,
    fields: &[String],
    limit: usize,
    cancel: &CancellationToken,
) -> Result<String> {
    valid_model(model)?;
    if !domain.is_array() {
        bail!("domain debe ser una lista, p. ej. [[\"name\", \"ilike\", \"acme\"]]");
    }
    let (tx, lines) = Tx::detached().capturing();
    let code = search_code(model, domain, fields, limit);
    let r = dev::shell(p, db, &code, &tx, cancel).await;
    let lines = lines.lock().map(|l| l.clone()).unwrap_or_default();
    let found = lines.iter().find_map(|l| l.strip_prefix("ODOTUI_JSON "));
    let Some(raw) = found else {
        let why = dev::digest(&lines, 15);
        return match r {
            Err(e) => Err(e.context(why)),
            Ok(_) => bail!("odoo shell no devolvió resultados\n{why}"),
        };
    };
    let v: Value = serde_json::from_str(raw).context("respuesta inválida de odoo shell")?;
    let n = v["records"].as_array().map_or(0, Vec::len);
    Ok(format!(
        "{n} de {} registro(s) de {model}:\n{}",
        v["total"],
        serde_json::to_string_pretty(&v["records"]).unwrap_or_default()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_hex() {
        assert!(valid_model("res.partner").is_ok());
        assert!(valid_model("x_ok_2").is_ok());
        assert!(valid_model("res.partner'; drop").is_err());
        assert_eq!(split_xml_id("base.view_x").unwrap(), ("base", "view_x"));
        assert!(split_xml_id("sinpunto").is_err());
        assert_eq!(unhex("3c666f726d2f3e0a"), "<form/>\n");
    }

    #[test]
    fn search_code_keeps_args_as_data() {
        let code = search_code(
            "res.partner",
            &serde_json::json!([["name", "ilike", "o'neil \"x\"\n"]]),
            &["name".into()],
            5,
        );
        let line = code.lines().nth(1).unwrap();
        assert!(line.starts_with("_a = json.loads(\""), "{line}");
        // Lo que llega a Python es exactamente el JSON de los argumentos.
        let lit = &line["_a = json.loads(".len()..line.len() - 1];
        let back: String = serde_json::from_str(lit).unwrap();
        let v: Value = serde_json::from_str(&back).unwrap();
        assert_eq!(v["domain"][0][2], "o'neil \"x\"\n");
        assert_eq!(v["limit"], 5);
    }
}
