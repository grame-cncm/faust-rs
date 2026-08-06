//! Production Faust parser crate (`lrpar`/`lrlex`).
//!
//! # Source provenance (C++)
//! - `compiler/parser/faustparser.y`
//! - `compiler/parser/faustlexer.l`
//! - `compiler/errors/errormsg.hh` / `compiler/errors/errormsg.cpp` (`setDefProp`/`setUseProp`)
//! - `compiler/global.hh` (`gWaveForm`, `gResult`)
//!
//! # Current scope
//! - Provides `ParserCtx` for parser-local state and property hooks.
//! - Parser/lexer migration is active through slices 1..12 with semantic actions.
//! - Routes expression constructors through `boxes` over `tlib::TreeArena` (no parser-local stubs).
//!
//! # Integer literal convention
//! - Parser integer tokens are lowered to `boxes` integer nodes with `i32`
//!   semantic width.
//! - Token parsing still uses `i64` as an intermediate and clamps to `i32`
//!   bounds at the parser boundary for deterministic behavior.

use boxes::{BoxMatch, dump_box, match_box};
use cfgrammar::Span;
use diagnostics::codes;
use diagnostics::{
    Applicability, Diagnostic, DiagnosticBundle, DiagnosticCode, Label, LabelRole, LabelStyle,
    RelatedDiagnostic, SourceId, SourceKind, SourceMapBuilder, SourceRange, SourceSpan, Stage,
    SuggestedFix, TextEdit,
};
use lrlex::lrlex_mod;
use lrlex::{DefaultLexerTypes, LRNonStreamingLexerDef};
use lrpar::lrpar_mod;
use lrpar::{LexError, Lexeme, Lexer, NonStreamingLexer};
use std::cell::RefCell;
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use tlib::{NodeKind, TreeArena, TreeId};

pub mod context;
pub mod metadata;
pub mod source_reader;

pub use context::{
    BoxOrigin, BoxOriginId, BoxOriginRole, BoxProvenance, LocatedBox, ParserCtx, SourceLocation,
    WidgetDeclaration,
};
pub use metadata::{CompilationMetadataKey, CompilationMetadataSnapshot, CompilationMetadataStore};
pub use source_reader::{
    ExpandedSource, ImportCycleEdge, ImportSite, SourceLineOrigin, SourceReader, SourceReaderError,
    VirtualSourceMap,
};

/// Primitive operator family recognized directly by the parser.
#[derive(Clone, Copy, Debug)]
pub enum PrimitiveOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    And,
    Or,
    Xor,
    Lsh,
    Rsh,
    Lt,
    Le,
    Gt,
    Ge,
    Eq,
    Ne,
    Pow,
    Delay,
    Delay1,
}

/// Mutable parser state threaded through grammar actions via `%parse-param`.
#[derive(Debug)]
pub struct ParseState {
    pub arena: TreeArena,
    pub ctx: ParserCtx,
    source_file: Box<str>,
    source_origins: Option<Vec<SourceLineOrigin>>,
    source_line_starts: Vec<usize>,
    metadata_store: CompilationMetadataStore,
}

impl ParseState {
    /// Creates parser state bound to one source file name/path.
    #[must_use]
    pub fn new(source_file: &str) -> Self {
        Self::new_with_origins_and_metadata(
            source_file,
            "",
            None,
            CompilationMetadataStore::new(source_file),
        )
    }

    /// Creates parser state bound to one source file and optional expanded-source origin map.
    #[must_use]
    pub fn new_with_origins(
        source_file: &str,
        input: &str,
        source_origins: Option<Vec<SourceLineOrigin>>,
    ) -> Self {
        Self::new_with_origins_and_metadata(
            source_file,
            input,
            source_origins,
            CompilationMetadataStore::new(source_file),
        )
    }

    /// Creates parser state bound to one source file, optional origin map, and
    /// one shared compilation-global metadata store.
    #[must_use]
    pub fn new_with_origins_and_metadata(
        source_file: &str,
        input: &str,
        source_origins: Option<Vec<SourceLineOrigin>>,
        metadata_store: CompilationMetadataStore,
    ) -> Self {
        Self {
            arena: TreeArena::new(),
            ctx: ParserCtx::new(),
            source_file: source_file.into(),
            source_origins,
            source_line_starts: compute_line_starts(input),
            metadata_store,
        }
    }

