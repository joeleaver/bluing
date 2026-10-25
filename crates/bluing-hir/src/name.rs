use std::fmt;

use bluing_syntax::ra_ap_syntax::SmolStr;

/// An identifier as the language sees it: `r#type` and `type` are the same
/// name, so the `r#` prefix is dropped.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Name(SmolStr);

impl Name {
    pub fn new(text: &str) -> Self {
        Name(SmolStr::new(text.strip_prefix("r#").unwrap_or(text)))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Name {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for Name {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self.0.as_str())
    }
}
