//! Parser context for the `lrpar/lrlex` migration prototype.
//!
//! # Source provenance (C++)
//! - `compiler/parser/faustparser.y`:
//!   - parser cursor (`FAUSTfilename`, `FAUSTlineno`)
//!   - waveform accumulator (`gGlobal->gWaveForm`)
//!   - parse root storage (`gGlobal->gResult`)
//! - `compiler/errors/errormsg.cpp`:
//!   - definition/use properties (`setDefProp`, `setUseProp`)
//!
//! # Parity invariants
//! - Definition/use properties are attached to `TreeId` symbols with source file + line payload.
//! - Waveform values are accumulated in parse order then drained by the corresponding action.
//! - Parser diagnostics are explicitly scoped to one parser context (no global mutable singleton).

use std::collections::HashMap;

use diagnostics::{DiagnosticCode, Severity, codes};
use tlib::{PropertyKey, PropertyStore, TreeId};

/// Parser source location equivalent to `(filename, lineno)` in C++ parser globals,
/// extended with optional column/range precision from `lrpar` spans.
#[derive(Clone, Debug, PartialEq, Eq)]
/// Parser source location tracked during lexing and grammar actions.
pub struct SourceLocation {
    file: Box<str>,
    line: u32,
    col: u32,
    end_line: u32,
    end_col: u32,
}

/// Stable parser-session identifier for one Box source occurrence.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BoxOriginId(u32);

impl BoxOriginId {
    /// Returns the deterministic zero-based session id.
    #[must_use]
    pub const fn as_u32(self) -> u32 {
        self.0
    }
}

/// Parser action that created one Box occurrence.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BoxOriginRole {
    /// Definition-side occurrence.
    Definition,
    /// Use-side occurrence.
    Use,
}

/// One source occurrence of a hash-consed Box node.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BoxOrigin {
    /// Stable parser-session occurrence id.
    pub id: BoxOriginId,
    /// Semantic Box identity shared by structurally equal occurrences.
    pub node: TreeId,
    /// Exact source location observed by the grammar action.
    pub location: SourceLocation,
    /// Definition/use role at the parser boundary.
    pub role: BoxOriginRole,
}

/// Exact Box occurrence selected at an ambiguity-sensitive boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct LocatedBox {
    /// Hash-consed semantic Box node.
    pub node: TreeId,
    /// Selected source occurrence.
    pub origin: BoxOriginId,
}

/// Occurrence-aware provenance retained alongside parser Box nodes.
///
/// A semantic node maps to every candidate origin. [`LocatedBox`] selects one
/// candidate without changing `TreeArena` hash-consing or semantic equality.
#[derive(Clone, Debug, Default)]
pub struct BoxProvenance {
    origins: Vec<BoxOrigin>,
    by_node: HashMap<TreeId, Vec<BoxOriginId>>,
}

impl BoxProvenance {
    fn record(
        &mut self,
        node: TreeId,
        location: SourceLocation,
        role: BoxOriginRole,
    ) -> LocatedBox {
        let raw_id = u32::try_from(self.origins.len()).unwrap_or(u32::MAX);
        let id = BoxOriginId(raw_id);
        self.origins.push(BoxOrigin {
            id,
            node,
            location,
            role,
        });
        self.by_node.entry(node).or_default().push(id);
        LocatedBox { node, origin: id }
    }

    /// Returns one recorded occurrence by id.
    #[must_use]
    pub fn get(&self, id: BoxOriginId) -> Option<&BoxOrigin> {
        self.origins.get(usize::try_from(id.0).ok()?)
    }

    /// Returns every candidate origin for one semantic node in parse order.
    #[must_use]
    pub fn origins_for(&self, node: TreeId) -> &[BoxOriginId] {
        self.by_node.get(&node).map_or(&[], Vec::as_slice)
    }

    /// Returns all occurrences in deterministic parse order.
    #[must_use]
    pub fn as_slice(&self) -> &[BoxOrigin] {
        &self.origins
    }

    /// Resolves an exact located occurrence and checks its semantic identity.
    #[must_use]
    pub fn resolve(&self, located: LocatedBox) -> Option<&BoxOrigin> {
        self.get(located.origin)
            .filter(|origin| origin.node == located.node)
    }
}

