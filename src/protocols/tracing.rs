use std::io::{self, Write};
use serde_json::Value;

use crate::{RenderCtx, write_json_atom};
use super::JsonProtocol;

/// Rust tracing JSON renderer
pub struct Tracing;

impl JsonProtocol for Tracing {
    fn sniff(&self, v: &Value) -> f32 {
        let o = match v.as_object() { Some(m) => m, None => return 0.0 };
        let mut score = 0.0f32;
        if o.get("level").and_then(Value::as_str).is_some() { score += 0.35; }
        if o.get("target").and_then(Value::as_str).is_some() { score += 0.35; }
        if o.get("fields").and_then(Value::as_object).and_then(|f| f.get("message")).and_then(Value::as_str).is_some() { score += 0.25; }
        if o.get("timestamp").is_some() { score += 0.05; }
        score.min(1.0)
    }

    fn render(&self, v: &Value, ctx: RenderCtx, out: &mut dyn Write) -> io::Result<bool> {
        let obj = match v.as_object() { Some(m) => m, None => return Ok(false) };

        let level = obj.get("level").and_then(Value::as_str);
        let target = obj.get("target").and_then(Value::as_str);
        let fields = obj.get("fields").and_then(Value::as_object);
        let message = fields.and_then(|f| f.get("message")).and_then(Value::as_str);
        if level.is_none() || target.is_none() || message.is_none() { return Ok(false); }

        let (lvl_color, lvl) = match level.unwrap() {
            "ERROR" | "error" => (ctx.pal.error, "ERROR"),
            "WARN" | "warn" => (ctx.pal.warn, "WARN"),
            "INFO" | "info" => (ctx.pal.info, "INFO"),
            other => (ctx.pal.faint, other),
        };

        let timestamp = obj.get("timestamp").and_then(Value::as_str).unwrap_or_default();
        let thread_id = obj.get("threadId").and_then(Value::as_str);
        let span = obj.get("span").and_then(Value::as_object).and_then(|s| s.get("name")).and_then(Value::as_str);

        // Compute indent columns for continuation: [ts] + space (if any) + 5-char level + 1 space
        let mut indent_cols: usize = 0;
        if ctx.show_ts && !timestamp.is_empty() {
            write!(out, "[{}] ", timestamp)?;
            indent_cols += 2 + timestamp.len() + 1; // '[' + ']' + ts + space
        }
        // Fixed-width level (5 chars) and no dash before message
        let lvl_fixed = format!("{:<5}", lvl);
        write!(out, "{}{}{} ", lvl_color, lvl_fixed, ctx.pal.reset)?;
        indent_cols += 5 + 1; // level field + space
        write!(out, "{}", message.unwrap())?;

        if ctx.compact {
            // Single-line: append logger/target and other details inline
            write!(out, " logger={}", target.unwrap())?;
            if let Some(span_name) = span { write!(out, " span={}", span_name)?; }
            if let Some(tid) = thread_id { write!(out, " threadId={}", tid)?; }
        } else {
            // Pretty: move the logger/target and details to the next aligned continuation line
            out.write_all(b"\n")?;
            // write indent spaces to align under the message
            let mut spaces = vec![b' '; indent_cols];
            out.write_all(&spaces)?;
            write!(out, "logger={}", target.unwrap())?;
            if let Some(span_name) = span { write!(out, " span={}", span_name)?; }
            if let Some(tid) = thread_id { write!(out, " threadId={}", tid)?; }
        }
        if let Some(fobj) = fields {
            for (k, val) in fobj {
                if k == "message" { continue; }
                if ctx.compact {
                    write!(out, " {}=", k)?;
                } else {
                    write!(out, " {}=", k)?;
                }
                write_json_atom(&mut *out, val)?;
            }
        }
        if let Some(spans) = obj.get("spans").and_then(Value::as_array) {
            if !spans.is_empty() { write!(out, " spans=")?; write!(out, "{}", spans.len())?; }
        }
        out.write_all(b"\n")?;
        Ok(true)
    }
}
