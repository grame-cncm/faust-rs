%start Program
%parse-param state: &std::cell::RefCell<crate::ParseState>

%left WITH
%left LETREC
%right SPLIT MIX
%right SEQ
%right PAR
%left REC
%left LT LE EQ GT GE NE
%left ADD SUB OR
%left MUL DIV MOD AND XOR LSH RSH
%left POWOP
%left FDELAY
%left DELAY1
%left DOT
%left LPAR
%left LCROC

%token PROCESS
%token INT FLOAT IDENT STRING FSTRING EXTRA
%token SEQ PAR SPLIT MIX REC
%token ADD SUB MUL DIV MOD FDELAY DELAY1
%token AND OR XOR LSH RSH LT LE GT GE EQ NE
%token WIRE CUT ENDDEF DEF LPAR RPAR LBRAQ RBRAQ LCROC RCROC DOT
%token WITH LETREC WHERE
%token MEM PREFIX
%token INTCAST FLOATCAST NOTYPECAST
%token RDTBL RWTBL SELECT2 SELECT3
%token BUTTON CHECKBOX VSLIDER HSLIDER NENTRY VGROUP HGROUP TGROUP VBARGRAPH HBARGRAPH SOUNDFILE
%token ATTACH MODULATE
%token ACOS ASIN ATAN ATAN2 COS SIN TAN
%token EXP LOG LOG10 POWOP POWFUN SQRT
%token ABS MIN MAX
%token FMOD REMAINDER
%token FLOOR CEIL RINT ROUND
%token IPAR ISEQ ISUM IPROD
%token INPUTS OUTPUTS FAUTODIFF RAUTODIFF ONDEMAND UPSAMPLING DOWNSAMPLING
%token IMPORT COMPONENT LIBRARY ENVIRONMENT WAVEFORM ROUTE ENABLE CONTROL
%token DECLARE CASE ARROW LAPPLY
%token ASSERTBOUNDS LOWEST HIGHEST
%token FLOATMODE DOUBLEMODE QUADMODE FIXEDPOINTMODE
%token LAMBDA
%token FFUNCTION FCONSTANT FVARIABLE
%token BDOC EDOC BEQN EEQN BDGM EDGM BLST ELST BMETADATA EMETADATA
%token DOCCHAR NOTICE
%token LSTTRUE LSTFALSE LSTDEPENDENCIES LSTMDOCTAGS LSTDISTRIBUTED LSTEQ LSTQ
%%
Program -> tlib::TreeId:
      StmtList {
          crate::with_state(state, |state| {
              let root = state.format_definitions($1);
              state.ctx.set_parse_result(root);
              root
          })
      }
    ;

StmtList -> tlib::TreeId:
      %empty {
          crate::with_state(state, |state| state.nil())
      }
    | StmtList VariantList Statement {
          crate::with_state(state, |state| state.prepend_statement_with_variant($1, $2, $3))
      }
    ;

DefList -> tlib::TreeId:
      %empty {
          crate::with_state(state, |state| state.nil())
      }
    | DefList VariantList Definition {
          crate::with_state(state, |state| state.prepend_statement_with_variant($1, $2, $3))
      }
    ;

VariantList -> u8:
      %empty { 0 }
    | VariantList Variant { $1 | $2 }
    ;

Variant -> u8:
      FLOATMODE { 1 }
    | DOUBLEMODE { 2 }
    | QUADMODE { 4 }
    | FIXEDPOINTMODE { 8 }
    ;

RecList -> tlib::TreeId:
      %empty {
          crate::with_state(state, |state| state.nil())
      }
    | RecList RecDefinition {
          crate::with_state(state, |state| state.prepend_statement($1, $2))
      }
    ;

Statement -> tlib::TreeId:
      Definition { $1 }
    | IMPORT LPAR UQString RPAR ENDDEF {
          crate::with_state(state, |state| state.import_statement($3))
      }
    | DECLARE IDENT UQString ENDDEF {
          crate::with_state(state, |state| {
              state.declare_metadata_from_token($lexer, $2, $3)
          })
      }
    | DECLARE IDENT IDENT UQString ENDDEF {
          crate::with_state(state, |state| {
              state.declare_definition_metadata_from_tokens($lexer, $2, $3, $4)
          })
      }
    | BDOC DocContent EDOC {
          crate::with_state(state, |state| state.doc_statement())
      }
    ;

