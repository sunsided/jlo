use serde_json::ser::PrettyFormatter;
use std::ops::{Deref, DerefMut};

/// Pretty formatter with two-space indentation for `serde_json::Serializer`.
pub struct TwoSpacePretty(PrettyFormatter<'static>);

impl Default for TwoSpacePretty {
    fn default() -> Self {
        TwoSpacePretty(PrettyFormatter::with_indent(b"  "))
    }
}

impl Deref for TwoSpacePretty {
    type Target = PrettyFormatter<'static>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for TwoSpacePretty {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

/// Allow use as a `Formatter` directly.
impl serde_json::ser::Formatter for TwoSpacePretty {}
