// src/main.rs
use clap::{ArgAction, Parser};
use std::fs::File;
use std::io::{self, BufRead, BufReader, LineWriter, Read, Write};
use std::ops::DerefMut;
use serde::Serialize;
use serde_json::{Value, ser::Formatter};

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
                if compact {
                    serde_json::to_writer(out.deref_mut(), &v).map_err(to_io_err)?;
                    out.write_all(b"\n")?;
                } else {
                    // Pretty with 2-space indent
                    let mut ser =
                        serde_json::Serializer::with_formatter(out.deref_mut(), TwoSpacePretty::default());
                    v.serialize(&mut ser).map_err(to_io_err)?;
                    out.write_all(b"\n")?;
                }
                // LineWriter flushes on newline; nothing else to do.
            }
            Err(_) => {
                // Ignore unhandled/invalid JSON lines silently.
                // (Deliberately do nothing.)
            }
        }
    }

    Ok(())
}

// Pretty formatter with two spaces (serde_json defaults to 2, but we ensure it explicitly)
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

impl Formatter for TwoSpacePretty {
}

// Map serde_json errors into io::Error for unified ? handling above
fn to_io_err<E: std::error::Error + Send + Sync + 'static>(e: E) -> io::Error {
    io::Error::new(io::ErrorKind::Other, e)
}