DocContent -> u8:
      %empty { 0 }
    | DocContent DocElem { 0 }
    ;

DocElem -> u8:
      DOCCHAR {
          crate::with_state(state, |state| {
              state.note_doc_char();
              0
          })
      }
    | BEQN Expression EEQN { 0 }
    | BDGM Expression EDGM { 0 }
    | NOTICE {
          crate::with_state(state, |state| {
              state.note_doc_notice();
              0
          })
      }
    | BLST LstAttrList ELST {
          crate::with_state(state, |state| {
              state.note_doc_listing();
              0
          })
      }
    | BMETADATA IDENT EMETADATA {
          crate::with_state(state, |state| {
              state.note_doc_metadata_tag_from_token($lexer, $2);
              0
          })
      }
    ;

LstAttrList -> u8:
      %empty { 0 }
    | LstAttrList LstAttr { 0 }
    ;

LstAttr -> u8:
      LSTDEPENDENCIES LSTEQ LSTQ LstAttrValue LSTQ {
          crate::with_state(state, |state| {
              state.set_lst_dependencies($4);
              0
          })
      }
    | LSTMDOCTAGS LSTEQ LSTQ LstAttrValue LSTQ {
          crate::with_state(state, |state| {
              state.set_lst_mdoctags($4);
              0
          })
      }
    | LSTDISTRIBUTED LSTEQ LSTQ LstAttrValue LSTQ {
          crate::with_state(state, |state| {
              state.set_lst_distributed($4);
              0
          })
      }
    ;

LstAttrValue -> bool:
      LSTTRUE { true }
    | LSTFALSE { false }
    ;

Definition -> tlib::TreeId:
      DefName DEF Expression ENDDEF {
          crate::with_state(state, |state| {
              state.mark_def_at_cursor($1);
              let nil = state.nil();
              state.make_definition($1, nil, $3)
          })
      }
    | DefName LPAR ArgList RPAR DEF Expression ENDDEF {
          crate::with_state(state, |state| {
              state.mark_def_at_cursor($1);
              state.make_definition($1, $3, $6)
          })
      }
    | DefName DEF ENDDEF {
          crate::with_state(state, |state| {
              state.recovery_statement("syntax error: empty definition body before ';'")
          })
      }
    | DefName DEF EXTRA ENDDEF {
          crate::with_state(state, |state| {
              state.recovery_statement("syntax error: invalid definition token before ';'")
          })
      }
    ;

RecDefinition -> tlib::TreeId:
      RecName DEF Expression ENDDEF {
          crate::with_state(state, |state| {
              state.mark_def_at_cursor($1);
              let nil = state.nil();
              state.make_definition($1, nil, $3)
          })
      }
    | RecName DEF ENDDEF {
          crate::with_state(state, |state| {
              state.recovery_statement("syntax error: empty recursive definition body before ';'")
          })
      }
    | RecName DEF EXTRA ENDDEF {
          crate::with_state(state, |state| {
              state.recovery_statement("syntax error: invalid recursive definition token before ';'")
          })
      }
    ;

DefName -> tlib::TreeId:
      IDENT {
          crate::with_state(state, |state| state.ident_from_token($lexer, $1, true))
      }
    | PROCESS {
          crate::with_state(state, |state| state.ident_from_token($lexer, $1, true))
      }
    ;

RecName -> tlib::TreeId:
      DELAY1 IdentExpr { $2 }
    ;

ParamList -> tlib::TreeId:
      IdentExpr {
          crate::with_state(state, |state| {
              let nil = state.nil();
              state.cons($1, nil)
          })
      }
    | ParamList PAR IdentExpr {
          crate::with_state(state, |state| state.cons($3, $1))
      }
    ;

