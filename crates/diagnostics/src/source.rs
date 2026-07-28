//! Immutable source snapshots and coordinate conversions for diagnostics.
//!
//! Faust's historical diagnostics expose file/line/column positions.  The Rust
//! port keeps that compatibility surface through [`SourceSpan`], while this
//! module defines the canonical internal representation used by diagnostics
//! v2: half-open UTF-8 byte ranges into immutable compilation snapshots.
//!
//! # Invariants
//!
//! - [`SourceRange`] bounds are UTF-8 character boundaries and `start <= end`;
//! - source identifiers are local to one [`SourceMap`];
//! - line breaks accept LF, CRLF, and bare CR without normalizing source text;
//! - human positions are 1-based display cells with four-column tab stops;
//! - LSP positions are 0-based UTF-16 code-unit offsets;
//! - source text and its SHA-256 [`ContentHash`] never change after construction.

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use sha2::{Digest, Sha256};
use unicode_width::UnicodeWidthChar;

use crate::SourceSpan;

const HUMAN_TAB_WIDTH: u32 = 4;

/// Compact identifier for one source snapshot inside a [`SourceMap`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SourceId(u32);

impl SourceId {
    /// Returns the map-local numeric identifier.
    #[must_use]
    pub const fn as_u32(self) -> u32 {
        self.0
    }
}

/// How a source entered one compilation session.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SourceKind {
    /// Entry source loaded from a host file.
    File,
    /// Entry source supplied directly as an in-memory string.
    Memory,
    /// Imported source loaded from a host file.
    ImportedFile,
    /// Imported logical source supplied by an embedded/virtual library bundle.
    VirtualLibrary,
}

/// SHA-256 digest of the exact compiled source snapshot.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct ContentHash([u8; 32]);

impl ContentHash {
    /// Computes a content hash from UTF-8 source bytes.
    #[must_use]
    pub fn of(source: &str) -> Self {
        Self(Sha256::digest(source.as_bytes()).into())
    }

    /// Returns the raw 32-byte SHA-256 digest.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Returns the lowercase hexadecimal digest used by machine protocols.
    #[must_use]
    pub fn to_hex(self) -> String {
        use std::fmt::Write as _;

        let mut out = String::with_capacity(64);
        for byte in self.0 {
            write!(out, "{byte:02x}").expect("writing to String cannot fail");
        }
        out
    }
}

impl fmt::Debug for ContentHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("ContentHash").field(&self.to_hex()).finish()
    }
}

/// Canonical half-open UTF-8 byte range in one source snapshot.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SourceRange {
    /// Snapshot containing this range.
    pub source: SourceId,
    /// Inclusive UTF-8 byte offset.
    pub start: u32,
    /// Exclusive UTF-8 byte offset.
    pub end: u32,
}

impl SourceRange {
    /// Creates a range. Use [`SourceMap::validate_range`] before consuming
    /// ranges received from untrusted or external producers.
    #[must_use]
    pub const fn new(source: SourceId, start: u32, end: u32) -> Self {
        Self { source, start, end }
    }

    /// Returns the byte length of this half-open range.
    #[must_use]
    pub const fn len(self) -> u32 {
        self.end.saturating_sub(self.start)
    }

    /// Returns `true` for an insertion point.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.start == self.end
    }
}

/// One-based line and display-cell column for terminal diagnostics.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct HumanPosition {
    /// One-based line.
    pub line: u32,
    /// One-based display column after tab expansion and Unicode width.
    pub column: u32,
}

/// Zero-based line and UTF-16 code-unit column used by LSP clients.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct LspPosition {
    /// Zero-based line.
    pub line: u32,
    /// Zero-based UTF-16 code-unit offset.
    pub character: u32,
}

