use clap::{ArgAction, Parser};
use serde::Serialize;
use serde_json::{ser::Formatter, Value};
use std::fs::File;
use std::io::{self, BufRead, BufReader, LineWriter, Read, Write};
use std::ops::{Deref, DerefMut};

/// logsniff: read NDJSON/JSON Lines, reformat, flush per line, ignore non-JSON.
#[derive(Parser, Debug)]
#[command(version, about)]
struct Cli {
    /// Compact output instead of pretty
    #[arg(short, long, action = ArgAction::SetTrue)]
    compact: bool,

    /// Input files (read stdin if none). Each file is treated as JSON Lines.
    files: Vec<String>,
}

fn main() -> io::Result<()> {
    let cli = Cli::parse();

    let stdout = io::stdout();
    let handle = stdout.lock();
    let mut out = LineWriter::new(handle);

    if cli.files.is_empty() {
        process_reader(BufReader::new(io::stdin().lock()), cli.compact, &mut out)?;
    } else {
        for path in &cli.files {
            let file = File::open(path)?;
            process_reader(BufReader::new(file), cli.compact, &mut out)?;
        }
    }

    out.flush()
}

fn process_reader<R: Read, W: Write>(
    mut reader: BufReader<R>,
    compact: bool,
    mut out: &mut W,
) -> io::Result<()> {
    let mut buf = Vec::with_capacity(8 * 1024);

    loop {
        buf.clear();
        let n = reader.read_until(b'\n', &mut buf)?;
        if n == 0 {
            break; // EOF
        }
        while matches!(buf.last(), Some(b'\n' | b'\r')) {
            buf.pop();
        }
        if buf.is_empty() {
            continue;
        }

        match serde_json::from_slice::<Value>(&buf) {
            Ok(v) => {
                if render_nginx_like(&v, out.deref_mut())? {
                    // done
                } else if render_tracing_like(&v, out.deref_mut())? {
                    // done
                } else {
                    // fallback: reformat JSON
                    if compact {
                        serde_json::to_writer(out.deref_mut(), &v).map_err(to_io_err)?;
                        out.write_all(b"\n")?;
                    } else {
                        let mut ser = serde_json::Serializer::with_formatter(
                            out.deref_mut(),
                            TwoSpacePretty::default(),
                        );
                        v.serialize(&mut ser).map_err(to_io_err)?;
                        out.write_all(b"\n")?;
                    }
                }
            }
            Err(_) => {
                // ignore
            }
        }
    }

    Ok(())
}

/// Detect & render NGINX-like JSON (fields like ts, method, path, status, host, xff, etc.)
/// Example output:
/// [2025-10-21T15:35:41+00:00] 200 GET example.org /login?callbackUrl=/ HTTP/1.1 — bytes=6814 rt=0.053 up=0.053 up_addr=127.0.0.1:3000 req=809a... xff="10.253.54.150" ua="GlitchTip/5.1.1" referer=""
/// Detect & render NGINX-like JSON (fields like ts, method, path, status, host, xff, etc.)
fn render_nginx_like<W: Write>(v: &Value, mut out: W) -> io::Result<bool> {
    let o = match v.as_object() {
        Some(m) => m,
        None => return Ok(false),
    };

    let ts = o.get("ts").and_then(Value::as_str);
    let method = o.get("method").and_then(Value::as_str);
    let path = o.get("path").and_then(Value::as_str);
    let status = o.get("status").and_then(Value::as_u64).or_else(|| {
        o.get("status").and_then(Value::as_str).and_then(|s| s.parse::<u64>().ok())
    });
    if method.is_none() || path.is_none() || status.is_none() {
        return Ok(false);
    }

    // Map status → pseudo log level
    let level = match status.unwrap() {
        100..=399 => "INFO",
        400..=499 => "WARN",
        500..=599 => "ERROR",
        _ => "INFO",
    };

    let protocol = o.get("protocol").and_then(Value::as_str).unwrap_or("");
    let query = o.get("query").and_then(Value::as_str).unwrap_or("");
    let host = o.get("host").and_then(Value::as_str).unwrap_or("");
    let remote_addr = o.get("remote_addr").and_then(Value::as_str);

    if let Some(ts) = ts {
        write!(out, "[{}] ", ts)?;
    }
    // prepend level
    write!(out, "{} ", level)?;
    // status and request line
    write!(out, "{} {} ", status.unwrap(), method.unwrap())?;
    if !host.is_empty() {
        write!(out, "{} ", host)?;
    }

    write!(out, "{}", path.unwrap())?;
    if !query.is_empty() {
        write!(out, "?{}", query)?;
    }
    if !protocol.is_empty() {
        write!(out, " {}", protocol)?;
    }

    // Separator
    write!(out, " —")?;

    // key-values
    write_kv_str(&mut out, "bytes", o.get("bytes_sent").and_then(Value::as_u64).map(|n| n.to_string()).as_deref())?;
    write_kv_num(&mut out, "rt", o.get("req_time").and_then(Value::as_f64))?;
    write_kv_num(&mut out, "up", o.get("upstream_time").and_then(as_f64_lossy))?;
    write_kv_str(&mut out, "up_addr", o.get("upstream_addr").and_then(Value::as_str))?;
    write_kv_str(&mut out, "req", o.get("req_id").and_then(Value::as_str))?;
    write_kv_str(&mut out, "trace", o.get("traceparent").and_then(Value::as_str))?;
    write_kv_str(&mut out, "xff", o.get("xff").and_then(Value::as_str))?;
    if let Some(ip) = remote_addr {
        write_kv_str(&mut out, "client", Some(ip))?;
    }
    write_kv_str(&mut out, "referer", o.get("referer").and_then(Value::as_str))?;
    write_kv_str(&mut out, "ua", o.get("user_agent").and_then(Value::as_str))?;

    if let Some(cache) = o.get("cache").and_then(Value::as_str) {
        if !cache.is_empty() {
            write_kv_str(&mut out, "cache", Some(cache))?;
        }
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
        // Avoid negative zeros, normalize very small values
        if f == -0.0 { f = 0.0; }
        write!(out, " {}=", key)?;
        // Trim trailing zeros in a simple way
        let s = format!("{:.6}", f);
        let s = s.trim_end_matches('0').trim_end_matches('.');
        write!(out, "{}", s)?;
    }
    Ok(())
}

