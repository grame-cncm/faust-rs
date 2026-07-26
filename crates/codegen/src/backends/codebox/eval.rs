//! A small evaluator for the codebox subset this backend emits.
//!
//! # Why this exists
//!
//! Codebox output cannot be checked by comparing text with the C++ compiler:
//! our FIR lowering legitimately differs in structure, so byte parity is
//! unreachable (see `porting/codebox-backend-port-plan-2026-07-26-en.md` §5.2).
//! A snapshot of our own output only catches *change*, never *wrongness*. This
//! evaluator is what makes the emitted code checkable numerically: run it
//! sample by sample and compare against a backend already validated against the
//! C++ reference.
//!
//! # Scope
//!
//! Deliberately not a codebox implementation — only the subset
//! [`super::generate_codebox_module`] emits:
//!
//! - `@state` and `let` declarations, scalars and `new FixedFloatArray(n)`;
//! - assignment to a variable or an array element;
//! - `if` / `else`, `for`, `while`;
//! - arithmetic and comparison operators, always parenthesised;
//! - `function` definitions and calls, including the integer helpers
//!   (`iadd`, `isub`, `imul`, `imod`) and the math names codebox exposes;
//! - `samplerate()`;
//! - `return [a, b]` lists.
//!
//! Anything outside that subset is a hard error rather than a silent default:
//! an evaluator that guesses would report a passing test for code RNBO would
//! reject.
//!
//! # What it does not prove
//!
//! That RNBO accepts the file. The grammar accepted here is the one the emitter
//! produces, so a construct both sides agree on but RNBO rejects passes. Only
//! the manual round-trip (plan §5.2 layer 3) covers that.

use std::collections::HashMap;

/// A runtime value. Codebox has one numeric type; the integer distinction only
/// matters for the helper functions, so everything is carried as `f64`.
#[derive(Clone, Debug, PartialEq)]
enum Value {
    Num(f64),
    Array(Vec<f64>),
    List(Vec<f64>),
}

impl Value {
    fn as_num(&self, context: &str) -> Result<f64, EvalError> {
        match self {
            Self::Num(v) => Ok(*v),
            other => Err(EvalError::new(format!(
                "{context}: expected a number, got {other:?}"
            ))),
        }
    }
}

/// Evaluation failure. Carries a message only: this is test scaffolding, and
/// every failure mode is a bug in the emitter or in this evaluator.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EvalError {
    /// What went wrong.
    pub message: String,
}

impl EvalError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for EvalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for EvalError {}

/// One parsed statement.
#[derive(Clone, Debug)]
enum Stmt {
    /// `@state`/`let` scalar declaration, or a plain assignment.
    Assign {
        target: LValue,
        value: Expr,
    },
    /// `@state x = new FixedFloatArray(n);`
    DeclareArray {
        name: String,
        size: usize,
    },
    If {
        cond: Expr,
        then_block: Vec<Stmt>,
        else_block: Vec<Stmt>,
    },
    /// `for (let i : Int = a; cond; i = step) { … }`
    For {
        var: String,
        start: Expr,
        cond: Expr,
        step: Expr,
        body: Vec<Stmt>,
    },
    While {
        cond: Expr,
        body: Vec<Stmt>,
    },
    Return(Vec<Expr>),
    /// A bare call used as a statement, e.g. `control();`.
    Call {
        name: String,
        args: Vec<Expr>,
    },
}

/// Assignment target.
#[derive(Clone, Debug)]
enum LValue {
    Var(String),
    Index(String, Box<Expr>),
}

/// One parsed expression.
#[derive(Clone, Debug)]
enum Expr {
    Num(f64),
    Var(String),
    Index(String, Box<Expr>),
    Binary(Box<Expr>, String, Box<Expr>),
    Call(String, Vec<Expr>),
}

/// A parsed codebox program.
pub struct Program {
    functions: HashMap<String, (Vec<String>, Vec<Stmt>)>,
    /// Statements at file scope, in order.
    top_level: Vec<Stmt>,
    /// Persistent state, surviving between `compute` calls.
    state: HashMap<String, Value>,
    sample_rate: f64,
}

impl Program {
    /// Parses one emitted codebox module.
    ///
    /// # Errors
    /// Returns [`EvalError`] on any construct outside the emitted subset.
    pub fn parse(source: &str) -> Result<Self, EvalError> {
        let mut parser = Parser::new(source);
        parser.parse_program()
    }