    #[must_use]
    fn node_builder(&mut self) -> boxes::BoxBuilder<'_> {
        boxes::BoxBuilder::new(&mut self.arena)
    }

    /// Equivalent to parser-level `nil` list root in C++ actions.
    #[must_use]
    pub fn nil(&mut self) -> TreeId {
        self.arena.nil()
    }

    /// Equivalent to C++ `cons(head, tail)` in parser actions.
    #[must_use]
    pub fn cons(&mut self, head: TreeId, tail: TreeId) -> TreeId {
        self.arena.cons(head, tail)
    }

    /// Formats raw parser definitions into normalized Faust definition bodies.
    ///
    /// Source provenance (C++):
    /// - `compiler/parser/sourcereader.cpp`
    /// - `standardArgList`
    /// - `makeDefinition`
    /// - `formatDefinitions`
    /// - `addFunctionMetadata`
    ///
    /// Raw parser definitions are stored as `cons(name, cons(args, body))`, where:
    /// - `args == nil` means plain `name = body;`
    /// - non-`nil` `args` retains the parser arglist (reversed list convention)
    ///
    /// This pass groups same-name definitions and lowers them as C++ does:
    /// - one no-arg clause -> body
    /// - one standard identifier arglist -> nested `abstr`
    /// - one non-standard arglist -> `case` with one rule
    /// - multiple no-arg clauses -> parser error, because ordinary
    ///   definitions cannot be repeated
    /// - multiple clauses with parameters -> `case` pattern-matching
    ///   definition (all clauses must have the same arity and arity > 0)
    ///
    /// Import-file nodes are preserved structurally instead of being grouped or
    /// erased. This matches the C++ `formatDefinitions(...)` contract where
    /// `isImportFile(...)` entries survive normalization and are only expanded
    /// later by `SourceReader::expandList(...)`.
    ///
    /// The grouping key is the textual definition name, so repeated parser
    /// clauses for the same function are intentionally merged even if they were
    /// not adjacent in the raw parser list. This mirrors the C++ post-parse
    /// normalization stage rather than preserving raw syntactic order one node
    /// at a time.
    #[must_use]
    pub fn format_definitions(&mut self, defs: TreeId) -> TreeId {
        let mut grouped: BTreeMap<String, (TreeId, Vec<TreeId>)> = BTreeMap::new();
        let mut imports = Vec::new();
        let mut cursor = defs;

        while !self.arena.is_nil(cursor) {
            let Some(def) = self.arena.hd(cursor) else {
                self.ctx.error("invalid definition list cell");
                return self.nil();
            };
            if !self.arena.is_nil(def) {
                if matches!(match_box(&self.arena, def), boxes::BoxMatch::ImportFile(_)) {
                    imports.push(def);
                    cursor = self.arena.tl(cursor).unwrap_or_else(|| self.nil());
                    continue;
                }
                let Some((name, payload)) = self.definition_name_and_payload(def) else {
                    self.ctx.error("invalid definition node shape");
                    return self.nil();
                };
                let Some(key) = self.definition_name_key(name) else {
                    self.ctx.error("invalid definition name");
                    return self.nil();
                };
                grouped
                    .entry(key)
                    .and_modify(|(_, variants)| variants.push(payload))
                    .or_insert_with(|| (name, vec![payload]));
            }
            cursor = self.arena.tl(cursor).unwrap_or_else(|| self.nil());
        }

        let mut out = self.nil();
        for import in &imports {
            out = self.cons(*import, out);
        }
        for (_key, (name, variants_rev)) in grouped {
            // C++ `makeDefinition(symbol, variants)` rejects repeated
            // zero-arity variants before evaluation:
            //
            //   foo = 1;
            //   foo = 2;
            //
            // is a multiple-definition error, not a pattern-matching
            // definition. Only clauses with at least one parameter can be
            // grouped into `boxCase`. Keep this check here instead of relying
            // on `eval::bind_definitions`: `--dump-box` must fail too, exactly
            // like C++ Faust fails while formatting parser definitions.
            if variants_rev.len() > 1 && self.group_has_zero_arity_variants(&variants_rev) {
                self.report_zero_arity_redefinition(name, &variants_rev);
                return self.nil();
            } else {
                let formatted = self.make_definition_from_variants(name, &variants_rev);
                if self.arena.is_nil(formatted) {
                    continue;
                }
                out = self.cons(formatted, out);
            }
        }
        out
    }

    /// Prepends non-`nil` statement in parser list order.
    #[must_use]
    pub fn prepend_statement(&mut self, list: TreeId, stmt: TreeId) -> TreeId {
        if self.arena.is_nil(stmt) {
            list
        } else {
            self.arena.cons(stmt, list)
        }
    }

    /// Prepends statement only if C++ `variantlist` accepts current precision mode.
    #[must_use]
    pub fn prepend_statement_with_variant(
        &mut self,
        list: TreeId,
        variants: u8,
        stmt: TreeId,
    ) -> TreeId {
        if !self.ctx.accept_definition(variants) {
            return list;
        }
        self.prepend_statement(list, stmt)
    }

    /// Builds one definition node shape compatible with C++ parser (`cons(name, cons(args, expr))`).
    ///
    /// This raw shape is the parser-side interchange format consumed later by
    /// [`format_definitions`](Self::format_definitions). It is intentionally not
    /// the final semantic definition representation used by `eval`.
    #[must_use]
    pub fn make_definition(&mut self, name: TreeId, args: TreeId, expr: TreeId) -> TreeId {
        let pair = self.arena.cons(args, expr);
        self.arena.cons(name, pair)
    }

    /// Marks one recovered statement and returns `nil` placeholder.
    #[must_use]
    pub fn recovery_statement(&mut self, message: &str) -> TreeId {
        self.ctx.note_recovery();
        self.ctx.error(message);
        self.arena.nil()
    }

    /// Sets definition property at current cursor position.
    pub fn mark_def_at_cursor(&mut self, sym: TreeId) {
        self.ctx.set_def_prop_at_cursor(sym);
    }

    /// Builds `boxIdent` from a token and optionally marks use property.
    #[must_use]
    pub fn ident_from_token<'lexer, 'input: 'lexer>(
        &mut self,
        lexer: &'lexer dyn NonStreamingLexer<'input, DefaultLexerTypes<u32>>,
        tok: Result<lrlex::DefaultLexeme<u32>, lrlex::DefaultLexeme<u32>>,
        mark_use: bool,
    ) -> TreeId {
        let span = token_span(&tok);
        self.update_cursor_from_span(lexer, span);
        let ident = self.node_builder().ident(lexer.span_str(span));
        if mark_use {
            self.ctx.set_use_prop_at_cursor(ident);
        }
        ident
    }

    /// Builds one symbol tree from a token and optionally marks use property.
    #[must_use]
    pub fn symbol_from_token<'lexer, 'input: 'lexer>(
        &mut self,
        lexer: &'lexer dyn NonStreamingLexer<'input, DefaultLexerTypes<u32>>,
        tok: Result<lrlex::DefaultLexeme<u32>, lrlex::DefaultLexeme<u32>>,
        mark_use: bool,
    ) -> TreeId {
        let span = token_span(&tok);
        self.update_cursor_from_span(lexer, span);
        let sym = self.arena.symbol(lexer.span_str(span));
        if mark_use {
            self.ctx.set_use_prop_at_cursor(sym);
        }
        sym
    }

    /// Builds a raw symbol from one token text (used for `STRING`/`FSTRING` in foreign forms).
    #[must_use]
    pub fn raw_symbol_from_token<'lexer, 'input: 'lexer>(
        &mut self,
        lexer: &'lexer dyn NonStreamingLexer<'input, DefaultLexerTypes<u32>>,
        tok: Result<lrlex::DefaultLexeme<u32>, lrlex::DefaultLexeme<u32>>,
    ) -> TreeId {
        let span = token_span(&tok);
        self.update_cursor_from_span(lexer, span);
        self.arena.symbol(lexer.span_str(span))
    }

    /// Builds type code node for foreign signatures (`int=0`, `float=1`, `any=2`).
    #[must_use]
    pub fn foreign_type_code(&mut self, code: i64) -> TreeId {
        self.arena.int(code)
    }

    /// Builds the 4-slot function name list used by C++ foreign signature encoding.
    #[must_use]
    pub fn foreign_name_slots(
        &mut self,
        n1: TreeId,
        n2: Option<TreeId>,
        n3: Option<TreeId>,
        n4: Option<TreeId>,
    ) -> TreeId {
        let nil = self.nil();
        let s2 = n2.unwrap_or(n1);
        let s3 = n3.unwrap_or(s2);
        let s4 = n4.unwrap_or(s3);
        let l3 = self.cons(s4, nil);
        let l2 = self.cons(s3, l3);
        let l1 = self.cons(s2, l2);
        self.cons(n1, l1)
    }

    /// Builds C++-shaped foreign signature list: `cons(ret_type, cons(names4, arg_types))`.
    #[must_use]
    pub fn foreign_signature(
        &mut self,
        ret_type: TreeId,
        names4: TreeId,
        arg_types: TreeId,
    ) -> TreeId {
        let payload = self.cons(names4, arg_types);
        self.cons(ret_type, payload)
    }

    /// Builds C++-equivalent foreign-function descriptor and wraps it as `boxFFun`.
    #[must_use]
    pub fn node_foreign_function(
        &mut self,
        signature: TreeId,
        incfile: TreeId,
        libfile: TreeId,
    ) -> TreeId {
        let ff = self.node_builder().ffunction(signature, incfile, libfile);
        self.node_builder().ffun(ff)
    }

    /// Builds one `boxCase` after C++-style rule checks and pattern preparation.
    ///
    /// Checks:
    /// - non-empty rule list,
    /// - identical arity for all rules.
    ///
    /// Pattern preparation mirrors C++ `prepareRule(s)` behavior:
    /// only the left-hand side list is transformed recursively.
    #[must_use]
    pub fn node_case_checked(&mut self, rules: TreeId) -> TreeId {
        if self.arena.is_nil(rules) {
            self.ctx.error("a case expression can't be empty");
            return self.nil();
        }

        let Some(expected_arity) = self.case_rules_arity_reference(rules) else {
            self.ctx.error("invalid case rule list shape");
            return self.nil();
        };

        let mut mapped = Vec::new();
        let mut cursor = rules;
        while !self.arena.is_nil(cursor) {
            let Some(rule) = self.arena.hd(cursor) else {
                self.ctx.error("invalid case rule list cell");
                return self.nil();
            };
            let Some((lhs, rhs)) = self.pair_cell(rule) else {
                self.ctx.error("invalid case rule shape");
                return self.nil();
            };
            let Some(arity) = self.list_len(lhs) else {
                self.ctx.error("invalid case rule lhs list");
                return self.nil();
            };
            if arity != expected_arity {
                self.ctx
                    .error("inconsistent number of parameters in pattern-matching rule");
                return self.nil();
            }
            let lhs_prepared = self.prepare_pattern(lhs);
            mapped.push(self.cons(lhs_prepared, rhs));
            cursor = self.arena.tl(cursor).unwrap_or_else(|| self.nil());
        }

        let mut mapped_rules = self.nil();
        for rule in mapped.iter().rev() {
            mapped_rules = self.cons(*rule, mapped_rules);
        }
        self.node_builder().case(mapped_rules)
    }

    /// Equivalent to C++ `buildBoxAbstr(params, body)` for parser lambda forms.
    #[must_use]
    pub fn node_lambda(&mut self, params: TreeId, body: TreeId) -> TreeId {
        self.node_builder().build_abstr(params, body)
    }

    fn case_rules_arity_reference(&self, rules: TreeId) -> Option<usize> {
        let first_rule = self.arena.hd(rules)?;
        let (lhs, _rhs) = self.pair_cell(first_rule)?;
        self.list_len(lhs)
    }

    fn pair_cell(&self, pair: TreeId) -> Option<(TreeId, TreeId)> {
        let head = self.arena.hd(pair)?;
        let tail = self.arena.tl(pair)?;
        Some((head, tail))
    }

    fn list_len(&self, mut list: TreeId) -> Option<usize> {
        let mut n = 0usize;
        while !self.arena.is_nil(list) {
            let _head = self.arena.hd(list)?;
            list = self.arena.tl(list)?;
            n = n.saturating_add(1);
        }
        Some(n)
    }

    fn map_list_with(
        &mut self,
        mut list: TreeId,
        mut f: impl FnMut(&mut Self, TreeId) -> TreeId,
    ) -> TreeId {
        let mut items = Vec::new();
        while !self.arena.is_nil(list) {
            let Some(head) = self.arena.hd(list) else {
                break;
            };
            items.push(f(self, head));
            list = self.arena.tl(list).unwrap_or_else(|| self.nil());
        }
        let mut out = self.nil();
        for item in items.iter().rev() {
            out = self.cons(*item, out);
        }
        out
    }

    fn definition_name_and_payload(&self, def: TreeId) -> Option<(TreeId, TreeId)> {
        let name = self.arena.hd(def)?;
        let payload = self.arena.tl(def)?;
        Some((name, payload))
    }

    fn definition_name_key(&self, name: TreeId) -> Option<String> {
        match match_box(&self.arena, name) {
            boxes::BoxMatch::Ident(text) => Some(text.to_owned()),
            _ => match self.arena.kind(name) {
                Some(NodeKind::Symbol(text)) => Some(text.as_ref().to_owned()),
                _ => None,
            },
        }
    }

    fn definition_payload_parts(&self, payload: TreeId) -> Option<(TreeId, TreeId)> {
        let args = self.arena.hd(payload)?;
        let body = self.arena.tl(payload)?;
        Some((args, body))
    }

    fn standard_arg_list(&self, mut args: TreeId) -> bool {
        let mut seen = HashSet::new();
        while !self.arena.is_nil(args) {
            let Some(head) = self.arena.hd(args) else {
                return false;
            };
            let Some(name) = self.definition_name_key(head) else {
                return false;
            };
            if !seen.insert(name) {
                return false;
            }
            let Some(tail) = self.arena.tl(args) else {
                return false;
            };
            args = tail;
        }
        true
    }

    fn list_len_strict(&self, mut list: TreeId) -> Option<usize> {
        let mut len = 0usize;
        while !self.arena.is_nil(list) {
            let _ = self.arena.hd(list)?;
            list = self.arena.tl(list)?;
            len = len.saturating_add(1);
        }
        Some(len)
    }

    /// Returns true when a same-name definition group contains at least one
    /// plain `name = body;` clause.
    ///
    /// C++ uses the first variant's arity as the pattern-matching reference and
    /// errors when that arity is zero. Detecting any zero-arity variant here
    /// also catches mixed invalid forms such as `foo = 1; foo(x) = x;` before
    /// they can be lowered to a misleading `case`.
    fn group_has_zero_arity_variants(&self, variants_rev: &[TreeId]) -> bool {
        variants_rev.iter().any(|payload| {
            self.definition_payload_parts(*payload)
                .is_some_and(|(args, _)| self.arena.is_nil(args))
        })
    }

    /// Emits the parser diagnostic equivalent of C++ `printRedefinitionError`.
    ///
    /// C++ folds the conflicting clauses into the message text:
    ///
    /// ```text
    /// multiple definitions of symbol 'foo'
    /// foo = ...;
    /// foo = ...;
    /// ```
    ///
    /// The Rust diagnostic keeps the message to one line and moves the clauses
    /// into a typed `declarations` fact plus one label per declaration site, so
    /// a reader sees *where* each clause is and a tool does not have to split a
    /// multi-line message.
    ///
    /// `variants_rev` stores payloads in parser-list reverse order, so it is
    /// iterated in reverse to follow source order. `dump_box` renders the
    /// clauses because this parser layer only has normalized box nodes at this
    /// point; exact pretty-print parity matters less than preserving the
    /// semantic error boundary.
    fn report_zero_arity_redefinition(&mut self, name: TreeId, variants_rev: &[TreeId]) {
        let name_text = self
            .definition_name_key(name)
            .unwrap_or_else(|| "<invalid>".to_owned());
        let mut declarations = Vec::new();
        for payload in variants_rev.iter().rev() {
            let Some((args, body)) = self.definition_payload_parts(*payload) else {
                continue;
            };
            if self.arena.is_nil(args) {
                declarations.push(format!("{name_text} = {};", dump_box(&self.arena, body)));
            } else {
                declarations.push(format!(
                    "{name_text}{} = {};",
                    dump_box(&self.arena, args),
                    dump_box(&self.arena, body)
                ));
            }
        }
        let sites = self.declaration_sites(name);
        self.ctx.error_conflicting_declarations(
            codes::PARSE_UNEXPECTED_TOKEN,
            &format!("multiple definitions of symbol '{name_text}'"),
            "duplicate-definition",
            &name_text,
            &sites,
            &declarations,
        );
    }

    /// Returns every recorded definition-side occurrence of one identifier, in
    /// source order.
    ///
    /// Hash-consing gives all occurrences of the same identifier one node, so
    /// the occurrence list recorded by the grammar actions is what distinguishes
    /// the participating declaration sites.
    fn declaration_sites(&self, name: TreeId) -> Vec<SourceLocation> {
        let mut sites = self
            .ctx
            .box_provenance()
            .origins_for(name)
            .iter()
            .filter_map(|id| self.ctx.box_provenance().get(*id))
            .filter(|origin| origin.role == BoxOriginRole::Definition)
            .map(|origin| origin.location.clone())
            .collect::<Vec<_>>();
        sites.dedup_by(|left, right| {
            left.file() == right.file() && left.line() == right.line() && left.col() == right.col()
        });
        sites
    }

    fn make_definition_from_variants(&mut self, name: TreeId, variants_rev: &[TreeId]) -> TreeId {
        let mut variants = variants_rev.iter().rev();
        let Some(first_payload) = variants.next().copied() else {
            self.ctx.error("definition group should not be empty");
            return self.nil();
        };
        let Some((first_args, first_body)) = self.definition_payload_parts(first_payload) else {
            self.ctx.error("invalid definition payload");
            return self.nil();
        };

        let formatted_expr = if variants_rev.len() == 1 {
            if self.arena.is_nil(first_args) {
                first_body
            } else if self.standard_arg_list(first_args) {
                self.node_builder().build_abstr(first_args, first_body)
            } else {
                let nil = self.nil();
                let rules = self.cons(first_payload, nil);
                self.node_case_checked(rules)
            }
        } else {
            let Some(expected_arity) = self.list_len_strict(first_args) else {
                self.ctx.error("invalid definition arglist");
                return self.nil();
            };
            let mut rules = self.nil();
            let mut prev_args = first_args;
            let mut prev_body = first_body;
            for payload in variants_rev.iter().rev() {
                let Some((args, body)) = self.definition_payload_parts(*payload) else {
                    self.ctx.error("invalid definition payload");
                    return self.nil();
                };
                let Some(arity) = self.list_len_strict(args) else {
                    self.ctx.error("invalid definition arglist");
                    return self.nil();
                };
                if arity != expected_arity {
                    self.ctx.error(&format!(
                        "inconsistent number of parameters in pattern-matching rule: previous arity {expected_arity}, got {arity}"
                    ));
                    let _ = (prev_args, prev_body, body);
                    return self.nil();
                }
                prev_args = args;
                prev_body = body;
                rules = self.cons(*payload, rules);
            }
            self.node_case_checked(rules)
        };

        let with_metadata = self.apply_declared_definition_metadata(name, formatted_expr);
        let nil = self.nil();
        self.make_definition(name, nil, with_metadata)
    }

    /// Reinjects parser-recorded `declare <def> <key> <value>;` entries like C++
    /// `addFunctionMetadata`.
    ///
    /// Source provenance (C++):
    /// - `compiler/parser/sourcereader.cpp`
    /// - `declareDefinitionMetadata`
    /// - `addFunctionMetadata`
    ///
    /// Mapping status: `adapted`.
    ///
    /// Rust intentionally keeps top-level `declare key value;` entries as
    /// parser-context metadata (`adapted` representation), while
    /// definition-scoped metadata is lowered into explicit `BOXMETADATA`
    /// wrappers so it survives parser-to-eval transport like the C++ pipeline.
    fn apply_declared_definition_metadata(&mut self, name: TreeId, expr: TreeId) -> TreeId {
        let Some(def_name) = self.definition_name_key(name) else {
            return expr;
        };

        let mut out = expr;
        let source_file = self.source_file.to_string();
        let entries: Vec<(String, String)> = self
            .ctx
            .declared_definition_metadata()
            .iter()
            .filter(|(target, _, _)| target.as_ref() == def_name)
            .map(|(_, key, value)| (key.to_string(), value.to_string()))
            .collect();
        for (key, value) in entries {
            let full_key = format!("{source_file}/{def_name}:{key}");
            let key_node = self.arena.symbol(full_key);
            let value_node = self.arena.string_lit(value);
            let md_pair = self.cons(key_node, value_node);
            out = self.node_builder().metadata(out, md_pair);
        }
        out
    }

    /// Prepares one parser-side pattern using the same opacity boundary as C++ `preparePattern()`.
    ///
    /// Source provenance (C++):
    /// - `compiler/boxes/boxes.cpp`
    /// - `preparePattern(Tree box)`
    ///
    /// Mapping status: `1:1` semantics.
    ///
    /// The important parity point is not merely turning identifiers into
    /// `BOXPATVAR`, but doing so only through the same recursive subset as the
    /// C++ parser helper. Forms such as `abstr`, `access`, `component`,
    /// `environment`, `slot`, `symbolic`, and `case` stay opaque.
    fn prepare_pattern(&mut self, node: TreeId) -> TreeId {
        if matches!(self.arena.kind(node), Some(NodeKind::Cons)) {
            return self.map_list_with(node, |s, id| s.prepare_pattern(id));
        }

        match match_box(&self.arena, node) {
            BoxMatch::Ident(_) => self.node_builder().pattern_var(node),
            BoxMatch::Appl(fun, args) => {
                let mapped_args = self.map_list_with(args, |s, id| s.prepare_pattern(id));
                let mapped_fun = match match_box(&self.arena, fun) {
                    BoxMatch::Ident(_) => fun,
                    _ => self.prepare_pattern(fun),
                };
                self.node_builder().appl(mapped_fun, mapped_args)
            }
            BoxMatch::WithLocalDef(body, ldef) => {
                let prepared_body = self.prepare_pattern(body);
                self.node_builder().with_local_def(prepared_body, ldef)
            }
            BoxMatch::Seq(left, right) => {
                let prepared_left = self.prepare_pattern(left);
                let prepared_right = self.prepare_pattern(right);
                self.node_builder().seq(prepared_left, prepared_right)
            }
            BoxMatch::Split(left, right) => {
                let prepared_left = self.prepare_pattern(left);
                let prepared_right = self.prepare_pattern(right);
                self.node_builder().split(prepared_left, prepared_right)
            }
            BoxMatch::Merge(left, right) => {
                let prepared_left = self.prepare_pattern(left);
                let prepared_right = self.prepare_pattern(right);
                self.node_builder().merge(prepared_left, prepared_right)
            }
            BoxMatch::Par(left, right) => {
                let prepared_left = self.prepare_pattern(left);
                let prepared_right = self.prepare_pattern(right);
                self.node_builder().par(prepared_left, prepared_right)
            }
            BoxMatch::Rec(left, right) => {
                let prepared_left = self.prepare_pattern(left);
                let prepared_right = self.prepare_pattern(right);
                self.node_builder().rec(prepared_left, prepared_right)
            }
            BoxMatch::Route(n, m, route_spec) => {
                let prepared_n = self.prepare_pattern(n);
                let prepared_m = self.prepare_pattern(m);
                let prepared_route_spec = self.prepare_pattern(route_spec);
                self.node_builder()
                    .route(prepared_n, prepared_m, prepared_route_spec)
            }
            BoxMatch::IPar(index, count, body) => {
                let prepared_body = self.prepare_pattern(body);
                self.node_builder().ipar(index, count, prepared_body)
            }
            BoxMatch::ISeq(index, count, body) => {
                let prepared_body = self.prepare_pattern(body);
                self.node_builder().iseq(index, count, prepared_body)
            }
            BoxMatch::ISum(index, count, body) => {
                let prepared_body = self.prepare_pattern(body);
                self.node_builder().isum(index, count, prepared_body)
            }
            BoxMatch::IProd(index, count, body) => {
                let prepared_body = self.prepare_pattern(body);
                self.node_builder().iprod(index, count, prepared_body)
            }
            BoxMatch::Inputs(expr) => {
                let prepared_expr = self.prepare_pattern(expr);
                self.node_builder().inputs(prepared_expr)
            }
            BoxMatch::Outputs(expr) => {
                let prepared_expr = self.prepare_pattern(expr);
                self.node_builder().outputs(prepared_expr)
            }
            BoxMatch::VGroup(label, expr) => {
                let prepared_expr = self.prepare_pattern(expr);
                self.node_builder().vgroup(label, prepared_expr)
            }
            BoxMatch::HGroup(label, expr) => {
                let prepared_expr = self.prepare_pattern(expr);
                self.node_builder().hgroup(label, prepared_expr)
            }
            BoxMatch::TGroup(label, expr) => {
                let prepared_expr = self.prepare_pattern(expr);
                self.node_builder().tgroup(label, prepared_expr)
            }
            _ => node,
        }
    }

    /// Parses one integer literal token to `boxInt`.
    #[must_use]
    pub fn int_from_token<'lexer, 'input: 'lexer>(
        &mut self,
        lexer: &'lexer dyn NonStreamingLexer<'input, DefaultLexerTypes<u32>>,
        tok: Result<lrlex::DefaultLexeme<u32>, lrlex::DefaultLexeme<u32>>,
    ) -> TreeId {
        let span = token_span(&tok);
        self.update_cursor_from_span(lexer, span);
        let raw = lexer.span_str(span);
        if raw.bytes().all(|b| b.is_ascii_digit()) {
            self.node_builder().int(i32_wrapping_from_str(raw))
        } else {
            self.ctx
                .error_with_code(codes::PARSE_INVALID_LITERAL, "invalid INT literal");
            self.node_builder().int(0)
        }
    }

    /// Parses one float literal token to `boxReal`.
    #[must_use]
    pub fn float_from_token<'lexer, 'input: 'lexer>(
        &mut self,
        lexer: &'lexer dyn NonStreamingLexer<'input, DefaultLexerTypes<u32>>,
        tok: Result<lrlex::DefaultLexeme<u32>, lrlex::DefaultLexeme<u32>>,
    ) -> TreeId {
        let span = token_span(&tok);
        self.update_cursor_from_span(lexer, span);
        let raw = lexer.span_str(span);
        let normalized = raw.strip_suffix('f').unwrap_or(raw);
        match normalized.parse::<f64>() {
            Ok(value) => self.node_builder().real(value),
            Err(_) => {
                self.ctx
                    .error_with_code(codes::PARSE_INVALID_LITERAL, "invalid FLOAT literal");
                self.node_builder().real(0.0)
            }
        }
    }

    /// Parses one quoted string token and removes outer quotes.
    #[must_use]
    pub fn uqstring_from_token<'lexer, 'input: 'lexer>(
        &mut self,
        lexer: &'lexer dyn NonStreamingLexer<'input, DefaultLexerTypes<u32>>,
        tok: Result<lrlex::DefaultLexeme<u32>, lrlex::DefaultLexeme<u32>>,
    ) -> TreeId {
        let span = token_span(&tok);
        self.update_cursor_from_span(lexer, span);
        let raw = lexer.span_str(span);
        let stripped = raw
            .strip_prefix('"')
            .and_then(|s| s.strip_suffix('"'))
            .unwrap_or(raw);
        self.arena.string_lit(stripped)
    }

    fn string_node_text(&self, node: TreeId) -> Option<&str> {
        match self.arena.kind(node) {
            Some(NodeKind::StringLiteral(value)) => Some(value.as_ref()),
            Some(NodeKind::Symbol(value)) => Some(value.as_ref()),
            _ => None,
        }
    }

    /// Records one import statement and returns an explicit parser/import node.
    ///
    /// Source provenance (C++):
    /// - `compiler/boxes/boxes.cpp`
    /// - `importFile(Tree filename)`
    /// - `compiler/parser/sourcereader.cpp`
    /// - `formatDefinitions(Tree rldef)`
    ///
    /// Mapping status: `1:1`.
    ///
    /// Rust keeps `import("...")` as a structural node so later file-backed
    /// parsing and eval flows can expand imports from parsed definition trees
    /// like the C++ compiler, instead of depending on raw-source flattening.
    #[must_use]
    pub fn import_statement(&mut self, path_node: TreeId) -> TreeId {
        match self.string_node_text(path_node).map(str::to_owned) {
            Some(path) => {
                self.ctx.note_import(&path);
                self.node_builder().import_file(path_node)
            }
            None => {
                self.ctx
                    .error_with_code(codes::PARSE_INVALID_LITERAL, "invalid import path literal");
                self.nil()
            }
        }
    }

    /// Records one `declare key value;` statement and returns `nil`.
    #[must_use]
    pub fn declare_metadata_from_token<'lexer, 'input: 'lexer>(
        &mut self,
        lexer: &'lexer dyn NonStreamingLexer<'input, DefaultLexerTypes<u32>>,
        key_tok: Result<lrlex::DefaultLexeme<u32>, lrlex::DefaultLexeme<u32>>,
        value_node: TreeId,
    ) -> TreeId {
        let key_span = token_span(&key_tok);
        self.update_cursor_from_span(lexer, key_span);
        let key = lexer.span_str(key_span);
        match self.string_node_text(value_node).map(str::to_owned) {
            Some(value) => {
                self.ctx.note_declared_metadata(key, &value);
                let current_source = self.ctx.cursor().file().to_owned();
                self.metadata_store
                    .declare_top_level(&current_source, key, &value);
            }
            None => self.ctx.error("invalid declare metadata value"),
        }
        self.nil()
    }

    /// Records one `declare def key value;` statement and returns `nil`.
    #[must_use]
    pub fn declare_definition_metadata_from_tokens<'lexer, 'input: 'lexer>(
        &mut self,
        lexer: &'lexer dyn NonStreamingLexer<'input, DefaultLexerTypes<u32>>,
        def_tok: Result<lrlex::DefaultLexeme<u32>, lrlex::DefaultLexeme<u32>>,
        key_tok: Result<lrlex::DefaultLexeme<u32>, lrlex::DefaultLexeme<u32>>,
        value_node: TreeId,
    ) -> TreeId {
        let def_span = token_span(&def_tok);
        self.update_cursor_from_span(lexer, def_span);
        let def = lexer.span_str(def_span);
        let key = lexer.span_str(token_span(&key_tok));
        match self.string_node_text(value_node).map(str::to_owned) {
            Some(value) => self.ctx.note_declared_definition_metadata(def, key, &value),
            None => self.ctx.error("invalid declare definition metadata value"),
        }
        self.nil()
    }

    /// Records one parsed documentation block and returns `nil`.
    #[must_use]
    pub fn doc_statement(&mut self) -> TreeId {
        self.ctx.note_doc_block();
        self.nil()
    }

    /// Records one parsed doc notice marker.
    pub fn note_doc_notice(&mut self) {
        self.ctx.note_doc_notice();
    }

    /// Records one parsed listing block.
    pub fn note_doc_listing(&mut self) {
        self.ctx.note_doc_listing();
    }

    /// Records one parsed `DOCCHAR`.
    pub fn note_doc_char(&mut self) {
        self.ctx.note_doc_char();
    }

    /// Records one parsed `<metadata>...</metadata>` tag content from `IDENT`.
    pub fn note_doc_metadata_tag_from_token<'lexer, 'input: 'lexer>(
        &mut self,
        lexer: &'lexer dyn NonStreamingLexer<'input, DefaultLexerTypes<u32>>,
        tag_tok: Result<lrlex::DefaultLexeme<u32>, lrlex::DefaultLexeme<u32>>,
    ) {
        let span = token_span(&tag_tok);
        self.update_cursor_from_span(lexer, span);
        self.ctx.note_doc_metadata_tag(lexer.span_str(span));
    }

    /// Updates listing dependencies switch.
    pub fn set_lst_dependencies(&mut self, value: bool) {
        self.ctx.set_lst_dependencies(value);
    }

    /// Updates listing mdoctags switch.
    pub fn set_lst_mdoctags(&mut self, value: bool) {
        self.ctx.set_lst_mdoctags(value);
    }

    /// Updates listing distributed switch.
    pub fn set_lst_distributed(&mut self, value: bool) {
        self.ctx.set_lst_distributed(value);
    }

    /// Appends one waveform numeric value in parse order.
    pub fn push_waveform_value(&mut self, value: TreeId) {
        self.ctx.push_waveform_value(value);
    }

    /// Builds `boxWaveform` from the accumulated parser waveform buffer and clears it.
    #[must_use]
    pub fn waveform_box_from_ctx(&mut self) -> TreeId {
        let values = self.ctx.take_waveform();
        self.node_builder().waveform(&values)
    }

    /// Builds `boxRoute(n,m,boxPar(boxInt(0),boxInt(0)))` like C++ fake-route form.
    #[must_use]
    pub fn route_box_default_spec(&mut self, n: TreeId, m: TreeId) -> TreeId {
        let z0 = self.node_builder().int(0);
        let z1 = self.node_builder().int(0);
        let fake = self.node_builder().par(z0, z1);
        self.node_builder().route(n, m, fake)
    }

    /// Parses one signed integer literal token to `boxInt`.
    #[must_use]
    pub fn signed_int_from_token<'lexer, 'input: 'lexer>(
        &mut self,
        lexer: &'lexer dyn NonStreamingLexer<'input, DefaultLexerTypes<u32>>,
        tok: Result<lrlex::DefaultLexeme<u32>, lrlex::DefaultLexeme<u32>>,
        sign: i64,
    ) -> TreeId {
        let span = token_span(&tok);
        self.update_cursor_from_span(lexer, span);
        let raw = lexer.span_str(span);
        if raw.bytes().all(|b| b.is_ascii_digit()) {
            // C++ does `-str2int(text)`: wrapping-parse unsigned digits, then negate.
            let unsigned_val = i32_wrapping_from_str(raw);
            let val = if sign < 0 {
                unsigned_val.wrapping_neg()
            } else {
                unsigned_val
            };
            self.node_builder().int(val)
        } else {
            self.ctx
                .error_with_code(codes::PARSE_INVALID_LITERAL, "invalid signed INT literal");
            self.node_builder().int(0)
        }
    }

    /// Parses one signed float literal token to `boxReal`.
    #[must_use]
    pub fn signed_float_from_token<'lexer, 'input: 'lexer>(
        &mut self,
        lexer: &'lexer dyn NonStreamingLexer<'input, DefaultLexerTypes<u32>>,
        tok: Result<lrlex::DefaultLexeme<u32>, lrlex::DefaultLexeme<u32>>,
        sign: f64,
    ) -> TreeId {
        let span = token_span(&tok);
        self.update_cursor_from_span(lexer, span);
        let raw = lexer.span_str(span);
        let normalized = raw.strip_suffix('f').unwrap_or(raw);
        match normalized.parse::<f64>() {
            Ok(value) => self.node_builder().real(value * sign),
            Err(_) => {
                self.ctx
                    .error_with_code(codes::PARSE_INVALID_LITERAL, "invalid signed FLOAT literal");
                self.node_builder().real(0.0)
            }
        }
    }

    /// Builds `boxPar(left, right)` and tags it with the operator token span.
    #[must_use]
    pub fn par_from_token<'lexer, 'input: 'lexer>(
        &mut self,
        lexer: &'lexer dyn NonStreamingLexer<'input, DefaultLexerTypes<u32>>,
        tok: Result<lrlex::DefaultLexeme<u32>, lrlex::DefaultLexeme<u32>>,
        left: TreeId,
        right: TreeId,
    ) -> TreeId {
        let node = self.node_builder().par(left, right);
        self.mark_use_from_token(lexer, tok, node)
    }

    /// Builds `boxSeq(left, right)` and tags it with the operator token span.
    #[must_use]
    pub fn seq_from_token<'lexer, 'input: 'lexer>(
        &mut self,
        lexer: &'lexer dyn NonStreamingLexer<'input, DefaultLexerTypes<u32>>,
        tok: Result<lrlex::DefaultLexeme<u32>, lrlex::DefaultLexeme<u32>>,
        left: TreeId,
        right: TreeId,
    ) -> TreeId {
        let node = self.node_builder().seq(left, right);
        self.mark_use_from_token(lexer, tok, node)
    }

    /// Builds `boxSplit(left, right)` and tags it with the operator token span.
    #[must_use]
    pub fn split_from_token<'lexer, 'input: 'lexer>(
        &mut self,
        lexer: &'lexer dyn NonStreamingLexer<'input, DefaultLexerTypes<u32>>,
        tok: Result<lrlex::DefaultLexeme<u32>, lrlex::DefaultLexeme<u32>>,
        left: TreeId,
        right: TreeId,
    ) -> TreeId {
        let node = self.node_builder().split(left, right);
        self.mark_use_from_token(lexer, tok, node)
    }

    /// Builds `boxMerge(left, right)` and tags it with the operator token span.
    #[must_use]
    pub fn merge_from_token<'lexer, 'input: 'lexer>(
        &mut self,
        lexer: &'lexer dyn NonStreamingLexer<'input, DefaultLexerTypes<u32>>,
        tok: Result<lrlex::DefaultLexeme<u32>, lrlex::DefaultLexeme<u32>>,
        left: TreeId,
        right: TreeId,
    ) -> TreeId {
        let node = self.node_builder().merge(left, right);
        self.mark_use_from_token(lexer, tok, node)
    }

    /// Builds `boxRec(left, right)` and tags it with the operator token span.
    #[must_use]
    pub fn rec_from_token<'lexer, 'input: 'lexer>(
        &mut self,
        lexer: &'lexer dyn NonStreamingLexer<'input, DefaultLexerTypes<u32>>,
        tok: Result<lrlex::DefaultLexeme<u32>, lrlex::DefaultLexeme<u32>>,
        left: TreeId,
        right: TreeId,
    ) -> TreeId {
        let node = self.node_builder().rec(left, right);
        self.mark_use_from_token(lexer, tok, node)
    }

    /// Encodes C++ infix primitive lowering: `a op b` -> `boxSeq(boxPar(a,b), boxOp())`.
    #[must_use]
    pub fn binary_prim(&mut self, left: TreeId, right: TreeId, op: PrimitiveOp) -> TreeId {
        let pair = self.node_builder().par(left, right);
        let prim = self.prim_box(op);
        self.node_builder().seq(pair, prim)
    }

    /// Encodes postfix primitive lowering: `a op` -> `boxSeq(a, boxOp())`.
    #[must_use]
    pub fn postfix_prim(&mut self, expr: TreeId, op: PrimitiveOp) -> TreeId {
        let prim = self.prim_box(op);
        self.node_builder().seq(expr, prim)
    }

    /// Equivalent to C++ `buildBoxAppl` prototype behavior (`boxAppl(fun, revarglist)`).
    #[must_use]
    pub fn apply_box(&mut self, fun: TreeId, rev_arg_list: TreeId) -> TreeId {
        self.node_builder().appl(fun, rev_arg_list)
    }

    /// Equivalent to C++ `boxAccess`.
    #[must_use]
    pub fn access_box(&mut self, expr: TreeId, ident: TreeId) -> TreeId {
        self.node_builder().access(expr, ident)
    }

    /// Equivalent to C++ `boxModifLocalDef`.
    #[must_use]
    pub fn modif_local_def_box(&mut self, expr: TreeId, defs: TreeId) -> TreeId {
        self.node_builder().modif_local_def(expr, defs)
    }

    fn prim_box(&mut self, op: PrimitiveOp) -> TreeId {
        match op {
            PrimitiveOp::Add => self.node_builder().add(),
            PrimitiveOp::Sub => self.node_builder().sub(),
            PrimitiveOp::Mul => self.node_builder().mul(),
            PrimitiveOp::Div => self.node_builder().div(),
            PrimitiveOp::Rem => self.node_builder().rem(),
            PrimitiveOp::And => self.node_builder().and(),
            PrimitiveOp::Or => self.node_builder().or(),
            PrimitiveOp::Xor => self.node_builder().xor(),
            PrimitiveOp::Lsh => self.node_builder().lsh(),
            PrimitiveOp::Rsh => self.node_builder().rsh(),
            PrimitiveOp::Lt => self.node_builder().lt(),
            PrimitiveOp::Le => self.node_builder().le(),
            PrimitiveOp::Gt => self.node_builder().gt(),
            PrimitiveOp::Ge => self.node_builder().ge(),
            PrimitiveOp::Eq => self.node_builder().eq(),
            PrimitiveOp::Ne => self.node_builder().ne(),
            PrimitiveOp::Pow => self.node_builder().pow(),
            PrimitiveOp::Delay => self.node_builder().delay(),
            PrimitiveOp::Delay1 => self.node_builder().delay1(),
        }
    }

    /// Records one written UI widget declaration and returns the widget node
    /// unchanged.
    ///
    /// The cursor is moved to the widget keyword first so the recorded location
    /// points at `hslider`/`vbargraph`/... rather than at whichever argument
    /// token the parser last consumed.
    fn record_widget_declaration<'lexer, 'input: 'lexer>(
        &mut self,
        lexer: &'lexer dyn NonStreamingLexer<'input, DefaultLexerTypes<u32>>,
        keyword: Result<lrlex::DefaultLexeme<u32>, lrlex::DefaultLexeme<u32>>,
        label: TreeId,
        node: TreeId,
    ) -> TreeId {
        let span = token_span(&keyword);
        self.update_cursor_from_span(lexer, span);
        let raw_label = self.string_node_text(label).unwrap_or_default().to_owned();
        self.ctx.record_widget_declaration(&raw_label);
        node
    }

    fn mark_use_from_token<'lexer, 'input: 'lexer>(
        &mut self,
        lexer: &'lexer dyn NonStreamingLexer<'input, DefaultLexerTypes<u32>>,
        tok: Result<lrlex::DefaultLexeme<u32>, lrlex::DefaultLexeme<u32>>,
        node: TreeId,
    ) -> TreeId {
        let span = token_span(&tok);
        self.update_cursor_from_span(lexer, span);
        self.ctx.set_use_prop_at_cursor(node);
        node
    }

    fn update_cursor_from_span<'lexer, 'input: 'lexer>(
        &mut self,
        _lexer: &'lexer dyn NonStreamingLexer<'input, DefaultLexerTypes<u32>>,
        span: Span,
    ) {
        let ((line, col), (end_line, end_col)) = self.span_line_col(span);
        if self.source_origins.is_none() {
            self.ctx.set_cursor_span(
                &self.source_file,
                u32::try_from(line).unwrap_or(u32::MAX),
                u32::try_from(col).unwrap_or(u32::MAX),
                u32::try_from(end_line).unwrap_or(u32::MAX),
                u32::try_from(end_col).unwrap_or(u32::MAX),
            );
            return;
        }

        let (file, mapped_line) = self.resolve_source_location(line);
        let (_, mapped_end_line) = self.resolve_source_location(end_line);
        let file_owned = file.to_string_lossy().into_owned();
        self.ctx.set_cursor_span(
            &file_owned,
            mapped_line,
            u32::try_from(col).unwrap_or(u32::MAX),
            mapped_end_line,
            u32::try_from(end_col).unwrap_or(u32::MAX),
        );
    }

    fn resolve_source_location(&self, line: usize) -> (std::path::PathBuf, u32) {
        if let Some(origins) = &self.source_origins
            && let Some(origin) = origins.get(line.saturating_sub(1))
        {
            return (origin.file.clone(), origin.line);
        }
        (
            std::path::PathBuf::from(self.source_file.as_ref()),
            u32::try_from(line).unwrap_or(u32::MAX),
        )
    }

    fn span_line_col(&self, span: Span) -> ((usize, usize), (usize, usize)) {
        (
            self.offset_line_col(span.start()),
            self.offset_line_col(span.end()),
        )
    }

    fn offset_line_col(&self, offset: usize) -> (usize, usize) {
        let line_idx = match self.source_line_starts.binary_search(&offset) {
            Ok(idx) => idx,
            Err(0) => 0,
            Err(idx) => idx.saturating_sub(1),
        };
        let line_start = self.source_line_starts[line_idx];
        (
            line_idx.saturating_add(1),
            offset.saturating_sub(line_start).saturating_add(1),
        )
    }
}

