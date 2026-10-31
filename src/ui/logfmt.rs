//! Colores por partes para el log de Odoo: hora, nivel, BD, logger y
//! mensaje; las peticiones web (werkzeug) compactadas en método, ruta,
//! estado y tiempos; y los tracebacks con archivo, línea y excepción.

use ratatui::{
    style::{Color, Style},
    text::{Line, Span},
};

use crate::{app::App, log::Level, theme::mix};

/// `2026-10-31 20:00:06,946 1 INFO prueba odoo.models: mensaje`
#[derive(Debug, PartialEq)]
pub struct OdooLine<'a> {
    pub time: &'a str,
    pub pid: &'a str,
    pub level: &'a str,
    pub db: &'a str,
    pub logger: &'a str,
    pub msg: &'a str,
}

pub fn parse_odoo(line: &str) -> Option<OdooLine<'_>> {
    let mut it = line.splitn(6, ' ');
    let (date, time, pid, level, db, rest) = (
        it.next()?,
        it.next()?,
        it.next()?,
        it.next()?,
        it.next()?,
        it.next()?,
    );
    let is_date = date.len() == 10 && date.as_bytes()[4] == b'-' && date.as_bytes()[7] == b'-';
    let is_time = time.len() >= 8 && time.as_bytes()[2] == b':';
    if !is_date || !is_time || !pid.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    if !matches!(level, "DEBUG" | "INFO" | "WARNING" | "ERROR" | "CRITICAL") {
        return None;
    }
    let (logger, msg) = rest.split_once(": ").unwrap_or((rest, ""));
    Some(OdooLine {
        time,
        pid,
        level,
        db,
        logger,
        msg,
    })
}

/// `172.20.0.1 - - [31/Oct/2026 20:00:06] "GET /web/x HTTP/1.1" 200 - 8 0.003 0.230`
#[derive(Debug, PartialEq)]
pub struct Request<'a> {
    pub ip: &'a str,
    pub method: &'a str,
    pub path: &'a str,
    pub status: &'a str,
    pub queries: Option<&'a str>,
    /// Tiempo en SQL + el resto, en segundos.
    pub secs: Option<f64>,
}

pub fn parse_request(msg: &str) -> Option<Request<'_>> {
    let ip = msg.split(' ').next()?;
    let start = msg.find('"')? + 1;
    let end = start + msg[start..].find('"')?;
    let mut req = msg[start..end].split(' ');
    let method = req.next()?;
    let path = req.next()?;
    let mut tail = msg[end + 1..].split_whitespace();
    let status = tail.next()?;
    if !status.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let _size = tail.next();
    let queries = tail.next();
    let secs = match (tail.next(), tail.next()) {
        (Some(a), Some(b)) => Some(a.parse::<f64>().ok()? + b.parse::<f64>().ok()?),
        _ => None,
    };
    Some(Request {
        ip,
        method,
        path,
        status,
        queries,
        secs,
    })
}

fn level_badge(app: &App, level: &str) -> Span<'static> {
    let t = &app.theme;
    match level {
        "ERROR" | "CRITICAL" => Span::styled(
            if level == "ERROR" { "ERROR" } else { "CRIT " },
            Style::new().fg(t.bg).bg(t.err).bold(),
        ),
        "WARNING" => Span::styled("WARN ", Style::new().fg(t.warn).bold()),
        "DEBUG" => Span::styled("DEBUG", Style::new().fg(t.dim)),
        _ => Span::styled("INFO ", Style::new().fg(t.accent)),
    }
}

fn status_color(app: &App, status: &str) -> Color {
    let t = &app.theme;
    match status.as_bytes().first() {
        Some(b'2') => t.ok,
        Some(b'3') => t.accent,
        Some(b'4') => t.warn,
        _ => t.err,
    }
}

fn odoo_spans(app: &App, o: &OdooLine) -> Vec<Span<'static>> {
    let t = &app.theme;
    let faint = Style::new().fg(mix(t.dim, t.bg, 0.25));
    let mut out = vec![
        Span::styled(o.time.to_string(), Style::new().fg(t.dim)),
        Span::styled(format!(" {:>3} ", o.pid), faint),
        level_badge(app, o.level),
        Span::raw(" "),
    ];
    if o.db != "?" {
        out.push(Span::styled(
            format!("{} ", o.db),
            Style::new().fg(mix(t.fg, t.dim, 0.45)),
        ));
    }
    let msg_style = match o.level {
        "ERROR" | "CRITICAL" => Style::new().fg(t.err),
        "WARNING" => Style::new().fg(t.warn),
        "DEBUG" => Style::new().fg(t.dim),
        _ => Style::new().fg(t.fg),
    };
    if o.logger == "werkzeug"
        && let Some(r) = parse_request(o.msg)
    {
        out.push(Span::styled("http ", Style::new().fg(t.accent)));
        let method_color = match r.method {
            "GET" => t.ok,
            "POST" => t.accent,
            _ => t.warn,
        };
        out.push(Span::styled(
            format!("{:<4} ", r.method),
            Style::new().fg(method_color).bold(),
        ));
        out.push(Span::styled(r.path.to_string(), Style::new().fg(t.fg)));
        out.push(Span::styled(
            format!(" {}", r.status),
            Style::new().fg(status_color(app, r.status)).bold(),
        ));
        let mut extra = String::new();
        if let Some(q) = r.queries.filter(|q| *q != "-") {
            extra += &format!(" · {q}q");
        }
        if let Some(s) = r.secs {
            extra += &format!(" · {s:.2}s");
        }
        // Las peticiones lentas resaltan.
        let slow = r.secs.is_some_and(|s| s >= 1.0);
        out.push(Span::styled(
            extra,
            if slow {
                Style::new().fg(t.warn).bold()
            } else {
                Style::new().fg(t.dim)
            },
        ));
        out.push(Span::styled(format!("  {}", r.ip), faint));
        return out;
    }
    out.push(Span::styled(
        o.logger.to_string(),
        Style::new().fg(mix(t.accent, t.fg, 0.25)),
    ));
    out.push(Span::styled(": ", Style::new().fg(t.dim)));
    out.push(Span::styled(o.msg.to_string(), msg_style));
    out
}