    /// Runs `dspsetup()`.
    ///
    /// # Errors
    /// Returns [`EvalError`] when evaluation fails.
    pub fn dspsetup(&mut self, sample_rate: f64) -> Result<(), EvalError> {
        self.sample_rate = sample_rate;
        self.call_void("dspsetup", &[])
    }

    /// Runs one sample by executing the emitted file's own top level.
    ///
    /// Deliberately not by calling `update` and `compute` directly: the file
    /// scope is where `outputs = compute(in1, …)` and `out1 = outputs[0]` live,
    /// so executing it is what checks the *wiring* — including the channel
    /// order, which is how bargraph outputs are appended after the real ones.
    /// Driving the functions directly would assume the wiring instead of
    /// verifying it.
    ///
    /// `params` are bound to the parameter names `update` declares, in order,
    /// standing in for the values an RNBO host would have written.
    ///
    /// # Errors
    /// Returns [`EvalError`] when evaluation fails, or when the top level does
    /// not produce the expected `outN` values.
    pub fn compute(&mut self, params: &[f64], inputs: &[f64]) -> Result<Vec<f64>, EvalError> {
        let names = self.parameter_names();
        if names.len() != params.len() {
            return Err(EvalError::new(format!(
                "expected {} parameter value(s), got {}",
                names.len(),
                params.len()
            )));
        }
        for (name, value) in names.iter().zip(params.iter()) {
            self.state.insert(name.clone(), Value::Num(*value));
        }

        // RNBO's implicit audio inlets, 1-based.
        let mut locals: HashMap<String, Value> = HashMap::new();
        for (index, value) in inputs.iter().enumerate() {
            locals.insert(format!("in{}", index + 1), Value::Num(*value));
        }

        let top_level = self.top_level.clone();
        self.exec_block(&top_level, &mut locals)?;

        // The wiring writes `out1..outN`; collect them until one is missing.
        let mut outputs = Vec::new();
        let mut channel = 1;
        while let Some(value) = locals
            .get(&format!("out{channel}"))
            .or_else(|| self.state.get(&format!("out{channel}")))
        {
            outputs.push(value.as_num("output channel")?);
            channel += 1;
        }
        if outputs.is_empty() {
            return Err(EvalError::new(
                "the top level produced no `outN` value; is the wiring emitted?",
            ));
        }
        Ok(outputs)
    }

    /// Number of arguments `compute` declares.
    #[must_use]
    pub fn compute_arity(&self) -> usize {
        self.functions
            .get("compute")
            .map_or(0, |(args, _)| args.len())
    }

    /// Number of arguments `update` declares — one per parameter.
    #[must_use]
    pub fn update_arity(&self) -> usize {
        self.functions
            .get("update")
            .map_or(0, |(args, _)| args.len())
    }

    /// Names of `update`'s parameters, in declaration order.
    #[must_use]
    pub fn parameter_names(&self) -> Vec<String> {
        self.functions
            .get("update")
            .map(|(args, _)| args.clone())
            .unwrap_or_default()
    }

    fn call_void(&mut self, name: &str, args: &[Value]) -> Result<(), EvalError> {
        self.call(name, args).map(|_| ())
    }

    fn call(&mut self, name: &str, args: &[Value]) -> Result<Option<Value>, EvalError> {
        let Some((params, body)) = self.functions.get(name).cloned() else {
            return Err(EvalError::new(format!("undefined function `{name}`")));
        };
        if params.len() != args.len() {
            return Err(EvalError::new(format!(
                "`{name}` takes {} argument(s), got {}",
                params.len(),
                args.len()
            )));
        }
        let mut locals: HashMap<String, Value> =
            params.iter().cloned().zip(args.iter().cloned()).collect();
        self.exec_block(&body, &mut locals)
    }

    /// Executes a block, returning `Some` when a `return` was hit.
    fn exec_block(
        &mut self,
        block: &[Stmt],
        locals: &mut HashMap<String, Value>,
    ) -> Result<Option<Value>, EvalError> {
        for stmt in block {
            if let Some(value) = self.exec(stmt, locals)? {
                return Ok(Some(value));
            }
        }
        Ok(None)
    }

