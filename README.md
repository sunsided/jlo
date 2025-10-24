# jlo — JSON Logging in Plain View

[![Crates.io](https://img.shields.io/crates/v/jlo.svg)](https://crates.io/crates/jlo)
[![Docs.rs](https://img.shields.io/badge/docs.rs-jlo-blue)](https://docs.rs/jlo)
[![License: EUPL-1.2](https://img.shields.io/badge/License-EUPL--1.2-blue.svg)](https://joinup.ec.europa.eu/collection/eupl/eupl-text-eupl-12)

`jlo` is a Rust CLI tool for reading, pretty-printing, and colorizing JSON log files (NDJSON/JSON Lines). It automatically detects and formats logs from common sources like Nginx and Rust's tracing, making structured logs easier to inspect in the terminal.

## Features

- Reads NDJSON/JSON Lines from files or stdin
- Pretty or compact output
- Colorizes log output by severity
- Protocol-specific formatting for Nginx and Rust tracing logs
- Ignores non-JSON lines

## Example Usage

```sh
jlo access.log
cat app.log | jlo
cat app.log | jlo --compact --color always
```

## Installation

Install via Cargo:

```shell
cargo install jlo
```

If you prefer bleeding edge:

```shell
cargo install --git https://github.com/sunsided/jlo
```

## License

Licensed under the European Union Public Licence (EUPL), Version 1.2.