fn compute_line_starts(input: &str) -> Vec<usize> {
    let mut starts = vec![0];
    for (idx, byte) in input.bytes().enumerate() {
        if byte == b'\n' {
            starts.push(idx.saturating_add(1));
        }
    }
    starts
}

/// Maps one lexer token (or lexer error token) to its raw span.
fn token_span(tok: &Result<lrlex::DefaultLexeme<u32>, lrlex::DefaultLexeme<u32>>) -> Span {
    match tok {
        Ok(lexeme) | Err(lexeme) => lexeme.span(),
    }
}

/// Converts an `i64` to `i32` with Faust-style wrapping.
///
/// The C++ Faust parser uses a manual `str2int` that accumulates digits into
/// a 32-bit `int` via `result = result * 10 + digit`, which naturally wraps
/// on overflow.  We replicate the same digit-by-digit wrapping so that
/// literals like `2147483648` produce the same bit pattern (`-2147483648`).
fn i32_wrapping_from_str(raw: &str) -> i32 {
    let mut result: i32 = 0;
    for b in raw.bytes() {
        debug_assert!(b.is_ascii_digit(), "non-digit byte in integer literal");
        result = result.wrapping_mul(10).wrapping_add((b - b'0') as i32);
    }
    result
}

/// Executes one mutable operation against parser state passed through `%parse-param`.
pub fn with_state<T>(state: &RefCell<ParseState>, f: impl FnOnce(&mut ParseState) -> T) -> T {
    let mut state = state.borrow_mut();
    f(&mut state)
}

