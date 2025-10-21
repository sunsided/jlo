use clap::{ArgAction, Parser};
use serde::Serialize;
use serde_json::{ser::Formatter, Value};
use std::fs::File;
use std::io::{self, BufRead, BufReader, LineWriter, Read, Write};
use std::ops::DerefMut;

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

    // Line-buffered stdout: flushes on '\n' regardless of TTY/pipe.
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

        // Trim trailing '\n' and optional '\r'
        while matches!(buf.last(), Some(b'\n' | b'\r')) {
            buf.pop();
        }
        if buf.is_empty() {
            continue;
        }

        // Best-effort JSON parse; skip on failure.
        match serde_json::from_slice::<Value>(&buf) {
            Ok(v) => {
                if !render_tracing_like(&v, out.deref_mut())? {
                    // Fallback to plain JSON reformatting
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
                // LineWriter flushes on newline; nothing else to do.
            }
            Err(_) => {
                // Ignore unhandled/invalid JSON lines silently.
            }
        }
    }

    Ok(())
}

/// Try to detect and render Rust `tracing` JSON (or similar) in a structured one-liner:
/// `[timestamp] LEVEL target — message key1=value key2=value ...`
/// Returns true if it rendered; false if not a match.
fn render_tracing_like<W: Write>(v: &Value, mut out: W) -> io::Result<bool> {
    let obj = match v.as_object() {
        Some(m) => m,
        None => return Ok(false),
    };

    // Heuristic: require "level" (string), "target" (string), and fields.message (string)
    let level = obj.get("level").and_then(Value::as_str);
    let target = obj.get("target").and_then(Value::as_str);
    let fields = obj.get("fields").and_then(Value::as_object);
    let message = fields
        .and_then(|f| f.get("message"))
        .and_then(Value::as_str);

    if level.is_none() || target.is_none() || message.is_none() {
        return Ok(false);
    }

    // Optional parts commonly present in tracing outputs:
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

    // Header: timestamp, level, target, span
    if !timestamp.is_empty() {
        write!(out, "[{}] ", timestamp)?;
    }
    write!(out, "{} {} ", level.unwrap(), target.unwrap())?;
    if let Some(span_name) = span {
        write!(out, "({}) ", span_name)?;
    }

    // Separator before message
    write!(out, "— ")?;
    write!(out, "{}", message.unwrap())?;

    // Append threadId if present
    if let Some(tid) = thread_id {
        write!(out, " threadId={}", tid)?;
    }

    // Append remaining fields from `fields` except `message`
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

    // Optionally include a compact summary of top-level `spans` length
    if let Some(spans) = obj.get("spans").and_then(Value::as_array) {
        if !spans.is_empty() {
            write!(out, " spans={}", spans.len())?;
        }
    }

    out.write_all(b"\n")?;
    Ok(true)
}

/// Write a compact single-atom JSON value for key=value lists.
/// Strings print without surrounding quotes when safe; everything else is compact JSON.
fn write_json_atom<W: Write>(mut out: W, v: &Value) -> io::Result<()> {
    match v {
        Value::String(s) => {
            if s.chars().all(|c| c.is_ascii_graphic() && c != ' ' && c != '=') {
                // Print bare if safe (no spaces/equals)
                write!(out, "{}", s)?;
            } else {
                // JSON-escape otherwise
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

impl std::ops::Deref for TwoSpacePretty {
    type Target = serde_json::ser::PrettyFormatter<'static>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
impl std::ops::DerefMut for TwoSpacePretty {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl Formatter for TwoSpacePretty {}

fn to_io_err<E: std::error::Error + Send + Sync + 'static>(e: E) -> io::Error {
    io::Error::new(io::ErrorKind::Other, e)
}