/// Error returned when a source id, byte range, or compatibility position is
/// outside its immutable snapshot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SourceCoordinateError {
    /// The source id does not belong to this map.
    UnknownSource(SourceId),
    /// A byte bound lies outside the source or `start > end`.
    InvalidRange(SourceRange),
    /// A byte bound splits one UTF-8 scalar value.
    NotCharBoundary(SourceRange),
    /// A legacy 1-based line/column cannot be mapped into the snapshot.
    InvalidSpan(SourceSpan),
}

impl fmt::Display for SourceCoordinateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownSource(id) => write!(f, "unknown source id {}", id.as_u32()),
            Self::InvalidRange(range) => {
                write!(
                    f,
                    "invalid source byte range {}..{}",
                    range.start, range.end
                )
            }
            Self::NotCharBoundary(range) => write!(
                f,
                "source byte range {}..{} splits a UTF-8 character",
                range.start, range.end
            ),
            Self::InvalidSpan(span) => write!(
                f,
                "invalid source span {}:{}:{}-{}:{}",
                span.file.display(),
                span.line,
                span.col,
                span.end_line,
                span.end_col
            ),
        }
    }
}

impl std::error::Error for SourceCoordinateError {}

/// One immutable source file/string snapshot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceFile {
    id: SourceId,
    name: PathBuf,
    kind: SourceKind,
    text: Arc<str>,
    content_hash: ContentHash,
    line_starts: Arc<[u32]>,
}

impl SourceFile {
    /// Returns this source's map-local id.
    #[must_use]
    pub const fn id(&self) -> SourceId {
        self.id
    }

    /// Returns the logical or filesystem name used by diagnostics.
    #[must_use]
    pub fn name(&self) -> &Path {
        &self.name
    }

    /// Returns how this source entered the compilation.
    #[must_use]
    pub const fn kind(&self) -> SourceKind {
        self.kind
    }

    /// Returns the exact immutable UTF-8 snapshot compiled by the parser.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Returns the SHA-256 digest of [`Self::text`].
    #[must_use]
    pub const fn content_hash(&self) -> ContentHash {
        self.content_hash
    }

    /// Returns one source line without its line terminator.
    ///
    /// `line` is one-based to match terminal diagnostics and [`SourceSpan`].
    #[must_use]
    pub fn line_text(&self, line: u32) -> Option<&str> {
        let line_index = usize::try_from(line.checked_sub(1)?).ok()?;
        let start = usize::try_from(*self.line_starts.get(line_index)?).ok()?;
        let raw_end = self
            .line_starts
            .get(line_index + 1)
            .copied()
            .map_or(self.text.len(), |value| value as usize);
        let bytes = self.text.as_bytes();
        let mut end = raw_end;
        if end > start && bytes[end - 1] == b'\n' {
            end -= 1;
            if end > start && bytes[end - 1] == b'\r' {
                end -= 1;
            }
        } else if end > start && bytes[end - 1] == b'\r' {
            end -= 1;
        }
        self.text.get(start..end)
    }

    fn line_index_for_offset(&self, offset: u32) -> usize {
        self.line_starts
            .partition_point(|start| *start <= offset)
            .saturating_sub(1)
    }

    fn byte_offset_for_scalar_column(&self, line: u32, column: u32) -> Option<u32> {
        let line_text = self.line_text(line)?;
        let line_index = usize::try_from(line.checked_sub(1)?).ok()?;
        let line_start = *self.line_starts.get(line_index)?;
        let scalar_index = usize::try_from(column.checked_sub(1)?).ok()?;
        let local = if scalar_index == line_text.chars().count() {
            line_text.len()
        } else {
            line_text.char_indices().nth(scalar_index)?.0
        };
        line_start.checked_add(u32::try_from(local).ok()?)
    }
}

/// Immutable collection of all source snapshots used by one compilation.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SourceMap {
    sources: Arc<[SourceFile]>,
}

impl SourceMap {
    /// Returns an empty source map.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of registered source snapshots.
    #[must_use]
    pub fn len(&self) -> usize {
        self.sources.len()
    }