ModEntry -> tlib::TreeId:
      UQString {
          crate::with_state(state, |state| {
              let nil = state.nil();
              state.cons($1, nil)
          })
      }
    | UQString SEQ Argument {
          crate::with_state(state, |state| state.cons($1, $3))
      }
    ;

ModList -> tlib::TreeId:
      ModEntry {
          crate::with_state(state, |state| {
              let nil = state.nil();
              state.cons($1, nil)
          })
      }
    | ModList PAR ModEntry {
          crate::with_state(state, |state| state.cons($3, $1))
      }
    ;

ArgList -> tlib::TreeId:
      Argument {
          crate::with_state(state, |state| {
              let nil = state.nil();
              state.cons($1, nil)
          })
      }
    | ArgList PAR Argument {
          crate::with_state(state, |state| state.cons($3, $1))
      }
    ;

Argument -> tlib::TreeId:
      Argument SEQ Argument {
          crate::with_state(state, |state| state.seq_from_token($lexer, $2, $1, $3))
      }
    | Argument SPLIT Argument {
          crate::with_state(state, |state| state.split_from_token($lexer, $2, $1, $3))
      }
    | Argument MIX Argument {
          crate::with_state(state, |state| state.merge_from_token($lexer, $2, $1, $3))
      }
    | Argument REC Argument {
          crate::with_state(state, |state| state.rec_from_token($lexer, $2, $1, $3))
      }
    | InfixExp { $1 }
    ;

Expression -> tlib::TreeId:
      Expression WITH LBRAQ DefList RBRAQ {
          crate::with_state(state, |state| {
              let defs = state.format_definitions($4);
              state.node_builder().with_local_def($1, defs)
          })
      }
    | Expression LETREC LBRAQ RecList RBRAQ {
          crate::with_state(state, |state| {
              let defs = state.format_definitions($4);
              let nil = state.nil();
              state.node_builder().with_rec_def($1, defs, nil)
          })
      }
    | Expression LETREC LBRAQ RecList WHERE DefList RBRAQ {
          crate::with_state(state, |state| {
              let rec_defs = state.format_definitions($4);
              let defs = state.format_definitions($6);
              state.node_builder().with_rec_def($1, rec_defs, defs)
          })
      }
    | Expression PAR Expression {
          crate::with_state(state, |state| state.par_from_token($lexer, $2, $1, $3))
      }
    | Expression SEQ Expression {
          crate::with_state(state, |state| state.seq_from_token($lexer, $2, $1, $3))
      }
    | Expression SPLIT Expression {
          crate::with_state(state, |state| state.split_from_token($lexer, $2, $1, $3))
      }
    | Expression MIX Expression {
          crate::with_state(state, |state| state.merge_from_token($lexer, $2, $1, $3))
      }
    | Expression REC Expression {
          crate::with_state(state, |state| state.rec_from_token($lexer, $2, $1, $3))
      }
    | InfixExp { $1 }
    ;