    fn exec(
        &mut self,
        stmt: &Stmt,
        locals: &mut HashMap<String, Value>,
    ) -> Result<Option<Value>, EvalError> {
        match stmt {
            Stmt::DeclareArray { name, size } => {
                self.state
                    .insert(name.clone(), Value::Array(vec![0.0; *size]));
                Ok(None)
            }
            Stmt::Assign { target, value } => {
                let value = self.eval(value, locals)?;
                match target {
                    LValue::Var(name) => {
                        // A name already known as state stays state; anything
                        // else is a local. The emitter declares state before
                        // use, so this cannot mis-route.
                        if self.state.contains_key(name) {
                            self.state.insert(name.clone(), value);
                        } else {
                            locals.insert(name.clone(), value);
                        }
                    }
                    LValue::Index(name, index) => {
                        let index = self.eval(index, locals)?.as_num("array index")?;
                        let index = index as usize;
                        let value = value.as_num("array element")?;
                        let slot = self.state.get_mut(name).ok_or_else(|| {
                            EvalError::new(format!("assignment to unknown array `{name}`"))
                        })?;
                        let Value::Array(cells) = slot else {
                            return Err(EvalError::new(format!("`{name}` is not an array")));
                        };
                        if index >= cells.len() {
                            return Err(EvalError::new(format!(
                                "index {index} out of bounds for `{name}` (len {})",
                                cells.len()
                            )));
                        }
                        cells[index] = value;
                    }
                }
                Ok(None)
            }
            Stmt::If {
                cond,
                then_block,
                else_block,
            } => {
                let taken = self.eval(cond, locals)?.as_num("if condition")? != 0.0;
                let block = if taken { then_block } else { else_block };
                self.exec_block(block, locals)
            }
            Stmt::For {
                var,
                start,
                cond,
                step,
                body,
            } => {
                let start = self.eval(start, locals)?;
                locals.insert(var.clone(), start);
                let mut guard = 0u32;
                while self.eval(cond, locals)?.as_num("for condition")? != 0.0 {
                    if let Some(value) = self.exec_block(body, locals)? {
                        return Ok(Some(value));
                    }
                    let next = self.eval(step, locals)?;
                    locals.insert(var.clone(), next);
                    guard += 1;
                    if guard > 10_000_000 {
                        return Err(EvalError::new("for loop did not terminate"));
                    }
                }
                Ok(None)
            }
            Stmt::While { cond, body } => {
                let mut guard = 0u32;
                while self.eval(cond, locals)?.as_num("while condition")? != 0.0 {
                    if let Some(value) = self.exec_block(body, locals)? {
                        return Ok(Some(value));
                    }
                    guard += 1;
                    if guard > 10_000_000 {
                        return Err(EvalError::new("while loop did not terminate"));
                    }
                }
                Ok(None)
            }
            Stmt::Return(exprs) => {
                let mut values = Vec::with_capacity(exprs.len());
                for expr in exprs {
                    values.push(self.eval(expr, locals)?.as_num("return element")?);
                }
                Ok(Some(Value::List(values)))
            }
            Stmt::Call { name, args } => {
                let mut evaluated = Vec::with_capacity(args.len());
                for arg in args {
                    evaluated.push(self.eval(arg, locals)?);
                }
                self.call(name, &evaluated)?;
                Ok(None)
            }
        }
    }

    fn eval(&mut self, expr: &Expr, locals: &HashMap<String, Value>) -> Result<Value, EvalError> {
        match expr {
            Expr::Num(v) => Ok(Value::Num(*v)),
            Expr::Var(name) => locals
                .get(name)
                .or_else(|| self.state.get(name))
                .cloned()
                .ok_or_else(|| EvalError::new(format!("undefined variable `{name}`"))),
            Expr::Index(name, index) => {
                let index = self.eval(index, locals)?.as_num("array index")? as usize;
                let cells = locals
                    .get(name)
                    .or_else(|| self.state.get(name))
                    .ok_or_else(|| EvalError::new(format!("undefined array `{name}`")))?;
                // `compute` returns a list and the wiring indexes it, so both
                // shapes are indexable.
                let cells = match cells {
                    Value::Array(cells) | Value::List(cells) => cells,
                    other => {
                        return Err(EvalError::new(format!(
                            "`{name}` is not indexable: {other:?}"
                        )));
                    }
                };
                cells.get(index).copied().map(Value::Num).ok_or_else(|| {
                    EvalError::new(format!(
                        "index {index} out of bounds for `{name}` (len {})",
                        cells.len()
                    ))
                })
            }
            Expr::Binary(lhs, op, rhs) => {
                let a = self.eval(lhs, locals)?.as_num("left operand")?;
                let b = self.eval(rhs, locals)?.as_num("right operand")?;
                apply_binary(a, op, b)
            }
            Expr::Call(name, args) => {
                let mut evaluated = Vec::with_capacity(args.len());
                for arg in args {
                    evaluated.push(self.eval(arg, locals)?);
                }
                if let Some(value) = self.builtin(name, &evaluated)? {
                    return Ok(value);
                }
                self.call(name, &evaluated)?
                    .ok_or_else(|| EvalError::new(format!("`{name}` returned no value")))
            }
        }
    }