lrlex_mod!("grammar/faustlexer.l");
lrpar_mod!("grammar/faustparser.y");

/// One lexed token with normalized name/text/location information.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LexedToken {
    pub name: Box<str>,
    pub text: Box<str>,
    pub span: Span,
    pub start_line: u32,
    pub start_col: u32,
}

/// Full output of one parse invocation.
///
/// Keeps both structural parse artifacts and diagnostics so later compiler
/// stages can reuse one parse session without recomputing metadata.
#[derive(Debug)]
pub struct ParseOutput {
    /// Root box node of the parsed program, or `None` if parsing failed entirely.
    pub root: Option<TreeId>,
    /// Raw parser error strings collected during recovery.
    pub errors: Vec<String>,
    /// Structured diagnostics (errors, warnings, remarks) emitted by the parser.
    pub diagnostics: DiagnosticBundle,
    /// Deterministic snapshot of the compilation-global top-level metadata set.
    ///
    /// Source provenance (C++):
    /// - `compiler/parser/sourcereader.cpp`
    /// - `declareMetadata(Tree key, Tree value)`
    /// - `gGlobal->gMetaDataSet`
    ///
    /// Mapping status: `1:1` semantics, adapted representation.
    ///
    /// The parser still keeps local `ParserCtx` bookkeeping for diagnostics and
    /// structural tests, but this snapshot is the canonical session-wide view
    /// of top-level `declare key "value";` statements seen so far.
    ///
    /// Later compilation stages must prefer this snapshot over ad hoc parser
    /// cursor state when they need the aggregate metadata result of one whole
    /// parse/import session.
    pub compilation_metadata: CompilationMetadataSnapshot,
    /// Canonical source files consumed by parser input resolution.
    ///
    /// - For `parse_program(...)`, this list is empty because no filesystem import
    ///   resolution occurs in-memory.
    /// - For `parse_file_with_imports(...)`, this list contains the deterministic
    ///   recursive import expansion order from [`SourceReader`], including the entry file.
    ///
    /// This list is primarily an audit/debugging artifact: it records which
    /// concrete files contributed text to the parse and in which stable order.
    pub used_files: Vec<std::path::PathBuf>,
    /// Parser context and arena retained for downstream structural checks.
    pub state: ParseState,
}

/// Returns the compiled Faust lexer definition (generated by `lrlex`).
///
/// Builds a fresh definition; see [`shared_lexerdef`] for the cached one used
/// on the parsing path.
#[must_use]
pub fn lexerdef() -> LRNonStreamingLexerDef<DefaultLexerTypes<u32>> {
    faustlexer_l::lexerdef()
}

