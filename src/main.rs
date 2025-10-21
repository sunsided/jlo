mod pretty;

use clap::{ArgAction, Parser, ValueEnum};
use serde::Serialize;
use serde_json::{ser::Formatter, Value};
use std::fs::File;
use std::io::{self, BufRead, BufReader, LineWriter, Read, Write};
use std::ops::{Deref, DerefMut};
use std::io::IsTerminal;
use crate::pretty::TwoSpacePretty;
// std >= 1.70

/// logsniff: read NDJSON/JSON Lines, reformat, flush per line, ignore non-JSON.
#[derive(Parser, Debug)]
#[command(version, about)]
struct Cli {
    /// Compact output instead of pretty
    #[arg(short, long, action = ArgAction::SetTrue)]
    compact: bool,

    /// Show or hide timestamp (default: true). Example: --timestamp=false
    #[arg(long, action = ArgAction::SetTrue, default_value_t = false)]
    timestamp: bool,

    /// Color output: auto|always|never (default: auto)
    #[arg(long, value_enum, default_value_t = ColorChoice::Auto)]
    color: ColorChoice,

    /// Input files (read stdin if none). Each file is treated as JSON Lines.
    files: Vec<String>,
}

#[derive(Copy, Clone, Debug, ValueEnum)]
enum ColorChoice { Auto, Always, Never }

#[derive(Copy, Clone)]
struct Palette {
    enabled: bool,
    info: &'static str,
    warn: &'static str,
    error: &'static str,
    status3xx: &'static str,
    faint: &'static str,
    reset: &'static str,
}
impl Palette {
    fn new(enabled: bool) -> Self {
        if enabled {
            Self {
                enabled,
                info: "\x1b[32m",   // green
                warn: "\x1b[33m",   // yellow
                error: "\x1b[31m",  // red
                status3xx: "\x1b[36m", // cyan
                faint: "\x1b[2m",
                reset: "\x1b[0m",
            }
        } else {
            Self { enabled, info: "", warn: "", error: "", status3xx: "", faint: "", reset: "" }
        }
    }
}

#[derive(Copy, Clone)]
struct RenderCtx {
    show_ts: bool,
    pal: Palette,
}

fn main() -> io::Result<()> {
    let cli = Cli::parse();

    let want_ts = cli.timestamp;
    let stdout_is_tty = io::stdout().is_terminal();
    let colors_enabled = match cli.color {
        ColorChoice::Auto => stdout_is_tty,
        ColorChoice::Always => true,
        ColorChoice::Never => false,
    };
    let ctx = RenderCtx { show_ts: want_ts, pal: Palette::new(colors_enabled) };

    let stdout = io::stdout();
    let handle = stdout.lock();
    let mut out = LineWriter::new(handle);

    if cli.files.is_empty() {
        process_reader(BufReader::new(io::stdin().lock()), cli.compact, ctx, &mut out)?;
    } else {
        for path in &cli.files {
            let file = File::open(path)?;
            process_reader(BufReader::new(file), cli.compact, ctx, &mut out)?;
        }
    }

    out.flush()
}

fn process_reader<R: Read, W: Write>(
    mut reader: BufReader<R>,
    compact: bool,
    ctx: RenderCtx,
    mut out: &mut W,
) -> io::Result<()> {
    let mut buf = Vec::with_capacity(8 * 1024);

    loop {
        buf.clear();
        let n = reader.read_until(b'\n', &mut buf)?;
        if n == 0 { break; }
        while matches!(buf.last(), Some(b'\n' | b'\r')) { buf.pop(); }
        if buf.is_empty() { continue; }

        match serde_json::from_slice::<Value>(&buf) {
            Ok(v) => {
                if render_nginx_like(&v, ctx, out.deref_mut())? {
                    // done
                } else if render_tracing_like(&v, ctx, out.deref_mut())? {
                    // done
                } else {
                    if compact {
                        serde_json::to_writer(out.deref_mut(), &v).map_err(to_io_err)?;
                        out.write_all(b"\n")?;
                    } else {
                        let mut ser = serde_json::Serializer::with_formatter(
                            out.deref_mut(), TwoSpacePretty::default());
                        v.serialize(&mut ser).map_err(to_io_err)?;
                        out.write_all(b"\n")?;
                    }
                }
            }
            Err(_) => { /* ignore */ }
        }
    }
    Ok(())
}

