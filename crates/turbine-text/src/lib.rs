//! Turbine's text model: documents on a rope, encoding detection for
//! modern and DOS-era files, grapheme-aware widths and bidi layout.

pub mod document;
pub mod codepages;
pub mod encoding;
pub mod unicode;

pub use document::Document;
pub use encoding::{LineEnding, TextEncoding};
pub use ropey;