impl SourceLocation {
    /// Creates a source location.
    #[must_use]
    pub fn new(file: &str, line: u32) -> Self {
        Self::new_span(file, line, 1, line, 1)
    }

    /// Creates a source location with explicit column.
    #[must_use]
    pub fn new_with_col(file: &str, line: u32, col: u32) -> Self {
        Self::new_span(file, line, col, line, col)
    }

    /// Creates a source location with explicit start/end range.
    #[must_use]
    pub fn new_span(file: &str, line: u32, col: u32, end_line: u32, end_col: u32) -> Self {
        Self {
            file: file.into(),
            line,
            col,
            end_line,
            end_col,
        }
    }

    /// Source file path/name.
    #[must_use]
    pub fn file(&self) -> &str {
        &self.file
    }

    /// 1-based line number.
    #[must_use]
    pub fn line(&self) -> u32 {
        self.line
    }

    /// 1-based start column number.
    #[must_use]
    pub fn col(&self) -> u32 {
        self.col
    }

    /// 1-based end line number.
    #[must_use]
    pub fn end_line(&self) -> u32 {
        self.end_line
    }

    /// 1-based end column number.
    #[must_use]
    pub fn end_col(&self) -> u32 {
        self.end_col
    }
}

/// One parser diagnostic with optional source location.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ParserDiagnostic {
    /// Severity of the diagnostic.
    pub severity: Severity,
    /// Stable diagnostic code assigned at the emission site.
    pub code: DiagnosticCode,
    /// Human-readable diagnostic message.
    pub message: Box<str>,
    /// Source location, when available.
    pub location: Option<SourceLocation>,
}

/// Parser-local mutable context replacing the parser-relevant subset of `gGlobal`.
///
/// Intentionally per-parse; owns all mutable parser state that used to be
/// spread across C++ globals: cursor, diagnostics, waveform accumulation,
/// definition/use properties, metadata declarations, and documentation counters.
#[derive(Debug)]
pub struct ParserCtx {
    cursor: SourceLocation,
    diagnostics: Vec<ParserDiagnostic>,
    parse_error_count: u32,
    recovery_count: u32,
    waveform: Vec<TreeId>,
    parse_result: Option<TreeId>,
    imports: Vec<Box<str>>,
    declared_metadata: Vec<(Box<str>, Box<str>)>,
    declared_definition_metadata: Vec<(Box<str>, Box<str>, Box<str>)>,
    doc_block_count: u32,
    doc_notice_count: u32,
    doc_listing_count: u32,
    doc_char_count: u32,
    doc_metadata_tags: Vec<Box<str>>,
    lst_dependencies: Option<bool>,
    lst_mdoctags: Option<bool>,
    lst_distributed: Option<bool>,
    float_size: u8,
    props: PropertyStore<SourceLocation>,
    def_prop_key: PropertyKey,
    use_prop_key: PropertyKey,
    box_provenance: BoxProvenance,
    definition_candidate_floor: HashMap<TreeId, usize>,
}

impl Default for ParserCtx {
    fn default() -> Self {
        Self::new()
    }
}

impl ParserCtx {
    /// Creates a fresh parser context.
    ///
    /// Cursor defaults follow C++ parser defaults (`FAUSTfilename="????"`).
    #[must_use]
    pub fn new() -> Self {
        let mut props = PropertyStore::new();
        let def_prop_key = props.key("DEFLINEPROP");
        let use_prop_key = props.key("USELINEPROP");
        Self {
            cursor: SourceLocation::new("????", 1),
            diagnostics: Vec::new(),
            parse_error_count: 0,
            recovery_count: 0,
            waveform: Vec::new(),
            parse_result: None,
            imports: Vec::new(),
            declared_metadata: Vec::new(),
            declared_definition_metadata: Vec::new(),
            doc_block_count: 0,
            doc_notice_count: 0,
            doc_listing_count: 0,
            doc_char_count: 0,
            doc_metadata_tags: Vec::new(),
            lst_dependencies: None,
            lst_mdoctags: None,
            lst_distributed: None,
            float_size: 1,
            props,
            def_prop_key,
            use_prop_key,
            box_provenance: BoxProvenance::default(),
            definition_candidate_floor: HashMap::new(),
        }
    }

