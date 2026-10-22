//! The layout layer the Turtle and SPARQL formatters share: the layout
//! [`Options`]; in `doc`, the token document, which splits a token stream
//! into the tokens to lay out and the comments, and attaches each comment to
//! a token; in `terms`, the triple terms (`Term`, `Obj`, `Po`) and their
//! widths; in `printer`, the `Printer` both write through, which also lays
//! out the triple terms. Each formatter keeps its own walk of the syntax and
//! its language's rules. The JSON-LD formatter takes only the options.
//!
//! Depends on nothing above `syntax` and the SPARQL tokens: the LSP adapter
//! `Options::from_lsp` lives in `features::format`.

mod doc;
mod printer;
mod terms;

pub(crate) use doc::{BIG, Doc, Ix, LayoutToken};
pub(crate) use printer::{Blank, Groups, Printer, Semicolons, Style};
pub(crate) use terms::{Obj, Po, Term};

/// The line width the formatters fill when the client gives none.
pub(crate) const DEFAULT_LINE_WIDTH: usize = 100;

/// How a formatter indents and how wide it fills a line.
#[derive(Clone, Debug)]
pub struct Options {
    pub tab_size: u32,
    pub insert_spaces: bool,
    pub line_width: usize,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            tab_size: 4,
            insert_spaces: true,
            line_width: DEFAULT_LINE_WIDTH,
        }
    }
}
