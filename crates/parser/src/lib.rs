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
use errors::codes;
use errors::{
    Diagnostic, DiagnosticBundle, DiagnosticCode, Label, LabelStyle, Severity, SourceSpan, Stage,
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

pub use context::{DiagnosticSeverity, ParserCtx, ParserDiagnostic, SourceLocation};
pub use metadata::{CompilationMetadataKey, CompilationMetadataSnapshot, CompilationMetadataStore};
pub use source_reader::{
    ExpandedSource, SourceLineOrigin, SourceReader, SourceReaderError, VirtualSourceMap,
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
    /// `variants_rev` stores payloads in parser-list reverse order. Iterate it
    /// in reverse when printing so diagnostics follow source order, matching
    /// the C++ message shape:
    ///
    /// ```text
    /// multiple definitions of symbol 'foo'
    /// foo = ...;
    /// foo = ...;
    /// ```
    ///
    /// `dump_box` is used for the expression snippets because this parser layer
    /// only has normalized box nodes at this point; exact pretty-print parity is
    /// less important than preserving the semantic error boundary.
    fn report_zero_arity_redefinition(&mut self, name: TreeId, variants_rev: &[TreeId]) {
        let name_text = self
            .definition_name_key(name)
            .unwrap_or_else(|| "<invalid>".to_owned());
        let mut message = format!("multiple definitions of symbol '{name_text}'");
        for payload in variants_rev.iter().rev() {
            let Some((args, body)) = self.definition_payload_parts(*payload) else {
                continue;
            };
            if self.arena.is_nil(args) {
                message.push_str(&format!("\n{name_text} = {};", dump_box(&self.arena, body)));
            } else {
                message.push_str(&format!(
                    "\n{name_text}{} = {};",
                    dump_box(&self.arena, args),
                    dump_box(&self.arena, body)
                ));
            }
        }
        self.ctx.error(&message);
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
            self.ctx.error("invalid INT literal");
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
                self.ctx.error("invalid FLOAT literal");
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
                self.ctx.error("invalid import path literal");
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
            self.ctx.error("invalid signed INT literal");
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
                self.ctx.error("invalid signed FLOAT literal");
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
#[must_use]
pub fn lexerdef() -> LRNonStreamingLexerDef<DefaultLexerTypes<u32>> {
    faustlexer_l::lexerdef()
}

/// Lexes `input` and returns named tokens with source locations.
pub fn lex_tokens(input: &str) -> Result<Vec<LexedToken>, String> {
    let lexerdef = lexerdef();
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
    parse_program_with_origins_and_precision(input, source_file, None, metadata_store, float_size)
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
/// Parses one in-memory source while preserving external line origins.
fn parse_program_with_origins_and_precision(
    input: &str,
    source_file: &str,
    source_origins: Option<Vec<SourceLineOrigin>>,
    metadata_store: CompilationMetadataStore,
    float_size: u8,
) -> ParseOutput {
    let lexerdef = lexerdef();
    let lexer = lexerdef.lexer(input);
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
        state
            .ctx
            .error_with_code(parser_code_for_lex_parse_error(&err), &message);
        rendered_errors.push(message);
    }

    let diagnostics = parser_ctx_to_bundle(&state.ctx);

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
    float_size: u8,
}

impl StructuralImportExpander {
    fn new(reader: SourceReader, metadata_store: CompilationMetadataStore, float_size: u8) -> Self {
        Self {
            reader,
            metadata_store,
            used_files: Vec::new(),
            active_stack: HashSet::new(),
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

        let source = self.reader.read_source_unit(resolved)?;
        let source_name = resolved.to_string_lossy().into_owned();
        let mut output = parse_program_with_origins_and_precision(
            &source,
            &source_name,
            None,
            self.metadata_store.clone(),
            self.float_size,
        );
        let mut expanded_in_scope = HashSet::new();
        self.expand_imports_in_output(&mut output, resolved, &mut expanded_in_scope)?;
        output.used_files = self.used_files;
        output.compilation_metadata = self.metadata_store.snapshot();
        self.active_stack.remove(resolved);
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
        let expanded = self.expand_definition_list_in_arena(
            &mut output.state.arena,
            root,
            current_file,
            expanded_in_scope,
            &mut output.errors,
            &mut output.diagnostics,
        )?;
        output.root = Some(expanded);
        output.state.ctx.set_parse_result(expanded);
        Ok(())
    }

    fn expand_definition_list_in_arena(
        &mut self,
        arena: &mut TreeArena,
        mut defs: TreeId,
        current_file: &Path,
        expanded_in_scope: &mut HashSet<PathBuf>,
        errors: &mut Vec<String>,
        diagnostics: &mut DiagnosticBundle,
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
                            return Err(SourceReaderError::UnresolvedImport {
                                name: import_name.into(),
                                from: current_file.to_path_buf(),
                            });
                        };

                        if self.active_stack.contains(&resolved_import) {
                            return Err(SourceReaderError::ImportCycle {
                                path: resolved_import,
                            });
                        }

                        if expanded_in_scope.insert(resolved_import.clone()) {
                            self.note_visit(&resolved_import);
                            self.active_stack.insert(resolved_import.clone());
                            let mut imported = self.parse_single_source_file(&resolved_import)?;
                            self.expand_imports_in_output(
                                &mut imported,
                                &resolved_import,
                                expanded_in_scope,
                            )?;
                            errors.extend(imported.errors.iter().cloned());
                            diagnostics.extend(imported.diagnostics.as_slice().iter().cloned());
                            if let Some(imported_root) = imported.root {
                                let mut imported_defs = imported_root;
                                while !imported.state.arena.is_nil(imported_defs) {
                                    let Some(imported_def) = imported.state.arena.hd(imported_defs)
                                    else {
                                        break;
                                    };
                                    items.push(
                                        arena.clone_subtree_from(
                                            &imported.state.arena,
                                            imported_def,
                                        ),
                                    );
                                    imported_defs = imported
                                        .state
                                        .arena
                                        .tl(imported_defs)
                                        .unwrap_or_else(|| imported.state.arena.nil());
                                }
                            }
                            self.active_stack.remove(&resolved_import);
                        }
                    }
                }
                _ => items.push(self.rewrite_nested_imports(
                    arena,
                    def,
                    current_file,
                    errors,
                    diagnostics,
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
        id: TreeId,
        current_file: &Path,
        errors: &mut Vec<String>,
        diagnostics: &mut DiagnosticBundle,
    ) -> Result<TreeId, SourceReaderError> {
        match match_box(arena, id) {
            BoxMatch::WithLocalDef(body, defs) => {
                let body =
                    self.rewrite_nested_imports(arena, body, current_file, errors, diagnostics)?;
                // Nested local-definition lists need their own duplicate-import
                // suppression scope. A library imported into one local
                // environment must not suppress the same library when it is
                // later imported into the surrounding top-level scope.
                let mut local_expanded = HashSet::new();
                let defs = self.expand_definition_list_in_arena(
                    arena,
                    defs,
                    current_file,
                    &mut local_expanded,
                    errors,
                    diagnostics,
                )?;
                Ok(boxes::BoxBuilder::new(arena).with_local_def(body, defs))
            }
            BoxMatch::ModifLocalDef(body, defs) => {
                let body =
                    self.rewrite_nested_imports(arena, body, current_file, errors, diagnostics)?;
                let mut local_expanded = HashSet::new();
                let defs = self.expand_definition_list_in_arena(
                    arena,
                    defs,
                    current_file,
                    &mut local_expanded,
                    errors,
                    diagnostics,
                )?;
                Ok(boxes::BoxBuilder::new(arena).modif_local_def(body, defs))
            }
            BoxMatch::WithRecDef(body, defs1, defs2) => {
                let body =
                    self.rewrite_nested_imports(arena, body, current_file, errors, diagnostics)?;
                let mut local_expanded_1 = HashSet::new();
                let defs1 = self.expand_definition_list_in_arena(
                    arena,
                    defs1,
                    current_file,
                    &mut local_expanded_1,
                    errors,
                    diagnostics,
                )?;
                let mut local_expanded_2 = HashSet::new();
                let defs2 = self.expand_definition_list_in_arena(
                    arena,
                    defs2,
                    current_file,
                    &mut local_expanded_2,
                    errors,
                    diagnostics,
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
                    let rewritten_child = self.rewrite_nested_imports(
                        arena,
                        *child,
                        current_file,
                        errors,
                        diagnostics,
                    )?;
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

    fn parse_single_source_file(&self, resolved: &Path) -> Result<ParseOutput, SourceReaderError> {
        let source = self.reader.read_source_unit(resolved)?;
        let source_name = resolved.to_string_lossy().into_owned();
        Ok(parse_program_with_origins_and_precision(
            &source,
            &source_name,
            None,
            self.metadata_store.clone(),
            self.float_size,
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
            let severity = match diag.severity {
                DiagnosticSeverity::Error => Severity::Error,
                DiagnosticSeverity::Warning => Severity::Warning,
                DiagnosticSeverity::Remark => Severity::Remark,
            };
            let code = diag
                .code
                .unwrap_or_else(|| parser_code_for_message(diag.message.as_ref(), diag.severity));
            let mut out = Diagnostic::new(severity, Stage::Parser, code, diag.message.clone());
            if let Some(location) = &diag.location {
                let span = SourceSpan::new(
                    location.file(),
                    location.line(),
                    location.col(),
                    location.end_line(),
                    location.end_col(),
                );
                out = out.with_label(Label::new(LabelStyle::Primary, span, "parser location"));
            }
            out
        })
        .collect::<Vec<_>>();
    DiagnosticBundle::from(diagnostics)
}

/// Chooses a stable parser diagnostic code from one rendered parser message.
fn parser_code_for_message(message: &str, severity: DiagnosticSeverity) -> DiagnosticCode {
    if matches!(
        severity,
        DiagnosticSeverity::Warning | DiagnosticSeverity::Remark
    ) {
        codes::PARSE_RECOVERY
    } else if message.contains("invalid") && message.contains("literal") {
        codes::PARSE_INVALID_LITERAL
    } else {
        codes::PARSE_UNEXPECTED_TOKEN
    }
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