    /// Sets parser cursor location (equivalent to lexer-maintained file/line globals).
    pub fn set_cursor(&mut self, file: &str, line: u32) {
        self.cursor = SourceLocation::new(file, line);
    }

    /// Sets parser cursor location with explicit column.
    pub fn set_cursor_with_col(&mut self, file: &str, line: u32, col: u32) {
        self.cursor = SourceLocation::new_with_col(file, line, col);
    }

    /// Sets parser cursor location with explicit start/end range.
    pub fn set_cursor_span(
        &mut self,
        file: &str,
        line: u32,
        col: u32,
        end_line: u32,
        end_col: u32,
    ) {
        self.cursor = SourceLocation::new_span(file, line, col, end_line, end_col);
    }

    /// Returns current parser cursor.
    #[must_use]
    pub fn cursor(&self) -> &SourceLocation {
        &self.cursor
    }

    /// Appends one waveform value in parse order.
    pub fn push_waveform_value(&mut self, value: TreeId) {
        self.waveform.push(value);
    }

    /// Returns current waveform buffer.
    #[must_use]
    pub fn waveform(&self) -> &[TreeId] {
        &self.waveform
    }

    /// Drains waveform buffer in FIFO parse order.
    pub fn take_waveform(&mut self) -> Vec<TreeId> {
        std::mem::take(&mut self.waveform)
    }

    /// Sets parse root result.
    pub fn set_parse_result(&mut self, root: TreeId) {
        self.parse_result = Some(root);
    }

    /// Returns parse root result if set.
    #[must_use]
    pub fn parse_result(&self) -> Option<TreeId> {
        self.parse_result
    }

    /// Clears parse root result.
    pub fn clear_parse_result(&mut self) {
        self.parse_result = None;
    }

    /// Records one `import("...")` statement payload.
    pub fn note_import(&mut self, path: &str) {
        self.imports.push(path.into());
    }

    /// Recorded import paths in parse order.
    #[must_use]
    pub fn imports(&self) -> &[Box<str>] {
        &self.imports
    }

    /// Records `declare key value;`.
    ///
    /// These entries preserve parse order so later metadata aggregation can
    /// replay the same override order as the C++ parser session.
    pub fn note_declared_metadata(&mut self, key: &str, value: &str) {
        self.declared_metadata.push((key.into(), value.into()));
    }

    /// Records `declare def key value;`.
    ///
    /// The definition name is kept as parsed text here; later stages resolve it
    /// against grouped definitions once the full file/import set is available.
    pub fn note_declared_definition_metadata(&mut self, def: &str, key: &str, value: &str) {
        self.declared_definition_metadata
            .push((def.into(), key.into(), value.into()));
    }

    /// Recorded `declare key value;` entries.
    #[must_use]
    pub fn declared_metadata(&self) -> &[(Box<str>, Box<str>)] {
        &self.declared_metadata
    }

    /// Recorded `declare def key value;` entries.
    #[must_use]
    pub fn declared_definition_metadata(&self) -> &[(Box<str>, Box<str>, Box<str>)] {
        &self.declared_definition_metadata
    }

    /// Records one parsed doc block.
    pub fn note_doc_block(&mut self) {
        self.doc_block_count = self.doc_block_count.saturating_add(1);
    }

    /// Number of parsed doc blocks.
    #[must_use]
    pub fn doc_block_count(&self) -> u32 {
        self.doc_block_count
    }

    /// Records one parsed doc notice.
    pub fn note_doc_notice(&mut self) {
        self.doc_notice_count = self.doc_notice_count.saturating_add(1);
    }

    /// Number of parsed doc notices.
    #[must_use]
    pub fn doc_notice_count(&self) -> u32 {
        self.doc_notice_count
    }

    /// Records one parsed listing block.
    pub fn note_doc_listing(&mut self) {
        self.doc_listing_count = self.doc_listing_count.saturating_add(1);
    }

