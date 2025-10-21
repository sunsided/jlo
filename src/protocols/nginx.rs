use std::io::{self, Write};
use serde_json::Value;

use crate::{RenderCtx, write_kv_str, write_kv_num, as_f64_lossy};
use super::JsonProtocol;

/// Nginx-like access log JSON renderer
pub struct Nginx;

impl JsonProtocol for Nginx {
    fn sniff(&self, v: &Value) -> f32 {
        let o = match v.as_object() { Some(m) => m, None => return 0.0 };
        let mut score = 0.0f32;
        if o.get("method").and_then(Value::as_str).is_some() { score += 0.4; }
        if o.get("path").and_then(Value::as_str).is_some() { score += 0.4; }
        if o.get("status").is_some() { score += 0.2; }
        // tiny bonus for other typical fields (capped at 1.0)
        for k in ["protocol","query","host","bytes_sent","req_time","upstream_time"] {
            if o.contains_key(k) { score += 0.05; }
        }
        score.min(1.0)
    }

    fn render(&self, v: &Value, ctx: RenderCtx, out: &mut dyn Write) -> io::Result<bool> {
        let o = match v.as_object() { Some(m) => m, None => return Ok(false) };

        let ts = o.get("ts").and_then(Value::as_str);
        let method = o.get("method").and_then(Value::as_str);
        let path = o.get("path").and_then(Value::as_str);
        let status = o.get("status").and_then(Value::as_u64)
            .or_else(|| o.get("status").and_then(Value::as_str).and_then(|s| s.parse::<u64>().ok()));
        if method.is_none() || path.is_none() || status.is_none() { return Ok(false); }
        let status = status.unwrap();

        // Status → level + color
        let (level, lvl_color) = match status {
            100..=299 => ("INFO", ctx.pal.info),
            300..=399 => ("INFO", ctx.pal.status3xx),
            400..=499 => ("WARN", ctx.pal.warn),
            500..=599 => ("ERROR", ctx.pal.error),
            _ => ("INFO", ctx.pal.info),
        };

        let protocol = o.get("protocol").and_then(Value::as_str).unwrap_or("");
        let query = o.get("query").and_then(Value::as_str).unwrap_or("");
        let host = o.get("host").and_then(Value::as_str).unwrap_or("");
        let remote_addr = o.get("remote_addr").and_then(Value::as_str);

        if ctx.show_ts {
            if let Some(ts) = ts { write!(out, "[{}] ", ts)?; }
        }

        // colored level
        write!(out, "{}{}{} ", lvl_color, level, ctx.pal.reset)?;
        // status and request line (dim method/proto)
        write!(out, "{} {}{}{} ", status, ctx.pal.faint, method.unwrap(), ctx.pal.reset)?;
        if !host.is_empty() { write!(out, "{} ", host)?; }

        write!(out, "{}", path.unwrap())?;
        if !query.is_empty() { write!(out, "?{}", query)?; }
        if !protocol.is_empty() { write!(out, " {}{}{}", ctx.pal.faint, protocol, ctx.pal.reset)?; }

        write!(out, " —")?;

        write_kv_str(&mut *out, "bytes", o.get("bytes_sent").and_then(Value::as_u64).map(|n| n.to_string()).as_deref())?;
        write_kv_num(&mut *out, "rt", o.get("req_time").and_then(Value::as_f64))?;
        write_kv_num(&mut *out, "up", o.get("upstream_time").and_then(as_f64_lossy))?;
        write_kv_str(&mut *out, "up_addr", o.get("upstream_addr").and_then(Value::as_str))?;
        write_kv_str(&mut *out, "req", o.get("req_id").and_then(Value::as_str))?;
        write_kv_str(&mut *out, "trace", o.get("traceparent").and_then(Value::as_str))?;
        write_kv_str(&mut *out, "xff", o.get("xff").and_then(Value::as_str))?;
        if let Some(ip) = remote_addr { write_kv_str(&mut *out, "client", Some(ip))?; }
        write_kv_str(&mut *out, "referer", o.get("referer").and_then(Value::as_str))?;
        write_kv_str(&mut *out, "ua", o.get("user_agent").and_then(Value::as_str))?;

        if let Some(cache) = o.get("cache").and_then(Value::as_str) {
            if !cache.is_empty() { write_kv_str(&mut *out, "cache", Some(cache))?; }
        }

        out.write_all(b"\n")?;
        Ok(true)
    }
}