    /// Returns `true` when no snapshots are registered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.sources.is_empty()
    }

    /// Iterates in deterministic registration order.
    pub fn iter(&self) -> impl ExactSizeIterator<Item = &SourceFile> {
        self.sources.iter()
    }

    /// Resolves one map-local source id.
    #[must_use]
    pub fn get(&self, id: SourceId) -> Option<&SourceFile> {
        self.sources.get(id.0 as usize)
    }

    /// Resolves the first snapshot with this logical/filesystem name.
    #[must_use]
    pub fn find_by_name(&self, name: &Path) -> Option<&SourceFile> {
        self.sources.iter().find(|source| source.name == name)
    }

    /// Validates bounds and UTF-8 boundaries for one canonical range.
    pub fn validate_range(&self, range: SourceRange) -> Result<(), SourceCoordinateError> {
        let source = self
            .get(range.source)
            .ok_or(SourceCoordinateError::UnknownSource(range.source))?;
        let start = range.start as usize;
        let end = range.end as usize;
        if start > end || end > source.text.len() {
            return Err(SourceCoordinateError::InvalidRange(range));
        }
        if !source.text.is_char_boundary(start) || !source.text.is_char_boundary(end) {
            return Err(SourceCoordinateError::NotCharBoundary(range));
        }
        Ok(())
    }

    /// Returns the exact text covered by a canonical range.
    pub fn slice(&self, range: SourceRange) -> Result<&str, SourceCoordinateError> {
        self.validate_range(range)?;
        let source = self
            .get(range.source)
            .ok_or(SourceCoordinateError::UnknownSource(range.source))?;
        Ok(&source.text[range.start as usize..range.end as usize])
    }

    /// Converts a byte offset into a terminal-oriented position.
    ///
    /// Tabs advance to the next four-column stop. Combining characters have
    /// width zero and wide Unicode scalars use their terminal display width.
    pub fn human_position(
        &self,
        source_id: SourceId,
        offset: u32,
    ) -> Result<HumanPosition, SourceCoordinateError> {
        let source = self
            .get(source_id)
            .ok_or(SourceCoordinateError::UnknownSource(source_id))?;
        let point = SourceRange::new(source_id, offset, offset);
        self.validate_range(point)?;
        let line_index = source.line_index_for_offset(offset);
        let line_start = source.line_starts[line_index] as usize;
        let prefix = &source.text[line_start..offset as usize];
        let mut zero_based_column = 0_u32;
        for ch in prefix.chars() {
            if ch == '\t' {
                zero_based_column += HUMAN_TAB_WIDTH - (zero_based_column % HUMAN_TAB_WIDTH);
            } else {
                zero_based_column = zero_based_column
                    .saturating_add(u32::try_from(ch.width().unwrap_or(0)).unwrap_or(0));
            }
        }
        Ok(HumanPosition {
            line: u32::try_from(line_index + 1).unwrap_or(u32::MAX),
            column: zero_based_column.saturating_add(1),
        })
    }

    /// Converts a byte offset into a zero-based LSP UTF-16 position.
    pub fn lsp_position(
        &self,
        source_id: SourceId,
        offset: u32,
    ) -> Result<LspPosition, SourceCoordinateError> {
        let source = self
            .get(source_id)
            .ok_or(SourceCoordinateError::UnknownSource(source_id))?;
        let point = SourceRange::new(source_id, offset, offset);
        self.validate_range(point)?;
        let line_index = source.line_index_for_offset(offset);
        let line_start = source.line_starts[line_index] as usize;
        let prefix = &source.text[line_start..offset as usize];
        Ok(LspPosition {
            line: u32::try_from(line_index).unwrap_or(u32::MAX),
            character: u32::try_from(prefix.encode_utf16().count()).unwrap_or(u32::MAX),
        })
    }

    /// Converts a canonical byte range to the existing file/line/column API.
    ///
    /// Compatibility columns count Unicode scalar values and remain 1-based.
    /// The end column is the half-open scalar boundary used by current caret
    /// rendering, despite the historical inclusive wording of [`SourceSpan`].
    pub fn to_source_span(&self, range: SourceRange) -> Result<SourceSpan, SourceCoordinateError> {
        self.validate_range(range)?;
        let source = self
            .get(range.source)
            .ok_or(SourceCoordinateError::UnknownSource(range.source))?;
        let (line, col) = scalar_position(source, range.start);
        let (end_line, end_col) = scalar_position(source, range.end);
        Ok(SourceSpan::new(
            source.name.clone(),
            line,
            col,
            end_line,
            end_col,
        ))
    }

    /// Converts an existing [`SourceSpan`] into a canonical byte range.
    pub fn from_source_span(
        &self,
        span: &SourceSpan,
    ) -> Result<SourceRange, SourceCoordinateError> {
        let source = self
            .find_by_name(&span.file)
            .ok_or_else(|| SourceCoordinateError::InvalidSpan(span.clone()))?;
        let start = source
            .byte_offset_for_scalar_column(span.line, span.col)
            .ok_or_else(|| SourceCoordinateError::InvalidSpan(span.clone()))?;
        let end = source
            .byte_offset_for_scalar_column(span.end_line, span.end_col)
            .ok_or_else(|| SourceCoordinateError::InvalidSpan(span.clone()))?;
        let range = SourceRange::new(source.id, start, end);
        self.validate_range(range)?;
        Ok(range)
    }
}