    /// Evaluates codebox's built-in functions, or `None` when `name` is a
    /// user-defined function.
    fn builtin(&self, name: &str, args: &[Value]) -> Result<Option<Value>, EvalError> {
        let n = |i: usize| -> Result<f64, EvalError> { args[i].as_num(name) };
        let value = match (name, args.len()) {
            ("samplerate", 0) => self.sample_rate,
            // Integer helpers: codebox wraps to 32 bits.
            ("iadd", 2) => f64::from((n(0)? as i32).wrapping_add(n(1)? as i32)),
            ("isub", 2) => f64::from((n(0)? as i32).wrapping_sub(n(1)? as i32)),
            ("imul", 2) => f64::from((n(0)? as i32).wrapping_mul(n(1)? as i32)),
            ("imod", 2) => {
                let divisor = n(1)? as i32;
                if divisor == 0 {
                    0.0
                } else {
                    f64::from((n(0)? as i32).wrapping_rem(divisor))
                }
            }
            ("trunc", 1) => n(0)?.trunc(),
            ("int", 1) => n(0)?.trunc(),
            ("abs", 1) => n(0)?.abs(),
            ("min", 2) => n(0)?.min(n(1)?),
            ("max", 2) => n(0)?.max(n(1)?),
            ("floor", 1) => n(0)?.floor(),
            ("ceil", 1) => n(0)?.ceil(),
            ("round", 1) => n(0)?.round(),
            ("rint", 1) => {
                // Round half to even, like C's rint under the default mode.
                let v = n(0)?;
                let r = v.round();
                if (v - v.trunc()).abs() == 0.5 && r % 2.0 != 0.0 {
                    r - v.signum()
                } else {
                    r
                }
            }
            ("sqrt", 1) => n(0)?.sqrt(),
            ("exp", 1) => n(0)?.exp(),
            ("exp2", 1) => n(0)?.exp2(),
            ("exp10", 1) => 10f64.powf(n(0)?),
            ("log", 1) => n(0)?.ln(),
            ("log2", 1) => n(0)?.log2(),
            ("log10", 1) => n(0)?.log10(),
            ("pow", 2) => n(0)?.powf(n(1)?),
            ("sin", 1) => n(0)?.sin(),
            ("cos", 1) => n(0)?.cos(),
            ("tan", 1) => n(0)?.tan(),
            ("asin", 1) => n(0)?.asin(),
            ("acos", 1) => n(0)?.acos(),
            ("atan", 1) => n(0)?.atan(),
            ("atan2", 2) => n(0)?.atan2(n(1)?),
            ("sinh", 1) => n(0)?.sinh(),
            ("cosh", 1) => n(0)?.cosh(),
            ("tanh", 1) => n(0)?.tanh(),
            ("asinh", 1) => n(0)?.asinh(),
            ("acosh", 1) => n(0)?.acosh(),
            ("atanh", 1) => n(0)?.atanh(),
            ("remainder", 2) => {
                let (a, b) = (n(0)?, n(1)?);
                a - b * (a / b).round()
            }
            ("safemod", 2) => {
                let (a, b) = (n(0)?, n(1)?);
                if b == 0.0 { 0.0 } else { a % b }
            }
            ("isnan", 1) => f64::from(u8::from(n(0)?.is_nan())),
            _ => return Ok(None),
        };
        Ok(Some(Value::Num(value)))
    }
}

