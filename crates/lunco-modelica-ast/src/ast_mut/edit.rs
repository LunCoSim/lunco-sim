//! Source-preserving structural edits record byte-range splices. Only claimed
//! ranges change; untouched comments, declarations, and formatting are copied
//! verbatim. New nodes are rendered through `crate::pretty`.
//! See `docs/architecture/29-rumoca-runtime-contracts.md`.

use std::ops::Range;

use super::errors::AstMutError;

/// One byte-range replacement against the original source.
#[derive(Debug, Clone)]
struct Splice {
    /// Byte range in the original UTF-8 source to replace.
    range: Range<usize>,
    /// Replacement text for [`Self::range`].
    text: String,
}

/// Accumulates the splices of a single op, then merges them into one patch.
pub struct Edit<'a> {
    source: &'a str,
    splices: Vec<Splice>,
}

impl<'a> Edit<'a> {
    /// Start an edit against `source`.
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

    /// Replace a byte range in the original source with `text`.
    pub fn replace(&mut self, range: Range<usize>, text: impl Into<String>) {
        self.splices.push(Splice {
            range,
            text: text.into(),
        });
    }

    /// Insert `text` at a byte offset in the original source.
    pub fn insert(&mut self, at: usize, text: impl Into<String>) {
        self.replace(at..at, text);
    }

    /// Delete a byte range from the original source.
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