/// Returns the process-wide Faust lexer definition.
///
/// Constructing it compiles the 128 rules of `faustlexer.l` into regex
/// automata, which measures at 2.3 ms — paid once per *file* parsed before
/// this cache, and a compilation parses ten of them for a two-line DSP that
/// imports `stdfaust.lib`. That was 23 ms of a 249 ms compile spent rebuilding
/// a constant.
///
/// Sharing is sound because the definition is immutable: `lexerdef.lexer(input)`
/// borrows it and keeps all mutable lexing state — position, start conditions
/// for the `comment`/`doc`/`lst` exclusive states — in the returned lexer. This
/// is not the arena-memo hazard of
/// `porting/eval-box-simplification-memoization-analysis-2026-08-06-en.md`:
/// nothing here is keyed by anything a compilation owns.
fn shared_lexerdef() -> &'static LRNonStreamingLexerDef<DefaultLexerTypes<u32>> {
    static LEXERDEF: std::sync::OnceLock<LRNonStreamingLexerDef<DefaultLexerTypes<u32>>> =
        std::sync::OnceLock::new();
    LEXERDEF.get_or_init(faustlexer_l::lexerdef)
}

/// Which lexing strategy `lex_stream` should use.
///
/// Exists so the token streams of the two can be compared over the whole
/// corpus before either becomes the default
/// (`porting/lexer-combined-dfa-port-plan-2026-08-06-en.md`, phase L0).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LexerImpl {
    /// `lrlex`: one anchored regex per rule, all eligible ones run at every
    /// token start. O(tokens x rules).
    PerRule,
    /// One multi-pattern DFA per start condition. O(input bytes).
    CombinedDfa,
}

/// One lexeme, reduced to what the parser actually consumes.
///
/// Deliberately not [`LexedToken`]: that carries a resolved name and
/// line/column, which are derived. Equality of *these* triples is the property
/// a lexer replacement has to preserve, and comparing derived fields instead
/// would let a difference in the raw stream hide behind a shared derivation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RawLexeme {
    pub tok_id: u32,
    pub start: usize,
    pub len: usize,
}

/// How a lex attempt ended.
///
/// Failures are part of the compared contract, not an absence of one: error
/// spans reach diagnostics that this project gates on, so a replacement that
/// gets every successful file right and reports failures one byte off is still
/// wrong.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LexOutcome {
    /// Lexing consumed the whole input.
    Complete(Vec<RawLexeme>),
    /// Lexing stopped; `error_at` is the byte offset of the failure, and the
    /// lexemes produced before it are kept for comparison.
    Failed {
        lexemes: Vec<RawLexeme>,
        error_at: usize,
    },
}

/// Lexes `input` with the requested strategy, returning the raw token stream.
///
/// # Errors
/// Never returns `Err`; a lex failure is reported as [`LexOutcome::Failed`] so
/// the failing position can be compared like any other observable.
#[must_use]
pub fn lex_stream(input: &str, impl_: LexerImpl) -> LexOutcome {
    match impl_ {
        // L0: both arms are `lrlex`, so the differential harness can be landed
        // and shown green against a known-identical pair before the new lexer
        // exists. A harness that first runs against a real change cannot
        // distinguish "the change is correct" from "the harness compares
        // nothing".
        LexerImpl::PerRule => lex_stream_per_rule(input),
        LexerImpl::CombinedDfa => lex_stream_combined(input),
    }
}

/// The combined multi-pattern automata, one per start condition.
///
/// Built once per process behind a `OnceLock`, like [`shared_lexerdef`].
struct CombinedDfas {
    /// Indexed by start-state id.
    states: Vec<StateDfa>,
    /// Token id for each rule index, `None` for rules that skip.
    tok_ids: Vec<Option<u32>>,
    /// `(target_state_id, operation)` for each rule index.
    targets: Vec<Option<(usize, lrlex::StartStateOperation)>>,
}

struct StateDfa {
    dfa: regex_automata::hybrid::dfa::DFA,
    /// Local `PatternID` to global rule index. Ascending, so the DFA's
    /// lowest-pattern-id tie-break is `lrlex`'s earliest-rule tie-break.
    rules: Vec<usize>,
}

/// Start conditions this implementation is known to handle.
///
/// `lrlex` does not expose `StartState::exclusive`, so eligibility for rules
/// with no explicit start states — which match only in *non-exclusive*
/// conditions — cannot be read from the API. It is derived here from the one
/// fact that is checked rather than assumed: `INITIAL` is inclusive and the
/// `%x` conditions are exclusive. If `faustlexer.l` ever declares a `%s`
/// (inclusive) condition, this list stops matching and the build fails loudly
/// instead of silently making that condition exclusive.
const KNOWN_START_CONDITIONS: [&str; 4] = ["INITIAL", "comment", "doc", "lst"];

fn shared_combined_dfas() -> &'static CombinedDfas {
    static DFAS: std::sync::OnceLock<CombinedDfas> = std::sync::OnceLock::new();
    DFAS.get_or_init(build_combined_dfas)
}

fn build_combined_dfas() -> CombinedDfas {
    use lrlex::LexerDef as _;
    use regex_automata::{MatchKind, hybrid::dfa::DFA};

    let def = shared_lexerdef();

    let names: Vec<&str> = def.iter_start_states().map(lrlex::StartState::name).collect();
    assert_eq!(
        names, KNOWN_START_CONDITIONS,
        "faustlexer.l declares start conditions this lexer was not written for; \
         `INITIAL` must be the only inclusive one (see KNOWN_START_CONDITIONS)"
    );

    let rules: Vec<_> = def.iter_rules().collect();

    // Name -> token id, recovered from the grammar. `Rule::tok_id` is not part
    // of lrlex's public API, but a named rule's id *is* the grammar's token
    // index — that is how `lrpar` matches the two — so inverting `token_epp`
    // recovers the same mapping rather than inventing a parallel one.
    //
    // `token_epp` panics past the last token instead of returning `None`, and
    // the grammar exports no count, so the scan stops as soon as every rule
    // name has been resolved. A build where some named rule has no grammar
    // token cannot lex at all under `lrlex` either — it would set that rule's
    // id to `None` — so the assertion below is the honest failure for it.
    let mut wanted: std::collections::HashSet<&str> =
        rules.iter().filter_map(|r| r.name()).collect();
    let mut tok_id_of_name: std::collections::HashMap<&str, u32> =
        std::collections::HashMap::new();
    let mut idx = 0u32;
    while !wanted.is_empty() {
        let name = faustparser_y::token_epp(cfgrammar::TIdx(idx));
        if let Some(name) = name {
            wanted.remove(name);
            tok_id_of_name.insert(name, idx);
        }
        idx += 1;
    }
    assert!(
        wanted.is_empty(),
        "lexer rules name tokens the grammar does not define: {wanted:?}"
    );
    let tok_ids = rules
        .iter()
        .map(|r| r.name().and_then(|n| tok_id_of_name.get(n).copied()))
        .collect();
    let targets = rules.iter().map(|r| r.target_state()).collect();

    let states = (0..names.len())
        .map(|state_id| {
            let exclusive = state_id != 0;
            let mut local = Vec::new();
            let mut pats = Vec::new();
            for (ridx, r) in rules.iter().enumerate() {
                let eligible = if r.start_states().is_empty() {
                    !exclusive
                } else {
                    r.start_states().contains(&state_id)
                };
                if eligible {
                    local.push(ridx);
                    pats.push(format!("(?:{})", r.re_str()));
                }
            }
            // The syntax flags must match the ones `lrlex` compiles its rules
            // with (`RegexOptions` in the generated lexer): `.` spans newlines,
            // `^`/`$` are per-line, and `\0…` is octal. Leaving them at the
            // defaults changed which rule matched at comment ends and made 46
            // library files diverge.
            let dfa = DFA::builder()
                .configure(DFA::config().match_kind(MatchKind::All))
                .syntax(
                    regex_automata::util::syntax::Config::new()
                        .dot_matches_new_line(true)
                        .multi_line(true)
                        .octal(true),
                )
                .build_many(&pats)
                .expect("faustlexer.l rules must compile into a multi-pattern DFA");
            StateDfa { dfa, rules: local }
        })
        .collect();

    CombinedDfas {
        states,
        tok_ids,
        targets,
    }
}

/// Lexes with one automaton per start condition.
///
/// Mirrors `lrlex`'s loop exactly — see
/// `porting/lexer-combined-dfa-port-plan-2026-08-06-en.md` §3.2 — with the
/// per-rule scan replaced by a single anchored search. `MatchKind::All` gives
/// the longest match and, on a tie, the lowest pattern id; patterns are added
/// in rule order, so that is `lrlex`'s "earliest rule wins".
fn lex_stream_combined(input: &str) -> LexOutcome {
    use lrlex::StartStateOperation;
    use regex_automata::{Anchored, Input};

    let dfas = shared_combined_dfas();
    let mut caches: Vec<_> = dfas.states.iter().map(|s| s.dfa.create_cache()).collect();
    let mut lexemes: Vec<RawLexeme> = Vec::new();
    // (repeat count, state id), mirroring lrlex's counted stack.
    let mut stack: Vec<(usize, usize)> = vec![(1, 0)];
    let mut at = 0usize;

    while at < input.len() {
        let Some(&(_, state_id)) = stack.last() else {
            return LexOutcome::Failed {
                lexemes,
                error_at: at,
            };
        };
        let sd = &dfas.states[state_id];
        let probe = Input::new(input).span(at..input.len()).anchored(Anchored::Yes);
        // A zero-length match is not progress: `lrlex` requires `longest > 0`,
        // and accepting one here would loop forever. A cache error is a hard
        // failure, never a quiet fallback to a different match.
        let hit = match sd.dfa.try_search_fwd(&mut caches[state_id], &probe) {
            Ok(Some(h)) if h.offset() > at => h,
            _ => {
                return LexOutcome::Failed {
                    lexemes,
                    error_at: at,
                };
            }
        };
        let ridx = sd.rules[hit.pattern().as_usize()];
        let end = hit.offset();

        match dfas.tok_ids[ridx] {
            Some(tok_id) => lexemes.push(RawLexeme {
                tok_id,
                start: at,
                len: end - at,
            }),
            // Unnamed rules skip; a named rule with no token id is an error,
            // exactly as in `lrlex`.
            None if shared_lexerdef_rule_is_anonymous(ridx) => {}
            None => {
                return LexOutcome::Failed {
                    lexemes,
                    error_at: at,
                };
            }
        }

        if let Some((target, op)) = dfas.targets[ridx].as_ref() {
            let target = *target;
            if target >= dfas.states.len() {
                return LexOutcome::Failed {
                    lexemes,
                    error_at: at,
                };
            }
            match op {
                StartStateOperation::ReplaceStack => {
                    stack.clear();
                    stack.push((1, target));
                }
                StartStateOperation::Push => match stack.last_mut() {
                    Some((count, s)) if *s == target => *count += 1,
                    _ => stack.push((1, target)),
                },
                StartStateOperation::Pop => match stack.last_mut() {
                    Some((count, _)) if *count > 1 => *count -= 1,
                    Some(_) => {
                        stack.pop();
                        // `lrlex` refills with INITIAL rather than leaving the
                        // stack empty, so a `<-comment>` at depth one returns
                        // to INITIAL instead of failing on the next token.
                        // Only its loop shows this; the `.l` file does not.
                        if stack.is_empty() {
                            stack.push((1, 0));
                        }
                    }
                    None => {
                        return LexOutcome::Failed {
                            lexemes,
                            error_at: at,
                        };
                    }
                },
            }
        }
        at = end;
    }
    LexOutcome::Complete(lexemes)
}

/// Builds the `lrpar` lexer the parser consumes, using the combined DFA.
///
/// Only token *production* changes: spans, line/column and error recovery all
/// stay `lrlex`'s, because the result is handed back to `LRNonStreamingLexer`.
/// That keeps the surface the grammar sees identical by construction rather
/// than by reimplementation, and is why `lex_stream`'s differential over the
/// corpus is sufficient evidence for the whole change.
fn combined_lexer(input: &str) -> lrlex::LRNonStreamingLexer<'_, '_, DefaultLexerTypes<u32>> {
    use lrpar::Lexeme as _;
    let mut out: Vec<Result<lrlex::DefaultLexeme<u32>, lrlex::LRLexError>> = Vec::new();
    match lex_stream_combined(input) {
        LexOutcome::Complete(lexemes) => {
            out.extend(
                lexemes
                    .into_iter()
                    .map(|l| Ok(lrlex::DefaultLexeme::new(l.tok_id, l.start, l.len))),
            );
        }
        LexOutcome::Failed { lexemes, error_at } => {
            out.extend(
                lexemes
                    .into_iter()
                    .map(|l| Ok(lrlex::DefaultLexeme::new(l.tok_id, l.start, l.len))),
            );
            out.push(Err(lrlex::LRLexError::new(cfgrammar::Span::new(
                error_at, error_at,
            ))));
        }
    }
    lrlex::LRNonStreamingLexer::new(
        input,
        out,
        {
            let mut cache = cfgrammar::NewlineCache::new();
            cache.feed(input);
            cache
        },
    )
}