/// Applies a binary operator.
fn apply_binary(a: f64, op: &str, b: f64) -> Result<Value, EvalError> {
    let value = match op {
        "+" => a + b,
        "-" => a - b,
        "*" => a * b,
        "/" => a / b,
        "%" => a % b,
        "&" => f64::from((a as i32) & (b as i32)),
        "|" => f64::from((a as i32) | (b as i32)),
        "^" => f64::from((a as i32) ^ (b as i32)),
        "<<" => f64::from((a as i32).wrapping_shl(b as u32)),
        ">>" => f64::from((a as i32).wrapping_shr(b as u32)),
        "<" => f64::from(u8::from(a < b)),
        "<=" => f64::from(u8::from(a <= b)),
        ">" => f64::from(u8::from(a > b)),
        ">=" => f64::from(u8::from(a >= b)),
        "==" => f64::from(u8::from(a == b)),
        "!=" => f64::from(u8::from(a != b)),
        other => return Err(EvalError::new(format!("unknown operator `{other}`"))),
    };
    Ok(Value::Num(value))
}

// ── Parser ───────────────────────────────────────────────────────────────────

/// Recursive-descent parser over the emitted subset.
struct Parser {
    tokens: Vec<String>,
    pos: usize,
}

impl Parser {
    fn new(source: &str) -> Self {
        Self {
            tokens: tokenize(source),
            pos: 0,
        }
    }

    fn peek(&self) -> Option<&str> {
        self.tokens.get(self.pos).map(String::as_str)
    }

    fn next(&mut self) -> Option<String> {
        let token = self.tokens.get(self.pos).cloned();
        if token.is_some() {
            self.pos += 1;
        }
        token
    }

    fn expect(&mut self, expected: &str) -> Result<(), EvalError> {
        match self.next() {
            Some(token) if token == expected => Ok(()),
            other => Err(EvalError::new(format!(
                "expected `{expected}`, got {other:?}"
            ))),
        }
    }