    /// Number of parsed listing blocks.
    #[must_use]
    pub fn doc_listing_count(&self) -> u32 {
        self.doc_listing_count
    }

    /// Records one doc character token consumed by the parser.
    pub fn note_doc_char(&mut self) {
        self.doc_char_count = self.doc_char_count.saturating_add(1);
    }

    /// Number of `DOCCHAR` tokens consumed by the parser.
    #[must_use]
    pub fn doc_char_count(&self) -> u32 {
        self.doc_char_count
    }

    /// Records one metadata tag name found in `<metadata>...</metadata>`.
    pub fn note_doc_metadata_tag(&mut self, tag: &str) {
        self.doc_metadata_tags.push(tag.into());
    }

    /// Metadata tag names parsed in documentation sections.
    #[must_use]
    pub fn doc_metadata_tags(&self) -> &[Box<str>] {
        &self.doc_metadata_tags
    }

    /// Equivalent to C++ listing switch update for dependencies.
    pub fn set_lst_dependencies(&mut self, value: bool) {
        self.lst_dependencies = Some(value);
    }

    /// Equivalent to C++ listing switch update for mdoctags.
    pub fn set_lst_mdoctags(&mut self, value: bool) {
        self.lst_mdoctags = Some(value);
    }

    /// Equivalent to C++ listing switch update for distributed.
    pub fn set_lst_distributed(&mut self, value: bool) {
        self.lst_distributed = Some(value);
    }

    /// Last seen dependencies listing switch value.
    #[must_use]
    pub fn lst_dependencies(&self) -> Option<bool> {
        self.lst_dependencies
    }

    /// Last seen mdoctags listing switch value.
    #[must_use]
    pub fn lst_mdoctags(&self) -> Option<bool> {
        self.lst_mdoctags
    }

    /// Last seen distributed listing switch value.
    #[must_use]
    pub fn lst_distributed(&self) -> Option<bool> {
        self.lst_distributed
    }

    /// Sets parser float precision mode equivalent to C++ `gFloatSize`:
    /// `1=single`, `2=double`, `3=quad`, `4=fixed`.
    pub fn set_float_size(&mut self, float_size: u8) {
        self.float_size = float_size.clamp(1, 4);
    }

    /// Returns parser float precision mode equivalent to C++ `gFloatSize`.
    #[must_use]
    pub fn float_size(&self) -> u8 {
        self.float_size
    }

    /// Equivalent to C++ `acceptdefinition(prefixset)`.
    ///
    /// A definition is accepted if `prefixset` is empty or if the current parser
    /// precision belongs to the variant prefix set.
    #[must_use]
    pub fn accept_definition(&self, prefixset: u8) -> bool {
        if prefixset == 0 {
            return true;
        }
        let precision_mask = match self.float_size {
            1 => 1,
            2 => 2,
            3 => 4,
            4 => 8,
            _ => 1,
        };
        (prefixset & precision_mask) != 0
    }

    /// Equivalent to C++ `setDefProp(sym, file, line)`.
    ///
    /// Only one definition location is stored per symbol key; later writes
    /// intentionally replace earlier ones, matching the property-store behavior
    /// used by the historical parser utilities.
    pub fn set_def_prop(&mut self, sym: TreeId, file: &str, line: u32) {
        self.set_def_prop_location(sym, SourceLocation::new(file, line));
    }

    /// Sets definition property with full source span precision.
    ///
    /// Rust extends the C++ file/line payload with range information so later
    /// diagnostics can preserve `lrpar` span precision when available.
    pub fn set_def_prop_location(&mut self, sym: TreeId, location: SourceLocation) {
        self.box_provenance
            .record(sym, location.clone(), BoxOriginRole::Definition);
        let _ = self.props.set_with_key(sym, self.def_prop_key, location);
    }

    /// Equivalent to C++ `setUseProp(sym, file, line)`.
    pub fn set_use_prop(&mut self, sym: TreeId, file: &str, line: u32) {
        self.set_use_prop_location(sym, SourceLocation::new(file, line));
    }

    /// Sets usage property with full source span precision.
    pub fn set_use_prop_location(&mut self, sym: TreeId, location: SourceLocation) {
        self.box_provenance
            .record(sym, location.clone(), BoxOriginRole::Use);
        let _ = self.props.set_with_key(sym, self.use_prop_key, location);
    }