/// Líneas de un traceback de Python; `None` si no lo son.
fn traceback_spans(app: &App, line: &str) -> Option<Vec<Span<'static>>> {
    let t = &app.theme;
    if line.starts_with("Traceback") {
        return Some(vec![Span::styled(
            line.to_string(),
            Style::new().fg(t.err).bold(),
        )]);
    }
    // `  File "/ruta.py", line 88, in action_confirm`
    let trimmed = line.trim_start();
    if let Some(rest) = trimmed.strip_prefix("File \"")
        && let Some((path, rest)) = rest.split_once("\", line ")
    {
        let indent = &line[..line.len() - trimmed.len()];
        let (num, func) = rest.split_once(", in ").unwrap_or((rest, ""));
        let mut out = vec![
            Span::styled(format!("{indent}File "), Style::new().fg(t.dim)),
            Span::styled(format!("\"{path}\""), Style::new().fg(t.accent)),
            Span::styled(", line ", Style::new().fg(t.dim)),
            Span::styled(num.to_string(), Style::new().fg(t.warn).bold()),
        ];
        if !func.is_empty() {
            out.push(Span::styled(", in ", Style::new().fg(t.dim)));
            out.push(Span::styled(func.to_string(), Style::new().fg(t.fg).bold()));
        }
        return Some(out);
    }
    // `odoo.exceptions.UserError: mensaje`
    if crate::log::is_exception(line)
        && let Some((name, msg)) = line.split_once(": ")
    {
        return Some(vec![
            Span::styled(name.to_string(), Style::new().fg(t.err).bold()),
            Span::styled(": ", Style::new().fg(t.dim)),
            Span::styled(msg.to_string(), Style::new().fg(t.fg)),
        ]);
    }
    None
}

/// La línea con colores por partes, recortada a `width` columnas.
pub fn format(app: &App, line: &str, level: Level, width: usize) -> Line<'static> {
    let spans = if let Some(o) = parse_odoo(line) {
        odoo_spans(app, &o)
    } else if let Some(s) = traceback_spans(app, line) {
        s
    } else {
        vec![Span::styled(
            line.to_string(),
            super::widgets::level_style(app, level),
        )]
    };
    Line::from(fit(spans, width))
}

/// Recorta los tramos para que quepan en `width`, con `…` al final.
pub fn fit(spans: Vec<Span<'static>>, width: usize) -> Vec<Span<'static>> {
    let total: usize = spans.iter().map(Span::width).sum();
    if total <= width {
        return spans;
    }
    let mut out = vec![];
    let mut used = 0;
    for s in spans {
        let w = s.width();
        if used + w < width {
            used += w;
            out.push(s);
            continue;
        }
        let room = width.saturating_sub(used + 1);
        let cut = super::widgets::truncate(&s.content, room + 1);
        let cut = cut.trim_end_matches('…').to_string();
        out.push(Span::styled(format!("{cut}…"), s.style));
        break;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const WERKZEUG: &str = "2026-10-31 20:00:09,768 1 INFO prueba werkzeug: 172.20.0.1 - - \
        [31/Oct/2026 20:00:09] \"POST /web/action/load HTTP/1.1\" 200 - 8 0.003 0.230";

    #[test]
    fn parses_odoo_lines() {
        let o = parse_odoo(WERKZEUG).unwrap();
        assert_eq!(
            (o.time, o.pid, o.level, o.db, o.logger),
            ("20:00:09,768", "1", "INFO", "prueba", "werkzeug")
        );
        let o =
            parse_odoo("2026-10-31 20:00:10,509 7 WARNING ? odoo.models: campo obsoleto").unwrap();
        assert_eq!(
            (o.db, o.logger, o.msg),
            ("?", "odoo.models", "campo obsoleto")
        );
        assert!(parse_odoo("Traceback (most recent call last):").is_none());
        assert!(parse_odoo("hola mundo que tal 1 INFO x").is_none());
    }

    #[test]
    fn parses_werkzeug_requests() {
        let o = parse_odoo(WERKZEUG).unwrap();
        let r = parse_request(o.msg).unwrap();
        assert_eq!(
            (r.ip, r.method, r.path, r.status),
            ("172.20.0.1", "POST", "/web/action/load", "200")
        );
        assert_eq!(r.queries, Some("8"));
        assert!((r.secs.unwrap() - 0.233).abs() < 1e-9);
        // Sin tiempos (p. ej. estáticos).
        let r = parse_request("1.2.3.4 - - [x] \"GET /a HTTP/1.1\" 304 -").unwrap();
        assert_eq!((r.status, r.secs), ("304", None));
    }

    #[test]
    fn fits_to_width() {
        let spans = vec![Span::raw("hola "), Span::raw("mundo largo")];
        let out = fit(spans, 9);
        let text: String = out.iter().map(|s| s.content.to_string()).collect();
        assert_eq!(text, "hola mun…");
        assert_eq!(fit(vec![Span::raw("ok")], 9).len(), 1);
    }
}