InfixExp -> tlib::TreeId:
      InfixExp ADD InfixExp {
          crate::with_state(state, |state| state.binary_prim($1, $3, crate::PrimitiveOp::Add))
      }
    | InfixExp SUB InfixExp {
          crate::with_state(state, |state| state.binary_prim($1, $3, crate::PrimitiveOp::Sub))
      }
    | InfixExp MUL InfixExp {
          crate::with_state(state, |state| state.binary_prim($1, $3, crate::PrimitiveOp::Mul))
      }
    | InfixExp DIV InfixExp {
          crate::with_state(state, |state| state.binary_prim($1, $3, crate::PrimitiveOp::Div))
      }
    | InfixExp MOD InfixExp {
          crate::with_state(state, |state| state.binary_prim($1, $3, crate::PrimitiveOp::Rem))
      }
    | InfixExp POWOP InfixExp {
          crate::with_state(state, |state| state.binary_prim($1, $3, crate::PrimitiveOp::Pow))
      }
    | InfixExp FDELAY InfixExp {
          crate::with_state(state, |state| state.binary_prim($1, $3, crate::PrimitiveOp::Delay))
      }
    | InfixExp DELAY1 {
          crate::with_state(state, |state| state.postfix_prim($1, crate::PrimitiveOp::Delay1))
      }
    | InfixExp DOT IdentExpr {
          crate::with_state(state, |state| state.access_box($1, $3))
      }
    | InfixExp AND InfixExp {
          crate::with_state(state, |state| state.binary_prim($1, $3, crate::PrimitiveOp::And))
      }
    | InfixExp OR InfixExp {
          crate::with_state(state, |state| state.binary_prim($1, $3, crate::PrimitiveOp::Or))
      }
    | InfixExp XOR InfixExp {
          crate::with_state(state, |state| state.binary_prim($1, $3, crate::PrimitiveOp::Xor))
      }
    | InfixExp LSH InfixExp {
          crate::with_state(state, |state| state.binary_prim($1, $3, crate::PrimitiveOp::Lsh))
      }
    | InfixExp RSH InfixExp {
          crate::with_state(state, |state| state.binary_prim($1, $3, crate::PrimitiveOp::Rsh))
      }
    | InfixExp LT InfixExp {
          crate::with_state(state, |state| state.binary_prim($1, $3, crate::PrimitiveOp::Lt))
      }
    | InfixExp LE InfixExp {
          crate::with_state(state, |state| state.binary_prim($1, $3, crate::PrimitiveOp::Le))
      }
    | InfixExp GT InfixExp {
          crate::with_state(state, |state| state.binary_prim($1, $3, crate::PrimitiveOp::Gt))
      }
    | InfixExp GE InfixExp {
          crate::with_state(state, |state| state.binary_prim($1, $3, crate::PrimitiveOp::Ge))
      }
    | InfixExp EQ InfixExp {
          crate::with_state(state, |state| state.binary_prim($1, $3, crate::PrimitiveOp::Eq))
      }
    | InfixExp NE InfixExp {
          crate::with_state(state, |state| state.binary_prim($1, $3, crate::PrimitiveOp::Ne))
      }
    | InfixExp LPAR ArgList RPAR {
          crate::with_state(state, |state| state.apply_box($1, $3))
      }
    | InfixExp LCROC DefList RCROC {
          crate::with_state(state, |state| {
              let defs = state.format_definitions($3);
              state.modif_local_def_box($1, defs)
          })
      }
    | Primitive { $1 }
    ;