/// Whether rule `ridx` produces no lexeme (its `.l` action is `;`).
fn shared_lexerdef_rule_is_anonymous(ridx: usize) -> bool {
    use lrlex::LexerDef as _;
    shared_lexerdef()
        .iter_rules()
        .nth(ridx)
        .is_some_and(|r| r.name().is_none())
}

fn lex_stream_per_rule(input: &str) -> LexOutcome {
    let lexerdef = shared_lexerdef();
    let lexer = lexerdef.lexer(input);
    let mut lexemes = Vec::new();
    for item in lexer.iter() {
        match item {
            Ok(lexeme) => lexemes.push(RawLexeme {
                tok_id: lexeme.tok_id(),
                start: lexeme.span().start(),
                len: lexeme.span().len(),
            }),
            Err(err) => {
                return LexOutcome::Failed {
                    lexemes,
                    error_at: err.span().start(),
                };
            }
        }
    }
    LexOutcome::Complete(lexemes)
}

/// Lexes `input` and returns named tokens with source locations.
pub fn lex_tokens(input: &str) -> Result<Vec<LexedToken>, String> {
    let lexerdef = shared_lexerdef();
    let lexer = lexerdef.lexer(input);
    let mut out = Vec::new();
    for item in lexer.iter() {
        let lexeme = item.map_err(|err| format!("lex error at span {:?}", err.span()))?;
        let name = faustparser_y::token_epp(cfgrammar::TIdx(lexeme.tok_id())).unwrap_or("<anon>");
        let span = lexeme.span();
        let ((line, col), _) = lexer.line_col(span);
        out.push(LexedToken {
            name: name.to_owned().into_boxed_str(),
            text: lexer.span_str(span).into(),
            span,
            start_line: u32::try_from(line).unwrap_or(u32::MAX),
            start_col: u32::try_from(col).unwrap_or(u32::MAX),
        });
    }
    Ok(out)
}

/// Parses one input with Slice-1 grammar and returns parser state.
#[must_use]
/// Parses one Faust source string into a [`ParseOutput`].
pub fn parse_program(input: &str, source_file: &str) -> ParseOutput {
    parse_program_with_metadata(
        input,
        source_file,
        CompilationMetadataStore::new(source_file),
    )
}

/// Parses one in-memory source using the provided shared metadata store.
pub fn parse_program_with_metadata(
    input: &str,
    source_file: &str,
    metadata_store: CompilationMetadataStore,
) -> ParseOutput {
    parse_program_with_precision_and_metadata(input, source_file, 1, metadata_store)
}

/// Parses one in-memory source using the selected Faust precision variant.
///
/// `float_size` follows the C++ parser convention: `1=single`, `2=double`,
/// `3=quad`, `4=fixed`.  The value is applied while parsing so definitions
/// guarded by `singleprecision`/`doubleprecision` are selected correctly.
pub fn parse_program_with_precision_and_metadata(
    input: &str,
    source_file: &str,
    float_size: u8,
    metadata_store: CompilationMetadataStore,
) -> ParseOutput {
    parse_program_with_origins_and_precision(
        input,
        source_file,
        None,
        metadata_store,
        float_size,
        SourceKind::Memory,
    )
}

/// Parses one in-memory source and expands imports structurally from the parsed
/// definition tree against explicit search paths plus an optional in-memory
/// source bundle.
///
/// This is the source-string counterpart of
/// [`parse_file_with_imports_and_metadata`]. It is primarily used by embedded
/// compiler services such as `faustwasm`, where the root DSP comes from a
/// string but the standard library set may be embedded as logical assets
/// rather than files on disk.
pub fn parse_program_with_imports_and_metadata(
    input: &str,
    source_file: &str,
    search_paths: &[std::path::PathBuf],
    virtual_sources: &VirtualSourceMap,
    metadata_store: CompilationMetadataStore,
) -> Result<ParseOutput, SourceReaderError> {
    parse_program_with_imports_and_precision_and_metadata(
        input,
        source_file,
        search_paths,
        virtual_sources,
        metadata_store,
        1,
    )
}

/// Parses an in-memory source bundle using the selected Faust precision
/// variant while expanding imports structurally.
pub fn parse_program_with_imports_and_precision_and_metadata(
    input: &str,
    source_file: &str,
    search_paths: &[std::path::PathBuf],
    virtual_sources: &VirtualSourceMap,
    metadata_store: CompilationMetadataStore,
    float_size: u8,
) -> Result<ParseOutput, SourceReaderError> {
    let bundle = virtual_sources.with_source(PathBuf::from(source_file), input.to_owned());
    let reader = SourceReader::with_virtual_sources(search_paths.to_vec(), bundle);
    StructuralImportExpander::new(reader, metadata_store, float_size)
        .parse_entry(Path::new(source_file))
}

/// Reads a source file, parses each imported file as its own unit, then expands
/// import-file nodes structurally like the C++ compiler.
///
/// This convenience entry point creates a fresh top-level metadata store whose
/// master source is the canonicalized entry path. Imported files encountered
/// during structural expansion will therefore contribute scoped metadata
/// entries relative to that master.
pub fn parse_file_with_imports(
    path: &std::path::Path,
    search_paths: &[std::path::PathBuf],
) -> Result<ParseOutput, SourceReaderError> {
    parse_file_with_imports_and_metadata(
        path,
        search_paths,
        CompilationMetadataStore::new(
            &path
                .canonicalize()
                .unwrap_or_else(|_| path.to_path_buf())
                .to_string_lossy(),
        ),
    )
}

/// Reads and expands a file using the selected Faust precision variant.
pub fn parse_file_with_imports_and_precision(
    path: &std::path::Path,
    search_paths: &[std::path::PathBuf],
    float_size: u8,
) -> Result<ParseOutput, SourceReaderError> {
    parse_file_with_imports_and_precision_and_metadata(
        path,
        search_paths,
        CompilationMetadataStore::new(
            &path
                .canonicalize()
                .unwrap_or_else(|_| path.to_path_buf())
                .to_string_lossy(),
        ),
        float_size,
    )
}

/// Reads a source file, parses imported files as separate parse units, then
/// expands import-file nodes structurally using the provided shared top-level
/// metadata store.
///
/// This is the file-backed parser entry point used by later compilation stages
/// that need top-level metadata continuity across parse/eval boundaries. The
/// returned [`ParseOutput::used_files`] preserves the deterministic recursive
/// structural import visitation order.
pub fn parse_file_with_imports_and_metadata(
    path: &std::path::Path,
    search_paths: &[std::path::PathBuf],
    metadata_store: CompilationMetadataStore,
) -> Result<ParseOutput, SourceReaderError> {
    parse_file_with_imports_and_precision_and_metadata(path, search_paths, metadata_store, 1)
}

/// Reads and expands a file using the selected Faust precision variant.
pub fn parse_file_with_imports_and_precision_and_metadata(
    path: &std::path::Path,
    search_paths: &[std::path::PathBuf],
    metadata_store: CompilationMetadataStore,
    float_size: u8,
) -> Result<ParseOutput, SourceReaderError> {
    let reader = SourceReader::new(search_paths.to_vec());
    StructuralImportExpander::new(reader, metadata_store, float_size).parse_entry(path)
}

#[derive(Clone, Debug, Default)]
struct ParseRecoveryDetails {
    expected_tokens: Vec<Box<str>>,
    unexpected_token: Option<Box<str>>,
    unambiguous_insert: Option<Box<str>>,
    typo_suggestion: Option<Box<str>>,
    opening_delimiter: Option<(usize, char)>,
    previous_token: Option<(usize, usize)>,
}

#[derive(Clone, Debug)]
struct EngineParseDiagnostic {
    code: DiagnosticCode,
    message: String,
    location: SourceLocation,
    span: Span,
    recovery: ParseRecoveryDetails,
}

fn parse_recovery_details(
    error: &lrpar::LexParseError<u32, lrlex::DefaultLexerTypes<u32>>,
    input: &str,
) -> ParseRecoveryDetails {
    let lrpar::LexParseError::ParseError(error) = error else {
        return ParseRecoveryDetails::default();
    };
    let span = error.lexeme().span();
    let unexpected = input
        .get(span.start()..span.end())
        .filter(|token| !token.is_empty())
        .map(|token| token.into());
    let mut expected = Vec::<Box<str>>::new();
    let mut singleton_inserts = Vec::new();
    for sequence in error.repairs() {
        if let [lrpar::ParseRepair::Insert(token)] = sequence.as_slice()
            && let Some(name) = faustparser_y::token_epp(*token)
        {
            singleton_inserts.push(normalize_expected_token(name));
        }
        for repair in sequence {
            if let lrpar::ParseRepair::Insert(token) = repair
                && let Some(name) = faustparser_y::token_epp(*token)
            {
                let name = normalize_expected_token(name);
                if !expected.contains(&name) {
                    expected.push(name);
                }
            }
        }
    }
    expected.sort();
    let unambiguous_insert = singleton_inserts
        .first()
        .filter(|first| {
            singleton_inserts.len() == error.repairs().len()
                && singleton_inserts.iter().all(|item| item == *first)
        })
        .cloned();
    let typo_suggestion = unexpected.as_deref().and_then(|unexpected| {
        expected
            .iter()
            .filter(|candidate| {
                candidate
                    .chars()
                    .all(|ch| ch.is_ascii_alphabetic() || ch == '_')
                    && levenshtein_distance(unexpected, candidate) <= 2
            })
            .min_by_key(|candidate| levenshtein_distance(unexpected, candidate))
            .cloned()
    });
    let opening_delimiter = unambiguous_insert
        .as_deref()
        .and_then(closing_delimiter)
        .and_then(|opening| unmatched_opening_delimiter(input, span.start(), opening));
    ParseRecoveryDetails {
        expected_tokens: expected,
        unexpected_token: unexpected,
        unambiguous_insert,
        typo_suggestion,
        opening_delimiter,
        previous_token: previous_token_range(input, span.start()),
    }
}

fn normalize_expected_token(token: &str) -> Box<str> {
    let token = token.trim_matches(|ch| matches!(ch, '\'' | '"' | '`'));
    match token {
        "ENDDEF" => ";",
        "DEF" => "=",
        "LPAR" => "(",
        "RPAR" => ")",
        "LBRAQ" => "{",
        "RBRAQ" => "}",
        "LCROC" => "[",
        "RCROC" => "]",
        "PAR" => ",",
        "SEQ" => ":",
        "REC" => "~",
        "SPLIT" => "<:",
        "MIX" => ":>",
        "ADD" => "+",
        "SUB" => "-",
        "MUL" => "*",
        "DIV" => "/",
        "MOD" => "%",
        "FDELAY" => "@",
        "DELAY1" => "'",
        "AND" => "&",
        "OR" => "|",
        "LT" => "<",
        "LE" => "<=",
        "GT" => ">",
        "GE" => ">=",
        "EQ" => "==",
        "NE" => "!=",
        "LSH" => "<<",
        "RSH" => ">>",
        "ARROW" => "=>",
        "LAPPLY" => "->",
        "LAMBDA" => "\\",
        "POWOP" => "^",
        "DOT" => ".",
        "PROCESS" => "process",
        "WITH" => "with",
        "LETREC" => "letrec",
        "WHERE" => "where",
        "MEM" => "mem",
        "PREFIX" => "prefix",
        "INTCAST" => "int",
        "FLOATCAST" => "float",
        "NOTYPECAST" => "any",
        "RDTBL" => "rdtable",
        "RWTBL" => "rwtable",
        "SELECT2" => "select2",
        "SELECT3" => "select3",
        "FFUNCTION" => "ffunction",
        "FCONSTANT" => "fconstant",
        "FVARIABLE" => "fvariable",
        "BUTTON" => "button",
        "CHECKBOX" => "checkbox",
        "VSLIDER" => "vslider",
        "HSLIDER" => "hslider",
        "NENTRY" => "nentry",
        "VGROUP" => "vgroup",
        "HGROUP" => "hgroup",
        "TGROUP" => "tgroup",
        "VBARGRAPH" => "vbargraph",
        "HBARGRAPH" => "hbargraph",
        "SOUNDFILE" => "soundfile",
        "ATTACH" => "attach",
        "MODULATE" => "minput",
        "ACOS" => "acos",
        "ASIN" => "asin",
        "ATAN" => "atan",
        "ATAN2" => "atan2",
        "COS" => "cos",
        "SIN" => "sin",
        "TAN" => "tan",
        "EXP" => "exp",
        "LOG" => "log",
        "LOG10" => "log10",
        "POWFUN" => "pow",
        "SQRT" => "sqrt",
        "ABS" => "abs",
        "MIN" => "min",
        "MAX" => "max",
        "FMOD" => "fmod",
        "REMAINDER" => "remainder",
        "FLOOR" => "floor",
        "CEIL" => "ceil",
        "RINT" => "rint",
        "ROUND" => "round",
        "XOR" => "xor",
        "ISEQ" => "seq",
        "IPAR" => "par",
        "ISUM" => "sum",
        "IPROD" => "prod",
        "INPUTS" => "inputs",
        "OUTPUTS" => "outputs",
        "FAUTODIFF" => "fad",
        "RAUTODIFF" => "rad",
        "ONDEMAND" => "ondemand",
        "UPSAMPLING" => "upsampling",
        "DOWNSAMPLING" => "downsampling",
        "IMPORT" => "import",
        "COMPONENT" => "component",
        "LIBRARY" => "library",
        "ENVIRONMENT" => "environment",
        "WAVEFORM" => "waveform",
        "ROUTE" => "route",
        "ENABLE" => "enable",
        "CONTROL" => "control",
        "DECLARE" => "declare",
        "CASE" => "case",
        "ASSERTBOUNDS" => "assertbounds",
        "LOWEST" => "lowest",
        "HIGHEST" => "highest",
        "FLOATMODE" => "singleprecision",
        "DOUBLEMODE" => "doubleprecision",
        "QUADMODE" => "quadprecision",
        "FIXEDPOINTMODE" => "fixedpointprecision",
        _ => token,
    }
    .into()
}

