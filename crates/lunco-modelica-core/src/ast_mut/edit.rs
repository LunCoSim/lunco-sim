//! The splice accumulator — how a structural edit becomes a text patch.
//!
//! # Why this exists
//!
//! A structural op used to be turned into text by re-emitting the whole class
//! through rumoca's `to_modelica()`. That is a *lossy* round-trip: it drops
//! comments, and its component emitter mis-renders any declaration carrying
//! both a `start` modifier and a binding. So dragging one icon on the canvas
//! silently rewrote every sibling declaration in the class — a
//! `parameter Real k = 2.0` came back as `k = 0.0`. Wrong numbers, no error.
//!
//! The rule now: **an edit may only touch the bytes it means to change.**
//! Mutations record [`Splice`]s against the original source; everything else is
//! copied through verbatim. Comments, formatting and untouched declarations
//! survive because they are never re-emitted — not because the emitter is
//! careful. New nodes are rendered by [`crate::pretty`], which is ours.
//!
//! See `docs/architecture/29-rumoca-workarounds.md` §5.

use std::ops::Range;

use super::errors::AstMutError;

/// One byte-range replacement against the original source.
#[derive(Debug, Clone)]
pub struct Splice {
    pub range: Range<usize>,
    pub text: String,
}

/// Accumulates the splices of a single op, then merges them into one patch.
pub struct Edit<'a> {
    source: &'a str,
    splices: Vec<Splice>,
}

impl<'a> Edit<'a> {
    pub fn new(source: &'a str) -> Self {
        Self {
            source,
            splices: Vec::new(),
        }
    }

    /// The source being edited. Mutations read spans out of it; they must never
    /// write to it except through the methods below.
    pub fn source(&self) -> &'a str {
        self.source
    }

    pub fn replace(&mut self, range: Range<usize>, text: impl Into<String>) {
        self.splices.push(Splice {
            range,
            text: text.into(),
        });
    }

    pub fn insert(&mut self, at: usize, text: impl Into<String>) {
        self.replace(at..at, text);
    }

    pub fn delete(&mut self, range: Range<usize>) {
        self.replace(range, "");
    }

    /// Merge into the single `(range, replacement)` the document layer applies.
    ///
    /// The range spans from the first splice to the last; the bytes *between*
    /// splices are copied from the original source unchanged. That is the whole
    /// point — a sibling declaration inside the covered range is preserved
    /// byte-for-byte because no splice claims it.
    pub fn into_patch(mut self) -> Result<(Range<usize>, String), AstMutError> {
        if self.splices.is_empty() {
            // A mutation that changed nothing. Emit an empty patch at 0 rather
            // than rewriting the document with identical text.
            return Ok((0..0, String::new()));
        }
        self.splices.sort_by_key(|s| (s.range.start, s.range.end));

        // Overlapping splices mean two mutations claimed the same bytes; the
        // merge below would silently drop one. That's a bug in the caller.
        for pair in self.splices.windows(2) {
            if pair[1].range.start < pair[0].range.end {
                return Err(AstMutError::OverlappingSplice {
                    first: format!("{:?}", pair[0].range),
                    second: format!("{:?}", pair[1].range),
                });
            }
        }

        let start = self.splices[0].range.start;
        let end = self
            .splices
            .last()
            .expect("non-empty, checked above")
            .range
            .end;
        if end > self.source.len() {
            return Err(AstMutError::SpliceOutOfBounds {
                end,
                len: self.source.len(),
            });
        }

        let mut out = String::new();
        let mut cursor = start;
        for splice in &self.splices {
            out.push_str(&self.source[cursor..splice.range.start]);
            out.push_str(&splice.text);
            cursor = splice.range.end;
        }
        out.push_str(&self.source[cursor..end]);
        Ok((start..end, out))
    }
}