/// Some fields come as strings like "0.053". Parse leniently.
fn as_f64_lossy(v: &Value) -> Option<f64> {
    v.as_f64().or_else(|| v.as_str()?.parse::<f64>().ok())
}

/// Try to detect and render Rust `tracing` JSON in a structured one-liner.
fn render_tracing_like<W: Write>(v: &Value, mut out: W) -> io::Result<bool> {
    let obj = match v.as_object() {
        Some(m) => m,
        None => return Ok(false),
    };

    let level = obj.get("level").and_then(Value::as_str);
    let target = obj.get("target").and_then(Value::as_str);
    let fields = obj.get("fields").and_then(Value::as_object);
    let message = fields
        .and_then(|f| f.get("message"))
        .and_then(Value::as_str);

    if level.is_none() || target.is_none() || message.is_none() {
        return Ok(false);
    }

    let timestamp = obj
        .get("timestamp")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let thread_id = obj.get("threadId").and_then(Value::as_str);
    let span = obj
        .get("span")
        .and_then(Value::as_object)
        .and_then(|s| s.get("name"))
        .and_then(Value::as_str);

    if !timestamp.is_empty() {
        write!(out, "[{}] ", timestamp)?;
    }
    write!(out, "{} {} ", level.unwrap(), target.unwrap())?;
    if let Some(span_name) = span {
        write!(out, "({}) ", span_name)?;
    }
    write!(out, "— {}", message.unwrap())?;

    if let Some(tid) = thread_id {
        write!(out, " threadId={}", tid)?;
    }
    if let Some(fobj) = fields {
        for (k, val) in fobj {
            if k == "message" {
                continue;
            }
            write!(out, " ")?;
            write!(out, "{}", k)?;
            write!(out, "=")?;
            write_json_atom(&mut out, val)?;
        }
    }
    if let Some(spans) = obj.get("spans").and_then(Value::as_array) {
        if !spans.is_empty() {
            write!(out, " spans={}", spans.len())?;
        }
    }
    out.write_all(b"\n")?;
    Ok(true)
}

/// Write a compact single-atom JSON value for key=value lists.
fn write_json_atom<W: Write>(mut out: W, v: &Value) -> io::Result<()> {
    match v {
        Value::String(s) => {
            if s.chars().all(|c| c.is_ascii_graphic() && c != ' ' && c != '=') {
                write!(out, "{}", s)?;
            } else {
                let mut buf = Vec::new();
                serde_json::to_writer(&mut buf, v).map_err(to_io_err)?;
                out.write_all(&buf)?;
            }
        }
        _ => {
            let mut buf = Vec::new();
            serde_json::to_writer(&mut buf, v).map_err(to_io_err)?;
            out.write_all(&buf)?;
        }
    }
    Ok(())
}

// Pretty formatter with two spaces
struct TwoSpacePretty(serde_json::ser::PrettyFormatter<'static>);

impl Default for TwoSpacePretty {
    fn default() -> Self {
        TwoSpacePretty(serde_json::ser::PrettyFormatter::with_indent(b"  "))
    }
}

impl Deref for TwoSpacePretty {
    type Target = serde_json::ser::PrettyFormatter<'static>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for TwoSpacePretty {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl Formatter for TwoSpacePretty {}

fn to_io_err<E: std::error::Error + Send + Sync + 'static>(e: E) -> io::Error {
    io::Error::new(io::ErrorKind::Other, e)
}