Primitive -> tlib::TreeId:
      INT {
          crate::with_state(state, |state| state.int_from_token($lexer, $1))
      }
    | FLOAT {
          crate::with_state(state, |state| state.float_from_token($lexer, $1))
      }
    | ADD INT {
          crate::with_state(state, |state| state.signed_int_from_token($lexer, $2, 1))
      }
    | ADD FLOAT {
          crate::with_state(state, |state| state.signed_float_from_token($lexer, $2, 1.0))
      }
    | SUB INT {
          crate::with_state(state, |state| state.signed_int_from_token($lexer, $2, -1))
      }
    | SUB FLOAT {
          crate::with_state(state, |state| state.signed_float_from_token($lexer, $2, -1.0))
      }
    | WIRE {
          crate::with_state(state, |state| state.node_builder().wire())
      }
    | CUT {
          crate::with_state(state, |state| state.node_builder().cut())
      }
    | MEM {
          crate::with_state(state, |state| state.node_builder().delay1())
      }
    | PREFIX {
          crate::with_state(state, |state| state.node_builder().prefix())
      }
    | INTCAST {
          crate::with_state(state, |state| state.node_builder().int_cast())
      }
    | FLOATCAST {
          crate::with_state(state, |state| state.node_builder().float_cast())
      }
    | ADD {
          crate::with_state(state, |state| state.node_builder().add())
      }
    | SUB {
          crate::with_state(state, |state| state.node_builder().sub())
      }
    | MUL {
          crate::with_state(state, |state| state.node_builder().mul())
      }
    | DIV {
          crate::with_state(state, |state| state.node_builder().div())
      }
    | MOD {
          crate::with_state(state, |state| state.node_builder().rem())
      }
    | FDELAY {
          crate::with_state(state, |state| state.node_builder().delay())
      }
    | AND {
          crate::with_state(state, |state| state.node_builder().and())
      }
    | OR {
          crate::with_state(state, |state| state.node_builder().or())
      }
    | XOR {
          crate::with_state(state, |state| state.node_builder().xor())
      }
    | LSH {
          crate::with_state(state, |state| state.node_builder().lsh())
      }
    | RSH {
          crate::with_state(state, |state| state.node_builder().rsh())
      }
    | LT {
          crate::with_state(state, |state| state.node_builder().lt())
      }
    | LE {
          crate::with_state(state, |state| state.node_builder().le())
      }
    | GT {
          crate::with_state(state, |state| state.node_builder().gt())
      }
    | GE {
          crate::with_state(state, |state| state.node_builder().ge())
      }
    | EQ {
          crate::with_state(state, |state| state.node_builder().eq())
      }
    | NE {
          crate::with_state(state, |state| state.node_builder().ne())
      }
    | POWOP {
          crate::with_state(state, |state| state.node_builder().pow())
      }
    | POWFUN {
          crate::with_state(state, |state| state.node_builder().pow())
      }
    | ACOS {
          crate::with_state(state, |state| state.node_builder().acos())
      }
    | ASIN {
          crate::with_state(state, |state| state.node_builder().asin())
      }
    | ATAN {
          crate::with_state(state, |state| state.node_builder().atan())
      }
    | ATAN2 {
          crate::with_state(state, |state| state.node_builder().atan2())
      }
    | COS {
          crate::with_state(state, |state| state.node_builder().cos())
      }
    | SIN {
          crate::with_state(state, |state| state.node_builder().sin())
      }
    | TAN {
          crate::with_state(state, |state| state.node_builder().tan())
      }
    | EXP {
          crate::with_state(state, |state| state.node_builder().exp())
      }
    | LOG {
          crate::with_state(state, |state| state.node_builder().log())
      }
    | LOG10 {
          crate::with_state(state, |state| state.node_builder().log10())
      }
    | SQRT {
          crate::with_state(state, |state| state.node_builder().sqrt())
      }
    | ABS {
          crate::with_state(state, |state| state.node_builder().abs())
      }
    | MIN {
          crate::with_state(state, |state| state.node_builder().min())
      }
    | MAX {
          crate::with_state(state, |state| state.node_builder().max())
      }
    | FMOD {
          crate::with_state(state, |state| state.node_builder().fmod())
      }
    | REMAINDER {
          crate::with_state(state, |state| state.node_builder().remainder())
      }
    | FLOOR {
          crate::with_state(state, |state| state.node_builder().floor())
      }
    | CEIL {
          crate::with_state(state, |state| state.node_builder().ceil())
      }
    | RINT {
          crate::with_state(state, |state| state.node_builder().rint())
      }
    | ROUND {
          crate::with_state(state, |state| state.node_builder().round())
      }
    | RDTBL {
          crate::with_state(state, |state| state.node_builder().read_only_table())
      }
    | RWTBL {
          crate::with_state(state, |state| state.node_builder().write_read_table())
      }
    | SELECT2 {
          crate::with_state(state, |state| state.node_builder().select2())
      }
    | SELECT3 {
          crate::with_state(state, |state| state.node_builder().select3())
      }
    | ASSERTBOUNDS {
          crate::with_state(state, |state| state.node_builder().assert_bounds())
      }
    | LOWEST {
          crate::with_state(state, |state| state.node_builder().lowest())
      }
    | HIGHEST {
          crate::with_state(state, |state| state.node_builder().highest())
      }
    | ATTACH {
          crate::with_state(state, |state| state.node_builder().attach())
      }
    | ENABLE {
          crate::with_state(state, |state| state.node_builder().enable())
      }
    | CONTROL {
          crate::with_state(state, |state| state.node_builder().control())
      }
    | FFUNCTION LPAR Signature PAR FString PAR RawString RPAR {
          crate::with_state(state, |state| state.node_foreign_function($3, $5, $7))
      }
    | FCONSTANT LPAR Type Name PAR FString RPAR {
          crate::with_state(state, |state| state.node_builder().fconst($3, $4, $6))
      }
    | FVARIABLE LPAR Type Name PAR FString RPAR {
          crate::with_state(state, |state| state.node_builder().fvar($3, $4, $6))
      }
    | CASE LBRAQ RuleList RBRAQ {
          crate::with_state(state, |state| state.node_case_checked($3))
      }
    | COMPONENT LPAR UQString RPAR {
          crate::with_state(state, |state| state.node_builder().component($3))
      }
    | LIBRARY LPAR UQString RPAR {
          crate::with_state(state, |state| state.node_builder().library($3))
      }
    | ENVIRONMENT LBRAQ StmtList RBRAQ {
          crate::with_state(state, |state| {
              let env = state.node_builder().environment();
              let defs = state.format_definitions($3);
              state.node_builder().with_local_def(env, defs)
          })
      }
    | WAVEFORM LBRAQ ValList RBRAQ {
          crate::with_state(state, |state| state.waveform_box_from_ctx())
      }
    | ROUTE LPAR Argument PAR Argument RPAR {
          crate::with_state(state, |state| state.route_box_default_spec($3, $5))
      }
    | ROUTE LPAR Argument PAR Argument PAR Expression RPAR {
          crate::with_state(state, |state| state.node_builder().route($3, $5, $7))
      }
    | BUTTON LPAR UQString RPAR {
          crate::with_state(state, |state| {
              let node = state.node_builder().button($3);
              state.record_widget_declaration($lexer, $1, $3, node)
          })
      }
    | CHECKBOX LPAR UQString RPAR {
          crate::with_state(state, |state| {
              let node = state.node_builder().checkbox($3);
              state.record_widget_declaration($lexer, $1, $3, node)
          })
      }
    | VSLIDER LPAR UQString PAR Argument PAR Argument PAR Argument PAR Argument RPAR {
          crate::with_state(state, |state| {
              let node = state.node_builder().vslider($3, $5, $7, $9, $11);
              state.record_widget_declaration($lexer, $1, $3, node)
          })
      }
    | HSLIDER LPAR UQString PAR Argument PAR Argument PAR Argument PAR Argument RPAR {
          crate::with_state(state, |state| {
              let node = state.node_builder().hslider($3, $5, $7, $9, $11);
              state.record_widget_declaration($lexer, $1, $3, node)
          })
      }
    | NENTRY LPAR UQString PAR Argument PAR Argument PAR Argument PAR Argument RPAR {
          crate::with_state(state, |state| {
              let node = state.node_builder().num_entry($3, $5, $7, $9, $11);
              state.record_widget_declaration($lexer, $1, $3, node)
          })
      }
    | VBARGRAPH LPAR UQString PAR Argument PAR Argument RPAR {
          crate::with_state(state, |state| {
              let node = state.node_builder().vbargraph($3, $5, $7);
              state.record_widget_declaration($lexer, $1, $3, node)
          })
      }
    | HBARGRAPH LPAR UQString PAR Argument PAR Argument RPAR {
          crate::with_state(state, |state| {
              let node = state.node_builder().hbargraph($3, $5, $7);
              state.record_widget_declaration($lexer, $1, $3, node)
          })
      }
    | VGROUP LPAR UQString PAR Expression RPAR {
          crate::with_state(state, |state| state.node_builder().vgroup($3, $5))
      }
    | HGROUP LPAR UQString PAR Expression RPAR {
          crate::with_state(state, |state| state.node_builder().hgroup($3, $5))
      }
    | TGROUP LPAR UQString PAR Expression RPAR {
          crate::with_state(state, |state| state.node_builder().tgroup($3, $5))
      }
    | SOUNDFILE LPAR UQString PAR Argument RPAR {
          crate::with_state(state, |state| state.node_builder().soundfile($3, $5))
      }
    | IPAR LPAR IdentExpr PAR Argument PAR Expression RPAR {
          crate::with_state(state, |state| state.node_builder().ipar($3, $5, $7))
      }
    | ISEQ LPAR IdentExpr PAR Argument PAR Expression RPAR {
          crate::with_state(state, |state| state.node_builder().iseq($3, $5, $7))
      }
    | ISUM LPAR IdentExpr PAR Argument PAR Expression RPAR {
          crate::with_state(state, |state| state.node_builder().isum($3, $5, $7))
      }
    | IPROD LPAR IdentExpr PAR Argument PAR Expression RPAR {
          crate::with_state(state, |state| state.node_builder().iprod($3, $5, $7))
      }
    | INPUTS LPAR Expression RPAR {
          crate::with_state(state, |state| state.node_builder().inputs($3))
      }
    | OUTPUTS LPAR Expression RPAR {
          crate::with_state(state, |state| state.node_builder().outputs($3))
      }
    | FAUTODIFF LPAR Argument PAR Argument RPAR {
          crate::with_state(state, |state| state.node_builder().forward_ad($3, $5))
      }
    | RAUTODIFF LPAR Argument PAR Argument RPAR {
          crate::with_state(state, |state| state.node_builder().reverse_ad($3, $5))
      }
    | ONDEMAND LPAR Expression RPAR {
          crate::with_state(state, |state| state.node_builder().ondemand($3))
      }
    | UPSAMPLING LPAR Expression RPAR {
          crate::with_state(state, |state| state.node_builder().upsampling($3))
      }
    | DOWNSAMPLING LPAR Expression RPAR {
          crate::with_state(state, |state| state.node_builder().downsampling($3))
      }
    | LAMBDA LPAR ParamList RPAR DOT LPAR Expression RPAR {
          crate::with_state(state, |state| state.node_lambda($3, $7))
      }
    | MODULATE LPAR ModList RPAR DOT LPAR Expression RPAR {
          crate::with_state(state, |state| {
              let _ = ($3, $7);
              state.recovery_statement(
                  "syntax error: legacy minput modulation form is not supported; use [modlist -> expression]",
              )
          })
      }
    | LCROC ModList LAPPLY Expression RCROC {
          crate::with_state(state, |state| state.node_builder().build_modulation($2, $4))
      }
    | IdentExpr { $1 }
    | SUB IdentExpr {
          crate::with_state(state, |state| {
              let zero = state.node_builder().int(0);
              state.binary_prim(zero, $2, crate::PrimitiveOp::Sub)
          })
      }
    | LPAR Expression RPAR { $2 }
    ;

