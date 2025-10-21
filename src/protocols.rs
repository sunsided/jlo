pub mod nginx;
pub mod tracing;

use std::io::{self, Write};
use serde_json::Value;

use crate::RenderCtx;

pub trait JsonProtocol {
    /// Return a confidence score in [0.0, 1.0] indicating how likely this
    /// protocol can render the given JSON value.
    fn sniff(&self, v: &Value) -> f32;

    /// Attempt to render the given JSON value. Returns Ok(true) if rendered,
    /// Ok(false) if not applicable.
    fn render(&self, v: &Value, ctx: RenderCtx, out: &mut dyn Write) -> io::Result<bool>;
}