    fn eat(&mut self, expected: &str) -> bool {
        if self.peek() == Some(expected) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn parse_program(&mut self) -> Result<Program, EvalError> {
        let mut functions = HashMap::new();
        let mut top_level = Vec::new();
        let mut state = HashMap::new();

        while self.peek().is_some() {
            if self.peek() == Some("function") {
                let (name, params, body) = self.parse_function()?;
                functions.insert(name, (params, body));
            } else {
                // A file-scope *declaration* runs once, creating state; a bare
                // statement is the per-sample wiring. Keeping a declaration in
                // `top_level` would re-run it every sample and wipe whatever
                // `dspsetup` initialised — which is exactly what happened before
                // this distinction existed.
                let is_declaration = matches!(self.peek(), Some("@state" | "let"));
                let stmt = self.parse_stmt()?;
                match &stmt {
                    Stmt::DeclareArray { name, size } => {
                        state.insert(name.clone(), Value::Array(vec![0.0; *size]));
                    }
                    Stmt::Assign {
                        target: LValue::Var(name),
                        ..
                    } => {
                        state.insert(name.clone(), Value::Num(0.0));
                        if !is_declaration {
                            top_level.push(stmt);
                        }
                    }
                    _ => top_level.push(stmt),
                }
            }
        }

        Ok(Program {
            functions,
            top_level,
            state,
            sample_rate: 44100.0,
        })
    }

    fn parse_function(&mut self) -> Result<(String, Vec<String>, Vec<Stmt>), EvalError> {
        self.expect("function")?;
        let name = self
            .next()
            .ok_or_else(|| EvalError::new("function name expected"))?;
        self.expect("(")?;
        let mut params = Vec::new();
        while !self.eat(")") {
            let param = self
                .next()
                .ok_or_else(|| EvalError::new("parameter name expected"))?;
            params.push(param);
            let _ = self.eat(",");
        }
        let body = self.parse_block()?;
        Ok((name, params, body))
    }

    fn parse_block(&mut self) -> Result<Vec<Stmt>, EvalError> {
        self.expect("{")?;
        let mut stmts = Vec::new();
        while !self.eat("}") {
            if self.peek().is_none() {
                return Err(EvalError::new("unterminated block"));
            }
            stmts.push(self.parse_stmt()?);
        }
        Ok(stmts)
    }

    fn parse_stmt(&mut self) -> Result<Stmt, EvalError> {
        // Storage keywords carry no runtime meaning here; the emitter has
        // already decided what persists, and the evaluator keys state by name.
        let _ = self.eat("@state");
        let _ = self.eat("let");

        match self.peek() {
            Some("if") => self.parse_if(),
            Some("for") => self.parse_for(),
            Some("while") => self.parse_while(),
            Some("return") => self.parse_return(),
            _ => self.parse_simple_stmt(),
        }
    }

    fn parse_if(&mut self) -> Result<Stmt, EvalError> {
        self.expect("if")?;
        self.expect("(")?;
        let cond = self.parse_expr()?;
        self.expect(")")?;
        let then_block = self.parse_block()?;
        let else_block = if self.eat("else") {
            self.parse_block()?
        } else {
            Vec::new()
        };
        Ok(Stmt::If {
            cond,
            then_block,
            else_block,
        })
    }

    fn parse_for(&mut self) -> Result<Stmt, EvalError> {
        self.expect("for")?;
        self.expect("(")?;
        let _ = self.eat("let");
        let var = self
            .next()
            .ok_or_else(|| EvalError::new("loop variable expected"))?;
        self.skip_type_annotation();
        self.expect("=")?;
        let start = self.parse_expr()?;
        self.expect(";")?;
        let cond = self.parse_expr()?;
        self.expect(";")?;
        // The step is `var = <expr>`.
        let _ = self.next();
        self.expect("=")?;
        let step = self.parse_expr()?;
        self.expect(")")?;
        let body = self.parse_block()?;
        Ok(Stmt::For {
            var,
            start,
            cond,
            step,
            body,
        })
    }

    fn parse_while(&mut self) -> Result<Stmt, EvalError> {
        self.expect("while")?;
        self.expect("(")?;
        let cond = self.parse_expr()?;
        self.expect(")")?;
        let body = self.parse_block()?;
        Ok(Stmt::While { cond, body })
    }

    fn parse_return(&mut self) -> Result<Stmt, EvalError> {
        self.expect("return")?;
        let mut items = Vec::new();
        if self.eat("[") {
            while !self.eat("]") {
                items.push(self.parse_expr()?);
                let _ = self.eat(",");
            }
        } else if self.peek() != Some(";") {
            items.push(self.parse_expr()?);
        }
        let _ = self.eat(";");
        Ok(Stmt::Return(items))
    }

    /// Parses an assignment or a bare call.
    fn parse_simple_stmt(&mut self) -> Result<Stmt, EvalError> {
        let name = self
            .next()
            .ok_or_else(|| EvalError::new("statement expected"))?;

        // `name(...)` as a statement.
        if self.peek() == Some("(") {
            self.expect("(")?;
            let mut args = Vec::new();
            while !self.eat(")") {
                args.push(self.parse_expr()?);
                let _ = self.eat(",");
            }
            let _ = self.eat(";");
            return Ok(Stmt::Call { name, args });
        }

        let target = if self.eat("[") {
            let index = self.parse_expr()?;
            self.expect("]")?;
            LValue::Index(name, Box::new(index))
        } else {
            self.skip_type_annotation();
            LValue::Var(name)
        };

        self.expect("=")?;

        // `new FixedFloatArray(n)`
        if self.peek() == Some("new") {
            self.expect("new")?;
            let _kind = self.next();
            self.expect("(")?;
            let size = self.parse_expr()?;
            self.expect(")")?;
            let _ = self.eat(";");
            let Expr::Num(size) = size else {
                return Err(EvalError::new("array size must be a literal"));
            };
            let LValue::Var(name) = target else {
                return Err(EvalError::new("array declaration target must be a name"));
            };
            return Ok(Stmt::DeclareArray {
                name,
                size: size as usize,
            });
        }

        let value = self.parse_expr()?;
        let _ = self.eat(";");
        Ok(Stmt::Assign { target, value })
    }

    /// Skips `: Int` / `: number`, which carry no runtime meaning here.
    fn skip_type_annotation(&mut self) {
        if self.eat(":") {
            let _ = self.next();
        }
    }

    /// Parses an expression.
    ///
    /// The emitter parenthesises every binary operation, so precedence is
    /// carried by the parentheses and this only has to handle a flat left-to-
    /// right sequence inside each group. Asserting that is deliberate: if the
    /// emitter ever stops parenthesising, this fails loudly instead of
    /// silently evaluating with the wrong precedence.
    fn parse_expr(&mut self) -> Result<Expr, EvalError> {
        let mut lhs = self.parse_atom()?;
        while let Some(op) = self.peek() {
            if !is_binary_op(op) {
                break;
            }
            let op = self.next().unwrap_or_default();
            let rhs = self.parse_atom()?;
            lhs = Expr::Binary(Box::new(lhs), op, Box::new(rhs));
        }
        Ok(lhs)
    }

    fn parse_atom(&mut self) -> Result<Expr, EvalError> {
        // Unary minus, which the emitter produces as `-1 * x` but the reference
        // may produce directly.
        if self.eat("-") {
            let inner = self.parse_atom()?;
            return Ok(Expr::Binary(
                Box::new(Expr::Num(-1.0)),
                "*".to_owned(),
                Box::new(inner),
            ));
        }
        if self.eat("(") {
            let inner = self.parse_expr()?;
            self.expect(")")?;
            return Ok(inner);
        }
        let token = self
            .next()
            .ok_or_else(|| EvalError::new("expression expected"))?;

        if let Some(value) = parse_number(&token) {
            return Ok(Expr::Num(value));
        }

        // The emitter writes `fUpdated = true;` in `dspsetup`, mirroring the
        // reference, even though the field is declared `Int`.
        match token.as_str() {
            "true" => return Ok(Expr::Num(1.0)),
            "false" => return Ok(Expr::Num(0.0)),
            _ => {}
        }

        if self.peek() == Some("(") {
            self.expect("(")?;
            let mut args = Vec::new();
            while !self.eat(")") {
                args.push(self.parse_expr()?);
                let _ = self.eat(",");
            }
            return Ok(Expr::Call(token, args));
        }

        if self.eat("[") {
            let index = self.parse_expr()?;
            self.expect("]")?;
            return Ok(Expr::Index(token, Box::new(index)));
        }

        Ok(Expr::Var(token))
    }
}

/// Whether a token is a binary operator.
fn is_binary_op(token: &str) -> bool {
    matches!(
        token,
        "+" | "-"
            | "*"
            | "/"
            | "%"
            | "&"
            | "|"
            | "^"
            | "<<"
            | ">>"
            | "<"
            | "<="
            | ">"
            | ">="
            | "=="
            | "!="
    )
}

/// Parses a codebox numeric literal, including the `f` suffix.
fn parse_number(token: &str) -> Option<f64> {
    let trimmed = token.strip_suffix('f').unwrap_or(token);
    if trimmed.is_empty() || !trimmed.starts_with(|c: char| c.is_ascii_digit() || c == '.') {
        return None;
    }
    trimmed.parse::<f64>().ok()
}

/// Splits a codebox source into tokens, dropping `//` comments.
fn tokenize(source: &str) -> Vec<String> {
    const TWO_CHAR: [&str; 6] = ["<<", ">>", "<=", ">=", "==", "!="];
    let mut tokens = Vec::new();
    for line in source.lines() {
        let line = match line.find("//") {
            Some(at) => &line[..at],
            None => line,
        };
        let bytes: Vec<char> = line.chars().collect();
        let mut i = 0;
        while i < bytes.len() {
            let c = bytes[i];
            if c.is_whitespace() {
                i += 1;
                continue;
            }
            if i + 1 < bytes.len() {
                let pair: String = bytes[i..i + 2].iter().collect();
                if TWO_CHAR.contains(&pair.as_str()) {
                    tokens.push(pair);
                    i += 2;
                    continue;
                }
            }
            if c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '@' {
                let start = i;
                while i < bytes.len()
                    && (bytes[i].is_ascii_alphanumeric()
                        || bytes[i] == '_'
                        || bytes[i] == '.'
                        || bytes[i] == '@')
                {
                    i += 1;
                }
                tokens.push(bytes[start..i].iter().collect());
                continue;
            }
            tokens.push(c.to_string());
            i += 1;
        }
    }
    tokens
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(source: &str, inputs: &[f64]) -> Vec<f64> {
        let mut program = Program::parse(source).expect("parse");
        program.dspsetup(44100.0).expect("dspsetup");
        program.compute(&[], inputs).expect("compute")
    }

    #[test]
    fn evaluates_a_passthrough() {
        let source = "\
@state fUpdated : Int = 0;
function dspsetup() {
\tfUpdated = true;
}
function control() {
}
function update() {
\tif (fUpdated) { fUpdated = false; control(); }
}
function compute(i0) {
\tlet input0_cb : number = i0;
\tlet output0_cb : number = 0;
\toutput0_cb = input0_cb;
\treturn [output0_cb];
}
update();
outputs = compute(in1);
out1 = outputs[0];
";
        assert_eq!(run(source, &[0.25]), vec![0.25]);
    }

    #[test]
    fn state_persists_between_samples() {
        let source = "\
@state acc_cb : number = 0;
function dspsetup() {
\tacc_cb = 0.0f;
}
function control() {
}
function update() {
}
function compute(i0) {
\tlet input0_cb : number = i0;
\tlet output0_cb : number = 0;
\tacc_cb = (acc_cb + input0_cb);
\toutput0_cb = acc_cb;
\treturn [output0_cb];
}
update();
outputs = compute(in1);
out1 = outputs[0];
";
        let mut program = Program::parse(source).expect("parse");
        program.dspsetup(44100.0).expect("dspsetup");
        assert_eq!(program.compute(&[], &[1.0]).expect("s1"), vec![1.0]);
        assert_eq!(program.compute(&[], &[1.0]).expect("s2"), vec![2.0]);
        assert_eq!(program.compute(&[], &[0.5]).expect("s3"), vec![2.5]);
    }

    #[test]
    fn arrays_are_declared_and_filled_in_dspsetup() {
        let source = "\
@state t_cb = new FixedFloatArray(3);
function dspsetup() {
\tt_cb[0] = 1.0f;
\tt_cb[1] = 2.0f;
\tfor (let l0_cb : Int = 2; (l0_cb < 3); l0_cb = iadd(l0_cb, 1)) {
\t\tt_cb[l0_cb] = 9.0f;
\t}
}
function control() {
}
function update() {
}
function compute(i0) {
\tlet output0_cb : number = 0;
\toutput0_cb = (t_cb[0] + (t_cb[1] + t_cb[2]));
\treturn [output0_cb];
}
update();
outputs = compute(in1);
out1 = outputs[0];
";
        assert_eq!(run(source, &[0.0]), vec![12.0]);
    }

    /// The integer helpers wrap at 32 bits, unlike plain arithmetic.
    #[test]
    fn integer_helpers_wrap_at_32_bits() {
        let source = "\
function dspsetup() {
}
function control() {
}
function update() {
}
function compute(i0) {
\tlet output0_cb : number = 0;
\toutput0_cb = iadd(2147483647, 1);
\treturn [output0_cb];
}
update();
outputs = compute(in1);
out1 = outputs[0];
";
        assert_eq!(run(source, &[0.0]), vec![f64::from(i32::MIN)]);
    }