Signature -> tlib::TreeId:
      Type FunName LPAR TypeList RPAR {
          crate::with_state(state, |state| {
              let names = state.foreign_name_slots($2, None, None, None);
              state.foreign_signature($1, names, $4)
          })
      }
    | Type FunName OR FunName LPAR TypeList RPAR {
          crate::with_state(state, |state| {
              let names = state.foreign_name_slots($2, Some($4), None, None);
              state.foreign_signature($1, names, $6)
          })
      }
    | Type FunName OR FunName OR FunName LPAR TypeList RPAR {
          crate::with_state(state, |state| {
              let names = state.foreign_name_slots($2, Some($4), Some($6), None);
              state.foreign_signature($1, names, $8)
          })
      }
    | Type FunName OR FunName OR FunName OR FunName LPAR TypeList RPAR {
          crate::with_state(state, |state| {
              let names = state.foreign_name_slots($2, Some($4), Some($6), Some($8));
              state.foreign_signature($1, names, $10)
          })
      }
    | Type FunName LPAR RPAR {
          crate::with_state(state, |state| {
              let names = state.foreign_name_slots($2, None, None, None);
              let nil = state.nil();
              state.foreign_signature($1, names, nil)
          })
      }
    | Type FunName OR FunName LPAR RPAR {
          crate::with_state(state, |state| {
              let names = state.foreign_name_slots($2, Some($4), None, None);
              let nil = state.nil();
              state.foreign_signature($1, names, nil)
          })
      }
    | Type FunName OR FunName OR FunName LPAR RPAR {
          crate::with_state(state, |state| {
              let names = state.foreign_name_slots($2, Some($4), Some($6), None);
              let nil = state.nil();
              state.foreign_signature($1, names, nil)
          })
      }
    | Type FunName OR FunName OR FunName OR FunName LPAR RPAR {
          crate::with_state(state, |state| {
              let names = state.foreign_name_slots($2, Some($4), Some($6), Some($8));
              let nil = state.nil();
              state.foreign_signature($1, names, nil)
          })
      }
    ;