/// Mutable construction helper that freezes into an immutable [`SourceMap`].
#[derive(Debug, Default)]
pub struct SourceMapBuilder {
    sources: Vec<SourceFile>,
}

impl SourceMapBuilder {
    /// Creates an empty source-map builder.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers one exact source snapshot and returns its map-local id.
    ///
    /// Re-registering the same `(name, text, kind)` returns the existing id.
    pub fn add(
        &mut self,
        name: impl Into<PathBuf>,
        kind: SourceKind,
        text: impl Into<Arc<str>>,
    ) -> SourceId {
        let name = name.into();
        let text = text.into();
        if let Some(existing) = self
            .sources
            .iter()
            .find(|source| source.name == name && source.kind == kind && source.text == text)
        {
            return existing.id;
        }
        let id = SourceId(u32::try_from(self.sources.len()).expect("source map exceeds u32 ids"));
        let source = SourceFile {
            id,
            name,
            kind,
            content_hash: ContentHash::of(&text),
            line_starts: Arc::from(line_starts(&text)),
            text,
        };
        self.sources.push(source);
        id
    }

    /// Freezes all registered sources in deterministic registration order.
    #[must_use]
    pub fn finish(self) -> SourceMap {
        SourceMap {
            sources: Arc::from(self.sources),
        }
    }
}

fn line_starts(source: &str) -> Vec<u32> {
    let bytes = source.as_bytes();
    let mut starts = vec![0_u32];
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'\r' if bytes.get(index + 1) == Some(&b'\n') => {
                index += 2;
                starts.push(u32::try_from(index).expect("source exceeds u32 byte offsets"));
            }
            b'\r' | b'\n' => {
                index += 1;
                starts.push(u32::try_from(index).expect("source exceeds u32 byte offsets"));
            }
            _ => index += 1,
        }
    }
    starts
}