fn build_engine_parse_diagnostics(
    errors: Vec<EngineParseDiagnostic>,
    source_id: SourceId,
    direct_source: bool,
    input: &str,
) -> Vec<Diagnostic> {
    let mut output: Vec<(Span, Diagnostic)> = Vec::new();
    for error in errors {
        let primary_span = SourceSpan::new(
            error.location.file(),
            error.location.line(),
            error.location.col(),
            error.location.end_line(),
            error.location.end_col(),
        );
        let mut diagnostic = Diagnostic::new(
            diagnostics::Severity::Error,
            Stage::Parser,
            error.code,
            error.message.clone(),
        )
        .with_category(diagnostics::DiagnosticCategory::UserCode)
        .with_detail_code("unexpected-token")
        .with_label(
            Label::new(
                LabelStyle::Primary,
                primary_span.clone(),
                "unexpected token",
            )
            .with_role(LabelRole::PrimaryCause),
        );
        if !error.recovery.expected_tokens.is_empty() {
            diagnostic = diagnostic.with_fact(
                "expected_tokens",
                error
                    .recovery
                    .expected_tokens
                    .iter()
                    .map(|token| token.to_string())
                    .collect::<Vec<_>>(),
            );
        }
        if let Some(unexpected) = &error.recovery.unexpected_token {
            diagnostic = diagnostic.with_fact("unexpected_token", unexpected.clone());
        }

        if direct_source
            && let Some(insert) = &error.recovery.unambiguous_insert
            && matches!(insert.as_ref(), ";" | ")" | "]" | "}")
            && let Ok(offset) = u32::try_from(error.span.start())
        {
            diagnostic = diagnostic.with_fix(SuggestedFix {
                title: format!("insert `{insert}`").into(),
                applicability: Applicability::MachineApplicable,
                edits: vec![TextEdit {
                    range: SourceRange::new(source_id, offset, offset),
                    replacement: insert.clone(),
                }],
                explanation: Some("the parser found only this insertion repair".into()),
            });
            if insert.as_ref() == ";"
                && let Some((start, end)) = error.recovery.previous_token
                && let (Ok(start), Ok(end)) = (u32::try_from(start), u32::try_from(end))
            {
                let span = source_span_for_offsets(&primary_span, input, start, end);
                diagnostic = diagnostic.with_label(
                    Label::new(
                        LabelStyle::Secondary,
                        span,
                        "the previous statement may need `;`",
                    )
                    .with_role(LabelRole::PreviousToken),
                );
            }
            if let Some((opening, delimiter)) = error.recovery.opening_delimiter
                && let (Ok(start), Ok(end)) = (
                    u32::try_from(opening),
                    u32::try_from(opening + delimiter.len_utf8()),
                )
            {
                let span = source_span_for_offsets(&primary_span, input, start, end);
                diagnostic = diagnostic.with_label(
                    Label::new(
                        LabelStyle::Secondary,
                        span,
                        format!("`{delimiter}` opened here"),
                    )
                    .with_role(LabelRole::MatchingDelimiter),
                );
            }
        } else if let Some(insert) = &error.recovery.unambiguous_insert {
            diagnostic = diagnostic.with_help(format!("insert `{insert}` before this token"));
        }

        if direct_source
            && let Some(suggestion) = &error.recovery.typo_suggestion
            && let (Ok(start), Ok(end)) = (
                u32::try_from(error.span.start()),
                u32::try_from(error.span.end()),
            )
        {
            diagnostic = diagnostic
                .with_fact("suggested_token", suggestion.clone())
                .with_fix(SuggestedFix {
                    title: format!("replace with `{suggestion}`").into(),
                    applicability: Applicability::MaybeIncorrect,
                    edits: vec![TextEdit {
                        range: SourceRange::new(source_id, start, end),
                        replacement: suggestion.clone(),
                    }],
                    explanation: Some("closest token accepted by the grammar at this point".into()),
                });
        }

        if let Some((previous_span, previous)) = output.last_mut()
            && *previous_span == error.span
        {
            *previous = previous.clone().with_related(RelatedDiagnostic {
                code: error.code,
                message: error.message.into(),
                labels: vec![
                    Label::new(
                        LabelStyle::Secondary,
                        primary_span,
                        "additional parser recovery",
                    )
                    .with_role(LabelRole::DerivedFrom),
                ],
            });
        } else {
            output.push((error.span, diagnostic));
        }
    }
    output
        .into_iter()
        .map(|(_, diagnostic)| diagnostic)
        .collect()
}

fn source_span_for_offsets(
    reference: &SourceSpan,
    input: &str,
    start: u32,
    end: u32,
) -> SourceSpan {
    let (line, col) = line_col_for_offset(input, start as usize);
    let (end_line, end_col) = line_col_for_offset(input, end as usize);
    SourceSpan::new(reference.file.clone(), line, col, end_line, end_col)
}

fn line_col_for_offset(input: &str, offset: usize) -> (u32, u32) {
    let prefix = input.get(..offset).unwrap_or(input);
    let line =
        u32::try_from(prefix.bytes().filter(|byte| *byte == b'\n').count() + 1).unwrap_or(u32::MAX);
    let col = u32::try_from(prefix.rsplit('\n').next().unwrap_or("").chars().count() + 1)
        .unwrap_or(u32::MAX);
    (line, col)
}

fn closing_delimiter(token: &str) -> Option<char> {
    match token {
        ")" => Some('('),
        "]" => Some('['),
        "}" => Some('{'),
        _ => None,
    }
}

fn unmatched_opening_delimiter(input: &str, end: usize, wanted: char) -> Option<(usize, char)> {
    let mut stack = Vec::new();
    for (offset, ch) in input.get(..end)?.char_indices() {
        match ch {
            '(' | '[' | '{' => stack.push((offset, ch)),
            ')' | ']' | '}' => {
                let expected = match ch {
                    ')' => '(',
                    ']' => '[',
                    '}' => '{',
                    _ => unreachable!(),
                };
                if stack
                    .last()
                    .is_some_and(|(_, opening)| *opening == expected)
                {
                    stack.pop();
                }
            }
            _ => {}
        }
    }
    stack
        .into_iter()
        .rev()
        .find(|(_, opening)| *opening == wanted)
}

fn previous_token_range(input: &str, end: usize) -> Option<(usize, usize)> {
    let prefix = input.get(..end)?;
    let last = prefix
        .char_indices()
        .rev()
        .find(|(_, ch)| !ch.is_whitespace())?;
    Some((last.0, last.0 + last.1.len_utf8()))
}

fn levenshtein_distance(lhs: &str, rhs: &str) -> usize {
    let mut previous = (0..=rhs.chars().count()).collect::<Vec<_>>();
    for (row, left) in lhs.chars().enumerate() {
        let mut current = vec![row + 1];
        for (column, right) in rhs.chars().enumerate() {
            current.push(
                (current[column] + 1)
                    .min(previous[column + 1] + 1)
                    .min(previous[column] + usize::from(left != right)),
            );
        }
        previous = current;
    }
    previous.last().copied().unwrap_or(0)
}

/// Parses one in-memory source while preserving external line origins.
fn parse_program_with_origins_and_precision(
    input: &str,
    source_file: &str,
    source_origins: Option<Vec<SourceLineOrigin>>,
    metadata_store: CompilationMetadataStore,
    float_size: u8,
    source_kind: SourceKind,
) -> ParseOutput {
    let direct_source = source_origins.is_none();
    let lexer = combined_lexer(input);
    let mut parse_state = ParseState::new_with_origins_and_metadata(
        source_file,
        input,
        source_origins,
        metadata_store,
    );
    parse_state.ctx.set_float_size(float_size);
    let state = RefCell::new(parse_state);
    let (root, errors) = faustparser_y::parse(&lexer, &state);
    let mut state = state.into_inner();

    let mut rendered_errors = Vec::with_capacity(errors.len());
    let mut engine_diagnostics = Vec::with_capacity(errors.len());
    for err in errors {
        let span = match &err {
            lrpar::LexParseError::LexError(e) => e.span(),
            lrpar::LexParseError::ParseError(e) => e.lexeme().span(),
        };
        let ((line, col), (end_line, end_col)) = state.span_line_col(span);
        if state.source_origins.is_none() {
            state.ctx.set_cursor_span(
                &state.source_file,
                u32::try_from(line).unwrap_or(u32::MAX),
                u32::try_from(col).unwrap_or(u32::MAX),
                u32::try_from(end_line).unwrap_or(u32::MAX),
                u32::try_from(end_col).unwrap_or(u32::MAX),
            );
        } else {
            let (file, mapped_line) = state.resolve_source_location(line);
            let (_, mapped_end_line) = state.resolve_source_location(end_line);
            let file_owned = file.to_string_lossy().into_owned();
            state.ctx.set_cursor_span(
                &file_owned,
                mapped_line,
                u32::try_from(col).unwrap_or(u32::MAX),
                mapped_end_line,
                u32::try_from(end_col).unwrap_or(u32::MAX),
            );
        }
        let message = err.pp(&lexer, &faustparser_y::token_epp).to_string();
        let recovery = parse_recovery_details(&err, input);
        engine_diagnostics.push(EngineParseDiagnostic {
            code: parser_code_for_lex_parse_error(&err),
            message: message.clone(),
            location: state.ctx.cursor().clone(),
            span,
            recovery,
        });
        state.ctx.note_engine_parse_error();
        rendered_errors.push(message);
    }

    let mut diagnostics = parser_ctx_to_bundle(&state.ctx);
    let mut sources = SourceMapBuilder::new();
    let source_id = sources.add(source_file, source_kind, input);
    let engine_diagnostics =
        build_engine_parse_diagnostics(engine_diagnostics, source_id, direct_source, input);
    diagnostics.extend(engine_diagnostics);
    diagnostics.set_source_map(sources.finish());

    ParseOutput {
        root,
        errors: rendered_errors,
        diagnostics,
        compilation_metadata: state.metadata_store.snapshot(),
        used_files: Vec::new(),
        state,
    }
}

/// Production structural import expander matching the C++ parser boundary.
///
/// Source provenance (C++):
/// - `compiler/parser/sourcereader.cpp`
/// - `SourceReader::getList(...)`
/// - `SourceReader::expandList(...)`
/// - `SourceReader::expandRec(...)`
///
/// Mapping status: `adapted`.
///
/// C++ parses each file into a definition list containing explicit
/// `importFile(...)` nodes, then expands those nodes structurally. Rust uses
/// the same semantic boundary here while keeping a Rust-native `ParseOutput`
/// transport and `TreeArena::clone_subtree_from(...)` to move imported
/// definitions across per-file arenas.
#[derive(Debug)]
struct StructuralImportExpander {
    reader: SourceReader,
    metadata_store: CompilationMetadataStore,
    used_files: Vec<PathBuf>,
    active_stack: HashSet<PathBuf>,
    active_paths: Vec<PathBuf>,
    import_edges: Vec<ImportCycleEdge>,
    source_map: SourceMapBuilder,
    float_size: u8,
}

struct ImportExpansionReports<'a> {
    errors: &'a mut Vec<String>,
    diagnostics: &'a mut DiagnosticBundle,
}

impl StructuralImportExpander {
    fn new(reader: SourceReader, metadata_store: CompilationMetadataStore, float_size: u8) -> Self {
        Self {
            reader,
            metadata_store,
            used_files: Vec::new(),
            active_stack: HashSet::new(),
            active_paths: Vec::new(),
            import_edges: Vec::new(),
            source_map: SourceMapBuilder::new(),
            float_size,
        }
    }

    fn parse_entry(self, entry: &Path) -> Result<ParseOutput, SourceReaderError> {
        let resolved = self.reader.resolve_entry_source_path(entry)?;
        self.parse_resolved_entry(&resolved)
    }

    fn parse_resolved_entry(mut self, resolved: &Path) -> Result<ParseOutput, SourceReaderError> {
        self.note_visit(resolved);
        self.active_stack.insert(resolved.to_path_buf());
        self.active_paths.push(resolved.to_path_buf());

        let source = self.reader.read_source_unit(resolved)?;
        let source_name = resolved.to_string_lossy().into_owned();
        let source_kind = if self.reader.is_virtual_source(resolved) {
            SourceKind::Memory
        } else {
            SourceKind::File
        };
        self.source_map
            .add(resolved.to_path_buf(), source_kind, source.as_str());
        let mut output = parse_program_with_origins_and_precision(
            &source,
            &source_name,
            None,
            self.metadata_store.clone(),
            self.float_size,
            source_kind,
        );
        let mut expanded_in_scope = HashSet::new();
        self.expand_imports_in_output(&mut output, resolved, &mut expanded_in_scope)?;
        output.used_files = self.used_files;
        output.compilation_metadata = self.metadata_store.snapshot();
        self.active_stack.remove(resolved);
        self.active_paths.pop();
        output
            .diagnostics
            .set_source_map(std::mem::take(&mut self.source_map).finish());
        Ok(output)
    }