TypeList -> tlib::TreeId:
      ArgType {
          crate::with_state(state, |state| {
              let nil = state.nil();
              state.cons($1, nil)
          })
      }
    | TypeList PAR ArgType {
          crate::with_state(state, |state| state.cons($3, $1))
      }
    ;

Type -> tlib::TreeId:
      ScalarType { $1 }
    ;

ArgType -> tlib::TreeId:
      ScalarType { $1 }
    | NOTYPECAST {
          crate::with_state(state, |state| state.foreign_type_code(2))
      }
    ;

ScalarType -> tlib::TreeId:
      INTCAST {
          crate::with_state(state, |state| state.foreign_type_code(0))
      }
    | FLOATCAST {
          crate::with_state(state, |state| state.foreign_type_code(1))
      }
    ;

FunName -> tlib::TreeId:
      IDENT {
          crate::with_state(state, |state| state.symbol_from_token($lexer, $1, false))
      }
    ;

Name -> tlib::TreeId:
      IDENT {
          crate::with_state(state, |state| state.symbol_from_token($lexer, $1, true))
      }
    ;

FString -> tlib::TreeId:
      STRING {
          crate::with_state(state, |state| state.raw_symbol_from_token($lexer, $1))
      }
    | FSTRING {
          crate::with_state(state, |state| state.raw_symbol_from_token($lexer, $1))
      }
    ;

