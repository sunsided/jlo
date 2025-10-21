mod pretty;
mod protocols;

use clap::{ArgAction, Parser, ValueEnum};
use serde::Serialize;
use serde_json::Value;
use std::fs::File;
use std::io::{self, BufRead, BufReader, LineWriter, Read, Write};
use std::ops::DerefMut;
use std::io::IsTerminal;
use crate::pretty::TwoSpacePretty;

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
pub(crate) struct Palette {
    pub(crate) enabled: bool,
    pub(crate) info: &'static str,
    pub(crate) warn: &'static str,
    pub(crate) error: &'static str,
    pub(crate) status3xx: &'static str,
    pub(crate) faint: &'static str,
    pub(crate) reset: &'static str,
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
pub(crate) struct RenderCtx {
    pub(crate) show_ts: bool,
    pub(crate) pal: Palette,
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
                use crate::protocols::{self, JsonProtocol};
                let protos: [&dyn JsonProtocol; 2] = [&protocols::nginx::Nginx, &protocols::tracing::Tracing];
                let mut best: Option<(&dyn JsonProtocol, f32)> = None;
                for p in protos.iter().copied() {
                    let s = p.sniff(&v);
                    if let Some((_, bs)) = best {
                        if s > bs { best = Some((p, s)); }
                    } else {
                        best = Some((p, s));
                    }
                }
                let mut rendered = false;
                if let Some((p, score)) = best {
                    if score > 0.0 {
                        rendered = p.render(&v, ctx, out.deref_mut())?;
                    }
                }
                if !rendered {
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

/// Helper: write key=value for string-ish fields if present & non-empty.
pub(crate) fn write_kv_str<W: Write>(mut out: W, key: &str, val: Option<&str>) -> io::Result<()> {
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
pub(crate) fn write_kv_num<W: Write>(mut out: W, key: &str, val: Option<f64>) -> io::Result<()> {
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
pub(crate) fn write_json_atom<W: Write>(mut out: W, v: &Value) -> io::Result<()> {
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
pub(crate) fn to_io_err<E: std::error::Error + Send + Sync + 'static>(e: E) -> std::io::Error {
    io::Error::new(io::ErrorKind::Other, e)
}

/// Some fields come as strings like `"0.053"`. Parse leniently into f64.
pub(crate) fn as_f64_lossy(v: &Value) -> Option<f64> {
    v.as_f64().or_else(|| v.as_str()?.parse::<f64>().ok())
}