    /// Returns occurrence-aware Box provenance for this parse session.
    #[must_use]
    pub fn box_provenance(&self) -> &BoxProvenance {
        &self.box_provenance
    }

    /// Imports occurrences whose semantic nodes were cloned into this parse
    /// arena.
    pub(crate) fn import_box_provenance(
        &mut self,
        source: &ParserCtx,
        node_map: &HashMap<TreeId, TreeId>,
    ) {
        for origin in source.box_provenance.as_slice() {
            if let Some(&node) = node_map.get(&origin.node) {
                self.box_provenance
                    .record(node, origin.location.clone(), origin.role);
            }
        }
    }

    /// Convenience hook: set definition property from current parser cursor.
    pub fn set_def_prop_at_cursor(&mut self, sym: TreeId) {
        let candidates = self.box_provenance.origins_for(sym);
        let floor = self
            .definition_candidate_floor
            .get(&sym)
            .copied()
            .unwrap_or(0);
        let loc = candidates
            .get(floor..)
            .and_then(|ids| {
                ids.iter()
                    .filter_map(|id| self.box_provenance.get(*id))
                    .find(|origin| origin.role == BoxOriginRole::Use)
            })
            .map_or_else(|| self.cursor.clone(), |origin| origin.location.clone());
        self.definition_candidate_floor
            .insert(sym, candidates.len());
        self.set_def_prop_location(sym, loc);
    }

    /// Convenience hook: set usage property from current parser cursor.
    pub fn set_use_prop_at_cursor(&mut self, sym: TreeId) {
        let loc = self.cursor.clone();
        self.set_use_prop_location(sym, loc);
    }

    /// Equivalent to C++ `hasDefProp(sym)`.
    #[must_use]
    pub fn has_def_prop(&self, sym: TreeId) -> bool {
        self.props.get_with_key(sym, self.def_prop_key).is_some()
    }

    /// Returns definition property when present.
    #[must_use]
    pub fn def_prop(&self, sym: TreeId) -> Option<&SourceLocation> {
        self.props.get_with_key(sym, self.def_prop_key)
    }

    /// Returns usage property when present.
    #[must_use]
    pub fn use_prop(&self, sym: TreeId) -> Option<&SourceLocation> {
        self.props.get_with_key(sym, self.use_prop_key)
    }

    /// Equivalent to C++ `getDefFileProp(sym)`.
    #[must_use]
    pub fn def_file_prop(&self, sym: TreeId) -> Option<&str> {
        self.def_prop(sym).map(SourceLocation::file)
    }

    /// Equivalent to C++ `getDefLineProp(sym)`.
    #[must_use]
    pub fn def_line_prop(&self, sym: TreeId) -> Option<u32> {
        self.def_prop(sym).map(SourceLocation::line)
    }

    /// Equivalent to C++ `getUseFileProp(sym)`.
    #[must_use]
    pub fn use_file_prop(&self, sym: TreeId) -> Option<&str> {
        self.use_prop(sym).map(SourceLocation::file)
    }

    /// Equivalent to C++ `getUseLineProp(sym)`.
    #[must_use]
    pub fn use_line_prop(&self, sym: TreeId) -> Option<u32> {
        self.use_prop(sym).map(SourceLocation::line)
    }

    /// Records a parser error at current cursor location.
    pub fn error(&mut self, message: &str) {
        self.parse_error_count = self.parse_error_count.saturating_add(1);
        self.push_diagnostic(
            Severity::Error,
            codes::PARSE_UNEXPECTED_TOKEN,
            message,
            Some(self.cursor.clone()),
        );
    }

    /// Records a parser error at current cursor location with explicit stable diagnostic code.
    pub fn error_with_code(&mut self, code: DiagnosticCode, message: &str) {
        self.parse_error_count = self.parse_error_count.saturating_add(1);
        self.push_diagnostic(Severity::Error, code, message, Some(self.cursor.clone()));
    }