RawString -> tlib::TreeId:
      STRING {
          crate::with_state(state, |state| state.raw_symbol_from_token($lexer, $1))
      }
    ;

RuleList -> tlib::TreeId:
      Rule {
          crate::with_state(state, |state| {
              let nil = state.nil();
              state.cons($1, nil)
          })
      }
    | RuleList Rule {
          crate::with_state(state, |state| state.cons($2, $1))
      }
    ;

Rule -> tlib::TreeId:
      LPAR ArgList RPAR ARROW Expression ENDDEF {
          crate::with_state(state, |state| state.cons($2, $5))
      }
    ;

ValList -> u8:
      Number {
          crate::with_state(state, |state| {
              state.push_waveform_value($1);
              0
          })
      }
    | ValList PAR Number {
          crate::with_state(state, |state| {
              state.push_waveform_value($3);
              0
          })
      }
    ;

Number -> tlib::TreeId:
      INT {
          crate::with_state(state, |state| state.int_from_token($lexer, $1))
      }
    | FLOAT {
          crate::with_state(state, |state| state.float_from_token($lexer, $1))
      }
    | ADD INT {
          crate::with_state(state, |state| state.signed_int_from_token($lexer, $2, 1))
      }
    | ADD FLOAT {
          crate::with_state(state, |state| state.signed_float_from_token($lexer, $2, 1.0))
      }
    | SUB INT {
          crate::with_state(state, |state| state.signed_int_from_token($lexer, $2, -1))
      }
    | SUB FLOAT {
          crate::with_state(state, |state| state.signed_float_from_token($lexer, $2, -1.0))
      }
    ;

UQString -> tlib::TreeId:
      STRING {
          crate::with_state(state, |state| state.uqstring_from_token($lexer, $1))
      }
    | FSTRING {
          crate::with_state(state, |state| state.uqstring_from_token($lexer, $1))
      }
    ;

IdentExpr -> tlib::TreeId:
      IDENT {
          crate::with_state(state, |state| state.ident_from_token($lexer, $1, true))
      }
    | PROCESS {
          crate::with_state(state, |state| state.ident_from_token($lexer, $1, true))
      }
    ;
%%