fn scalar_position(source: &SourceFile, offset: u32) -> (u32, u32) {
    let line_index = source.line_index_for_offset(offset);
    let line_start = source.line_starts[line_index] as usize;
    let prefix = &source.text[line_start..offset as usize];
    (
        u32::try_from(line_index + 1).unwrap_or(u32::MAX),
        u32::try_from(prefix.chars().count() + 1).unwrap_or(u32::MAX),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unicode_map() -> (SourceMap, SourceId) {
        let mut builder = SourceMapBuilder::new();
        let id = builder.add(
            "unicode.dsp",
            SourceKind::Memory,
            "\tprocess = e\u{301} + 音 + 😀;\r\nnext = _;\rlast = _;\n",
        );
        (builder.finish(), id)
    }

    #[test]
    fn registers_every_source_kind_and_deduplicates_exact_snapshots() {
        let mut builder = SourceMapBuilder::new();
        let file = builder.add("main.dsp", SourceKind::File, "process = _;");
        assert_eq!(
            builder.add("main.dsp", SourceKind::File, "process = _;"),
            file
        );
        builder.add("<memory>", SourceKind::Memory, "process = 0;");
        builder.add("lib.dsp", SourceKind::ImportedFile, "foo = _;");
        builder.add(
            "stdfaust.lib",
            SourceKind::VirtualLibrary,
            "library = \"x\";",
        );
        let map = builder.finish();
        assert_eq!(map.len(), 4);
        assert_eq!(map.get(file).map(SourceFile::kind), Some(SourceKind::File));
    }

    #[test]
    fn canonical_ranges_are_half_open_utf8_boundaries() {
        let (map, id) = unicode_map();
        let source = map.get(id).unwrap().text();
        let emoji = u32::try_from(source.find('😀').unwrap()).unwrap();
        let range = SourceRange::new(id, emoji, emoji + 4);
        assert_eq!(map.slice(range).unwrap(), "😀");
        assert_eq!(range.len(), 4);
        assert!(matches!(
            map.validate_range(SourceRange::new(id, emoji + 1, emoji + 4)),
            Err(SourceCoordinateError::NotCharBoundary(_))
        ));
    }

    #[test]
    fn unicode_tabs_crlf_and_lsp_positions_are_distinct_and_correct() {
        let (map, id) = unicode_map();
        let source = map.get(id).unwrap().text();
        let emoji = u32::try_from(source.find('😀').unwrap()).unwrap();
        assert_eq!(
            map.human_position(id, emoji).unwrap(),
            HumanPosition {
                line: 1,
                column: 24
            }
        );
        assert_eq!(
            map.lsp_position(id, emoji).unwrap(),
            LspPosition {
                line: 0,
                character: 20
            }
        );

        let next = u32::try_from(source.find("next").unwrap()).unwrap();
        let last = u32::try_from(source.find("last").unwrap()).unwrap();
        assert_eq!(map.human_position(id, next).unwrap().line, 2);
        assert_eq!(map.human_position(id, last).unwrap().line, 3);
        assert_eq!(
            map.get(id).unwrap().line_text(1),
            Some("\tprocess = e\u{301} + 音 + 😀;")
        );
    }

    #[test]
    fn multiline_range_round_trips_through_legacy_span() {
        let (map, id) = unicode_map();
        let source = map.get(id).unwrap().text();
        let start = u32::try_from(source.find("process").unwrap()).unwrap();
        let end = u32::try_from(source.find("next").unwrap() + "next".len()).unwrap();
        let range = SourceRange::new(id, start, end);
        let span = map.to_source_span(range).unwrap();
        assert_eq!((span.line, span.col), (1, 2));
        assert_eq!((span.end_line, span.end_col), (2, 5));
        assert_eq!(map.from_source_span(&span).unwrap(), range);
    }

    #[test]
    fn content_hash_binds_the_exact_snapshot() {
        let mut builder = SourceMapBuilder::new();
        let first = builder.add("a.dsp", SourceKind::Memory, "process = _;\n");
        let second = builder.add("a.dsp", SourceKind::Memory, "process = 0;\n");
        let map = builder.finish();
        assert_ne!(
            map.get(first).unwrap().content_hash(),
            map.get(second).unwrap().content_hash()
        );
        assert_eq!(map.get(first).unwrap().content_hash().to_hex().len(), 64);
    }
}