    fn expand_imports_in_output(
        &mut self,
        output: &mut ParseOutput,
        current_file: &Path,
        expanded_in_scope: &mut HashSet<PathBuf>,
    ) -> Result<(), SourceReaderError> {
        let Some(root) = output.root else {
            return Ok(());
        };
        let mut reports = ImportExpansionReports {
            errors: &mut output.errors,
            diagnostics: &mut output.diagnostics,
        };
        let expanded = self.expand_definition_list_in_arena(
            &mut output.state.arena,
            &mut output.state.ctx,
            root,
            current_file,
            expanded_in_scope,
            &mut reports,
        )?;
        output.root = Some(expanded);
        output.state.ctx.set_parse_result(expanded);
        Ok(())
    }

    fn expand_definition_list_in_arena(
        &mut self,
        arena: &mut TreeArena,
        ctx: &mut ParserCtx,
        mut defs: TreeId,
        current_file: &Path,
        expanded_in_scope: &mut HashSet<PathBuf>,
        reports: &mut ImportExpansionReports<'_>,
    ) -> Result<TreeId, SourceReaderError> {
        let mut items = Vec::new();

        while !arena.is_nil(defs) {
            let Some(def) = arena.hd(defs) else {
                break;
            };
            match match_box(arena, def) {
                BoxMatch::ImportFile(filename) => {
                    if let Some(import_name) = string_node_text_from_arena(arena, filename) {
                        let Some(resolved_import) = self
                            .reader
                            .resolve_import_source_path(import_name, current_file.parent())
                        else {
                            // Box nodes carry no source location, so recover the
                            // directive's span by re-scanning the file. Error
                            // path only.
                            let site = std::fs::read_to_string(current_file)
                                .ok()
                                .and_then(|text| ImportSite::locate_in(&text, import_name));
                            let mut searched: Vec<PathBuf> = Vec::new();
                            if let Some(dir) = current_file.parent() {
                                searched.push(dir.to_path_buf());
                            }
                            searched.extend(self.reader.search_paths().iter().cloned());
                            return Err(SourceReaderError::UnresolvedImport {
                                name: import_name.into(),
                                from: current_file.to_path_buf(),
                                site,
                                searched,
                            });
                        };

                        let site = self
                            .reader
                            .read_source_unit(current_file)
                            .ok()
                            .and_then(|text| ImportSite::locate_in(&text, import_name));
                        let import_edge = ImportCycleEdge {
                            from: current_file.to_path_buf(),
                            to: resolved_import.clone(),
                            site,
                        };

                        if self.active_stack.contains(&resolved_import) {
                            return Err(SourceReaderError::ImportCycle {
                                path: resolved_import.clone(),
                                cycle: source_reader::import_cycle_from_stack(
                                    &self.active_paths,
                                    &self.import_edges,
                                    &resolved_import,
                                    Some(import_edge),
                                ),
                            });
                        }

                        if expanded_in_scope.insert(resolved_import.clone()) {
                            self.note_visit(&resolved_import);
                            self.active_stack.insert(resolved_import.clone());
                            self.active_paths.push(resolved_import.clone());
                            self.import_edges.push(import_edge);
                            let imported = (|| {
                                let mut imported =
                                    self.parse_single_source_file(&resolved_import)?;
                                self.expand_imports_in_output(
                                    &mut imported,
                                    &resolved_import,
                                    expanded_in_scope,
                                )?;
                                Ok(imported)
                            })();
                            self.import_edges.pop();
                            self.active_paths.pop();
                            self.active_stack.remove(&resolved_import);
                            let imported: ParseOutput = imported?;
                            reports.errors.extend(imported.errors.iter().cloned());
                            reports
                                .diagnostics
                                .extend(imported.diagnostics.as_slice().iter().cloned());
                            if let Some(imported_root) = imported.root {
                                let mut imported_defs = imported_root;
                                let mut imported_node_map = std::collections::HashMap::new();
                                while !imported.state.arena.is_nil(imported_defs) {
                                    let Some(imported_def) = imported.state.arena.hd(imported_defs)
                                    else {
                                        break;
                                    };
                                    let cloned = arena
                                        .clone_subtree_from(&imported.state.arena, imported_def);
                                    map_cloned_subtree_nodes(
                                        arena,
                                        cloned,
                                        &imported.state.arena,
                                        imported_def,
                                        &mut imported_node_map,
                                    );
                                    items.push(cloned);
                                    imported_defs = imported
                                        .state
                                        .arena
                                        .tl(imported_defs)
                                        .unwrap_or_else(|| imported.state.arena.nil());
                                }
                                ctx.import_box_provenance(&imported.state.ctx, &imported_node_map);
                            }
                        }
                    }
                }
                _ => items.push(self.rewrite_nested_imports(
                    arena,
                    ctx,
                    def,
                    current_file,
                    reports,
                )?),
            }

            defs = arena.tl(defs).unwrap_or_else(|| arena.nil());
        }

        let mut out = arena.nil();
        for item in items.iter().rev() {
            out = arena.cons(*item, out);
        }
        Ok(out)
    }

    fn rewrite_nested_imports(
        &mut self,
        arena: &mut TreeArena,
        ctx: &mut ParserCtx,
        id: TreeId,
        current_file: &Path,
        reports: &mut ImportExpansionReports<'_>,
    ) -> Result<TreeId, SourceReaderError> {
        match match_box(arena, id) {
            BoxMatch::WithLocalDef(body, defs) => {
                let body = self.rewrite_nested_imports(arena, ctx, body, current_file, reports)?;
                // Nested local-definition lists need their own duplicate-import
                // suppression scope. A library imported into one local
                // environment must not suppress the same library when it is
                // later imported into the surrounding top-level scope.
                let mut local_expanded = HashSet::new();
                let defs = self.expand_definition_list_in_arena(
                    arena,
                    ctx,
                    defs,
                    current_file,
                    &mut local_expanded,
                    reports,
                )?;
                Ok(boxes::BoxBuilder::new(arena).with_local_def(body, defs))
            }
            BoxMatch::ModifLocalDef(body, defs) => {
                let body = self.rewrite_nested_imports(arena, ctx, body, current_file, reports)?;
                let mut local_expanded = HashSet::new();
                let defs = self.expand_definition_list_in_arena(
                    arena,
                    ctx,
                    defs,
                    current_file,
                    &mut local_expanded,
                    reports,
                )?;
                Ok(boxes::BoxBuilder::new(arena).modif_local_def(body, defs))
            }
            BoxMatch::WithRecDef(body, defs1, defs2) => {
                let body = self.rewrite_nested_imports(arena, ctx, body, current_file, reports)?;
                let mut local_expanded_1 = HashSet::new();
                let defs1 = self.expand_definition_list_in_arena(
                    arena,
                    ctx,
                    defs1,
                    current_file,
                    &mut local_expanded_1,
                    reports,
                )?;
                let mut local_expanded_2 = HashSet::new();
                let defs2 = self.expand_definition_list_in_arena(
                    arena,
                    ctx,
                    defs2,
                    current_file,
                    &mut local_expanded_2,
                    reports,
                )?;
                Ok(boxes::BoxBuilder::new(arena).with_rec_def(body, defs1, defs2))
            }
            _ => {
                let Some(node) = arena.node(id).cloned() else {
                    return Ok(id);
                };
                if node.children.is_empty() {
                    return Ok(id);
                }

                let mut rewritten = Vec::with_capacity(node.children.len());
                let mut changed = false;
                for child in node.children.as_slice() {
                    let rewritten_child =
                        self.rewrite_nested_imports(arena, ctx, *child, current_file, reports)?;
                    changed |= rewritten_child != *child;
                    rewritten.push(rewritten_child);
                }
                if !changed {
                    return Ok(id);
                }

                let new_kind = match node.kind {
                    NodeKind::Tag(tag_id) => {
                        let tag_name = arena
                            .tag_name(tag_id)
                            .expect("rewritten tag id should resolve")
                            .to_owned();
                        NodeKind::Tag(arena.intern_tag(&tag_name))
                    }
                    other => other,
                };
                Ok(arena.intern(new_kind, &rewritten))
            }
        }
    }

    fn parse_single_source_file(
        &mut self,
        resolved: &Path,
    ) -> Result<ParseOutput, SourceReaderError> {
        let source = self.reader.read_source_unit(resolved)?;
        let source_name = resolved.to_string_lossy().into_owned();
        let source_kind = if self.reader.is_virtual_source(resolved) {
            SourceKind::VirtualLibrary
        } else {
            SourceKind::ImportedFile
        };
        self.source_map
            .add(resolved.to_path_buf(), source_kind, source.as_str());
        Ok(parse_program_with_origins_and_precision(
            &source,
            &source_name,
            None,
            self.metadata_store.clone(),
            self.float_size,
            source_kind,
        ))
    }

    fn note_visit(&mut self, path: &Path) {
        let path = path.to_path_buf();
        if !self.used_files.iter().any(|existing| existing == &path) {
            self.used_files.push(path);
        }
    }
}

fn string_node_text_from_arena(arena: &TreeArena, node: TreeId) -> Option<&str> {
    match arena.kind(node) {
        Some(NodeKind::StringLiteral(value)) => Some(value.as_ref()),
        Some(NodeKind::Symbol(value)) => Some(value.as_ref()),
        _ => None,
    }
}

/// Reconstructs the source-to-destination node mapping for one subtree cloned
/// with `TreeArena::clone_subtree_from`.
///
/// Both trees preserve ordered child structure. Destination hash-consing may
/// map several source ids to one destination id, which is intentional: the
/// occurrence provenance copied through this map retains their distinct
/// source locations.
fn map_cloned_subtree_nodes(
    destination: &TreeArena,
    destination_root: TreeId,
    source: &TreeArena,
    source_root: TreeId,
    mapping: &mut std::collections::HashMap<TreeId, TreeId>,
) {
    let mut stack = vec![(source_root, destination_root)];
    while let Some((source_node, destination_node)) = stack.pop() {
        if mapping.insert(source_node, destination_node).is_some() {
            continue;
        }
        let source_children = source.children(source_node).unwrap_or(&[]);
        let destination_children = destination.children(destination_node).unwrap_or(&[]);
        for (&source_child, &destination_child) in source_children.iter().zip(destination_children)
        {
            stack.push((source_child, destination_child));
        }
    }
}

/// Parses the minimal prototype sentence `process = _;`.
#[must_use]
/// Minimal parser smoke-check used by tests and tooling.
pub fn parse_minimal(input: &str) -> bool {
    let output = parse_program(input, "<memory>");
    output.root.is_some() && output.errors.is_empty()
}

/// Updates parser cursor from one lexed token, then tags `sym` as use-site at that location.
pub fn set_use_prop_from_token(ctx: &mut ParserCtx, sym: TreeId, file: &str, token: &LexedToken) {
    ctx.set_cursor_with_col(file, token.start_line, token.start_col);
    ctx.set_use_prop_at_cursor(sym);
}

/// Converts parser-local diagnostics to the shared workspace diagnostic model.
fn parser_ctx_to_bundle(ctx: &ParserCtx) -> DiagnosticBundle {
    let diagnostics = ctx
        .diagnostics()
        .iter()
        .map(|diag| {
            let mut out = Diagnostic::new(
                diag.severity,
                Stage::Parser,
                diag.code,
                diag.message.clone(),
            );
            if let Some(location) = &diag.location {
                let span = SourceSpan::new(
                    location.file(),
                    location.line(),
                    location.col(),
                    location.end_line(),
                    location.end_col(),
                );
                out = out.with_label(Label::new(
                    LabelStyle::Primary,
                    span,
                    diag.primary_message.clone(),
                ));
            }
            for site in &diag.related_sites {
                let span = SourceSpan::new(
                    site.location.file(),
                    site.location.line(),
                    site.location.col(),
                    site.location.end_line(),
                    site.location.end_col(),
                );
                out = out.with_label(
                    Label::new(LabelStyle::Secondary, span, site.message.clone())
                        .with_role(site.role),
                );
            }
            if let Some(detail_code) = &diag.detail_code {
                out = out.with_detail_code(detail_code.clone());
            }
            for (key, value) in &diag.facts {
                out = out.with_fact(key.clone(), value.clone());
            }
            for note in &diag.notes {
                out = out.with_note(note.clone());
            }
            for help in &diag.help {
                out = out.with_help(help.clone());
            }
            out
        })
        .collect::<Vec<_>>();
    DiagnosticBundle::from(diagnostics)
}

/// Maps lexer/parser engine errors to stable diagnostic codes.
fn parser_code_for_lex_parse_error(
    err: &lrpar::LexParseError<u32, lrlex::DefaultLexerTypes<u32>>,
) -> DiagnosticCode {
    match err {
        lrpar::LexParseError::LexError(_) => codes::LEX_INVALID_TOKEN,
        lrpar::LexParseError::ParseError(_) => codes::PARSE_UNEXPECTED_TOKEN,
    }
}