/// Detect & render NGINX-like JSON. Adds colored level + optional ts.
fn render_nginx_like<W: Write>(v: &Value, ctx: RenderCtx, mut out: W) -> io::Result<bool> {
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

    write_kv_str(&mut out, "bytes", o.get("bytes_sent").and_then(Value::as_u64).map(|n| n.to_string()).as_deref())?;
    write_kv_num(&mut out, "rt", o.get("req_time").and_then(Value::as_f64))?;
    write_kv_num(&mut out, "up", o.get("upstream_time").and_then(as_f64_lossy))?;
    write_kv_str(&mut out, "up_addr", o.get("upstream_addr").and_then(Value::as_str))?;
    write_kv_str(&mut out, "req", o.get("req_id").and_then(Value::as_str))?;
    write_kv_str(&mut out, "trace", o.get("traceparent").and_then(Value::as_str))?;
    write_kv_str(&mut out, "xff", o.get("xff").and_then(Value::as_str))?;
    if let Some(ip) = remote_addr { write_kv_str(&mut out, "client", Some(ip))?; }
    write_kv_str(&mut out, "referer", o.get("referer").and_then(Value::as_str))?;
    write_kv_str(&mut out, "ua", o.get("user_agent").and_then(Value::as_str))?;

    if let Some(cache) = o.get("cache").and_then(Value::as_str) {
        if !cache.is_empty() { write_kv_str(&mut out, "cache", Some(cache))?; }
    }

    out.write_all(b"\n")?;
    Ok(true)
}

/// Detect & render Rust `tracing` JSON. Adds colored level + optional ts.
fn render_tracing_like<W: Write>(v: &Value, ctx: RenderCtx, mut out: W) -> io::Result<bool> {
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

    if ctx.show_ts && !timestamp.is_empty() {
        write!(out, "[{}] ", timestamp)?;
    }
    write!(out, "{}{}{} {} ", lvl_color, lvl, ctx.pal.reset, target.unwrap())?;
    if let Some(span_name) = span {
        write!(out, "({}) ", span_name)?;
    }
    write!(out, "— {}", message.unwrap())?;

    if let Some(tid) = thread_id { write!(out, " threadId={}", tid)?; }
    if let Some(fobj) = fields {
        for (k, val) in fobj {
            if k == "message" { continue; }
            write!(out, " {}=", k)?;
            write_json_atom(&mut out, val)?;
        }
    }
    if let Some(spans) = obj.get("spans").and_then(Value::as_array) {
        if !spans.is_empty() { write!(out, " spans={}", spans.len())?; }
    }
    out.write_all(b"\n")?;
    Ok(true)
}

/// Helper: write key=value for string-ish fields if present & non-empty.
fn write_kv_str<W: Write>(mut out: W, key: &str, val: Option<&str>) -> io::Result<()> {
    if let Some(s) = val {
        if !s.is_empty() {
            write!(out, " {}=", key)?;
            // bare if safe, else JSON-quoted
            if s.chars().all(|c| c.is_ascii_graphic() && c != ' ' && c != '=') {
                write!(out, "{}", s)?;
            } else {
                let mut buf = Vec::new();
                serde_json::to_writer(&mut buf, &Value::String(s.to_string())).map_err(to_io_err)?;
                out.write_all(&buf)?;
            }
        }
    }
    Ok(())
}

/// Helper: write key=value for numeric (f64) with trimmed trailing zeros.
fn write_kv_num<W: Write>(mut out: W, key: &str, val: Option<f64>) -> io::Result<()> {
    if let Some(mut f) = val {
        if f == -0.0 {
            f = 0.0;
        }
        write!(out, " {}=", key)?;
        // Trim trailing zeros
        let s = format!("{:.6}", f);
        let s = s.trim_end_matches('0').trim_end_matches('.');
        write!(out, "{}", s)?;
    }
    Ok(())
}

/// Write a compact single-atom JSON value for key=value lists.
///
/// Strings are printed without quotes when safe (no spaces or `=`),
/// everything else is serialized as compact JSON.
fn write_json_atom<W: Write>(mut out: W, v: &Value) -> io::Result<()> {
    match v {
        Value::String(s) => {
            if s.chars().all(|c| c.is_ascii_graphic() && c != ' ' && c != '=') {
                // Safe to print bare
                write!(out, "{}", s)?;
            } else {
                // Fallback to proper JSON string escaping
                let mut buf = Vec::new();
                serde_json::to_writer(&mut buf, v).map_err(to_io_err)?;
                out.write_all(&buf)?;
            }
        }
        _ => {
            // Non-string → write as compact JSON
            let mut buf = Vec::new();
            serde_json::to_writer(&mut buf, v).map_err(to_io_err)?;
            out.write_all(&buf)?;
        }
    }
    Ok(())
}

/// Map arbitrary errors into `io::Error` so callers can stay on `io::Result`.
fn to_io_err<E: std::error::Error + Send + Sync + 'static>(e: E) -> std::io::Error {
    io::Error::new(io::ErrorKind::Other, e)
}

/// Some fields come as strings like `"0.053"`. Parse leniently into f64.
fn as_f64_lossy(v: &Value) -> Option<f64> {
    v.as_f64().or_else(|| v.as_str()?.parse::<f64>().ok())
}