    /// Records a parser warning at current cursor location.
    pub fn warning(&mut self, message: &str) {
        self.push_diagnostic(
            Severity::Warning,
            codes::PARSE_RECOVERY,
            message,
            Some(self.cursor.clone()),
        );
    }

    /// Records a parser remark at current cursor location.
    pub fn remark(&mut self, message: &str) {
        self.push_diagnostic(
            Severity::Remark,
            codes::PARSE_RECOVERY,
            message,
            Some(self.cursor.clone()),
        );
    }

    /// Records one parser recovery event (e.g. `error ENDDEF` path).
    pub fn note_recovery(&mut self) {
        self.recovery_count = self.recovery_count.saturating_add(1);
    }

    /// Number of parser errors recorded in this context.
    #[must_use]
    pub fn parse_error_count(&self) -> u32 {
        self.parse_error_count
    }

    /// Number of parser recovery events.
    #[must_use]
    pub fn recovery_count(&self) -> u32 {
        self.recovery_count
    }

    /// All recorded diagnostics.
    #[must_use]
    pub(crate) fn diagnostics(&self) -> &[ParserDiagnostic] {
        &self.diagnostics
    }

    /// Returns `true` when no diagnostics are currently recorded.
    #[must_use]
    pub fn diagnostics_is_empty(&self) -> bool {
        self.diagnostics.is_empty()
    }

    fn push_diagnostic(
        &mut self,
        severity: Severity,
        code: DiagnosticCode,
        message: &str,
        location: Option<SourceLocation>,
    ) {
        self.diagnostics.push(ParserDiagnostic {
            severity,
            code,
            message: message.into(),
            location,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::{BoxOriginRole, ParserCtx, SourceLocation};
    use diagnostics::{Severity, codes};
    use tlib::TreeArena;

    #[test]
    fn diagnostic_codes_are_assigned_independently_of_message_wording() {
        let mut ctx = ParserCtx::new();
        ctx.error("invalid literal wording does not classify this error");
        ctx.error_with_code(
            codes::PARSE_INVALID_LITERAL,
            "numeric token cannot be represented",
        );
        ctx.warning("recovered after one token");
        ctx.remark("continuing parse");

        let diagnostics = ctx.diagnostics();
        assert_eq!(diagnostics[0].code, codes::PARSE_UNEXPECTED_TOKEN);
        assert_eq!(diagnostics[1].code, codes::PARSE_INVALID_LITERAL);
        assert_eq!(diagnostics[2].code, codes::PARSE_RECOVERY);
        assert_eq!(diagnostics[3].code, codes::PARSE_RECOVERY);
        assert_eq!(diagnostics[0].severity, Severity::Error);
        assert_eq!(diagnostics[2].severity, Severity::Warning);
        assert_eq!(diagnostics[3].severity, Severity::Remark);
        assert_eq!(ctx.parse_error_count(), 2);
    }

    #[test]
    fn hash_consed_box_keeps_all_origins_and_located_selection() {
        let mut arena = TreeArena::new();
        let shared = arena.symbol("same");
        assert_eq!(shared, arena.symbol("same"));

        let mut ctx = ParserCtx::new();
        ctx.set_use_prop_location(shared, SourceLocation::new_span("a.dsp", 1, 3, 1, 7));
        ctx.set_use_prop_location(shared, SourceLocation::new_span("b.dsp", 9, 5, 9, 9));

        let candidates = ctx.box_provenance().origins_for(shared);
        assert_eq!(candidates.len(), 2);
        let first = ctx
            .box_provenance()
            .resolve(super::LocatedBox {
                node: shared,
                origin: candidates[0],
            })
            .expect("first exact occurrence should resolve");
        let second = ctx
            .box_provenance()
            .resolve(super::LocatedBox {
                node: shared,
                origin: candidates[1],
            })
            .expect("second exact occurrence should resolve");
        assert_eq!(first.location.file(), "a.dsp");
        assert_eq!(second.location.file(), "b.dsp");
        assert_eq!(first.role, BoxOriginRole::Use);
        assert_eq!(second.role, BoxOriginRole::Use);

        assert_eq!(
            ctx.use_prop(shared).map(SourceLocation::file),
            Some("b.dsp"),
            "the compatibility property remains last-write-wins"
        );
    }
}