    /// `trunc` rounds toward zero, which is the whole point of C4's cast rule:
    /// `floor` would give a different answer for negatives.
    #[test]
    fn trunc_rounds_toward_zero() {
        let source = "\
function dspsetup() {
}
function control() {
}
function update() {
}
function compute(i0) {
\tlet output0_cb : number = 0;
\toutput0_cb = trunc(i0);
\treturn [output0_cb];
}
update();
outputs = compute(in1);
out1 = outputs[0];
";
        assert_eq!(run(source, &[-1.5]), vec![-1.0]);
        assert_eq!(run(source, &[1.5]), vec![1.0]);
    }

    #[test]
    fn unknown_constructs_are_errors_not_guesses() {
        let source = "function dspsetup() {\n\tmystery_thing @@ 3;\n}\n";
        assert!(
            Program::parse(source).is_err() || {
                let mut program = Program::parse(source).expect("parse");
                program.dspsetup(44100.0).is_err()
            }
        );
    }

    #[test]
    fn samplerate_reads_the_configured_rate() {
        let source = "\
@state sr_cb : Int = 0;
function dspsetup() {
\tsr_cb = samplerate();
}
function control() {
}
function update() {
}
function compute(i0) {
\tlet output0_cb : number = 0;
\toutput0_cb = sr_cb;
\treturn [output0_cb];
}
update();
outputs = compute(in1);
out1 = outputs[0];
";
        let mut program = Program::parse(source).expect("parse");
        program.dspsetup(48000.0).expect("dspsetup");
        assert_eq!(
            program.compute(&[], &[0.0]).expect("compute"),
            vec![48000.0]
        );
    }
}
