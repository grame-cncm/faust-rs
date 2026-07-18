/-
  Lean 4 specification for:

    vector-mode-signal-level-analysis-cpp-port-plan-2026-07-10-en.md

  Scope
  -----
  This file mechanizes the finite structural core of section 5:

  * signal decorations and representative typing rules;
  * dependency graphs with consumer -> dependency edges;
  * valid schedule certificates and an executable certificate checker;
  * the four public scheduling strategy tags;
  * the ordered C++ needSeparateLoop decision;
  * effects, placement, epochs, and typed transports;
  * region-aware lowering and fission/simulation proof obligations;
  * lockstep instance bundling: isomorphism modulo leaves and bundle
    obligations (planned schema-v2 extension);
  * abstract delay and repeated-transition semantics.

  It deliberately does not claim a proof of the complete Faust compiler.
  FissionSafe, schedule independence, and scalar/vector simulation are exposed
  as propositions that later implementation-specific developments must prove.

  This file uses only Lean's bundled Std library. It contains no `sorry` and no
  axioms. Validate it with:

      lean porting/vector-mode-scheduling-formal-spec.lean

  How to read this file
  ---------------------
  Lean distinguishes programs from propositions, but both are ordinary terms:

  * definitions returning `Bool`, such as `verifySchedule`, are executable;
  * definitions returning `Prop`, such as `ValidSchedule`, state contracts;
  * a `theorem` supplies a checked proof of a proposition;
  * a proof-valued structure field, such as `Scheduler.sound`, is an obligation
    that every future implementation must fill before Lean accepts it.

  Consequently, the file proves the algebraic and finite checker lemmas that do
  not depend on faust-rs, while representing compiler-specific correctness as
  explicit interfaces. Merely compiling this file does not prove the current
  Rust compiler correct; it proves that this specification is well typed, has
  no admitted facts, and that every theorem body is accepted by Lean's kernel.

  Naming conventions
  ------------------
  Names ending in `B` return `Bool` and can be evaluated. Their proposition-level
  wrappers use equality with `true`. Graph edges always point from a consumer to
  one of its dependencies. Thus a dependency must occur before its consumer in
  a valid execution order.
-/

import Std

namespace Faust.VectorScheduling

abbrev SigId := Nat
abbrev LoopId := Nat
abbrev EpochId := Nat
abbrev ClockId := Nat

/-
  Identifiers are natural numbers because the porting plan requires stable,
  deterministic allocation. They are aliases, not distinct runtime wrappers:
  the model focuses on semantics rather than Rust representation details.
-/

/-! ## Analysis domains -/

/-
  `Rate` is the three-point variability lattice used by the signal analysis:

      Konst < Block < Samp

  `join` computes the least upper bound. The three following theorems establish
  the semilattice laws needed to combine operand analyses independently of
  traversal grouping.
-/

inductive Rate where
  | konst
  | block
  | samp
  deriving Repr, DecidableEq, BEq

namespace Rate

def join : Rate -> Rate -> Rate
  | .samp, _ => .samp
  | _, .samp => .samp
  | .block, _ => .block
  | _, .block => .block
  | .konst, .konst => .konst

def isSlow : Rate -> Bool
  | .samp => false
  | _ => true

theorem join_comm (a b : Rate) : join a b = join b a := by
  cases a <;> cases b <;> rfl

theorem join_assoc (a b c : Rate) : join (join a b) c = join a (join b c) := by
  cases a <;> cases b <;> cases c <;> rfl

theorem join_idem (a : Rate) : join a a = a := by
  cases a <;> rfl

end Rate

inductive Vectorability where
  | vect
  | scal
  | trueScal
  deriving Repr, DecidableEq, BEq

namespace Vectorability

/-
  Vectorability is ordered from least to most restrictive:

      Vect < Scal < TrueScal

  A parent inherits the strongest restriction of its children. `TrueScal`
  denotes an operation whose semantics intrinsically require scalar execution;
  `Scal` may still participate in a serial island inside a vector plan.
-/

def join : Vectorability -> Vectorability -> Vectorability
  | .trueScal, _ => .trueScal
  | _, .trueScal => .trueScal
  | .scal, _ => .scal
  | _, .scal => .scal
  | .vect, .vect => .vect

theorem join_comm (a b : Vectorability) : join a b = join b a := by
  cases a <;> cases b <;> rfl

theorem join_assoc (a b c : Vectorability) :
    join (join a b) c = join a (join b c) := by
  cases a <;> cases b <;> cases c <;> rfl

end Vectorability

inductive ValueTy where
  | int
  | real
  | tuple (components : List ValueTy)
  deriving Repr

def promoteNumeric : ValueTy -> ValueTy -> Option ValueTy
  | .int, .int => some .int
  | .int, .real => some .real
  | .real, .int => some .real
  | .real, .real => some .real
  | _, _ => none

/-
  This deliberately small value-type universe is sufficient to state the
  scheduling invariants. `none` makes an invalid numeric combination explicit
  instead of silently inventing a coercion. Backend-specific widths and
  `FaustFloat` lowering belong to the FIR refinement of this model.
-/

inductive Purity where
  | pure
  | impure
  | unknown
  deriving Repr, DecidableEq, BEq

inductive Resource where
  | state (id : Nat)
  | table (id : Nat)
  | ui (id : Nat)
  | output (channel : Nat)
  | foreign (name : String)
  deriving Repr, DecidableEq, BEq

inductive Effect where
  | read (resource : Resource)
  | write (resource : Resource)
  | foreignCall (name : String) (purity : Purity)
  deriving Repr, DecidableEq, BEq

namespace Effect

/-
  `conflicts a b` is a conservative dependence test. Two reads commute. Accesses
  to different resources commute. A write conflicts with another access to the
  same resource. Pure foreign calls add no ordering edge, while impure or unknown
  calls conflict conservatively because the model cannot inspect their effects.
-/

def conflicts : Effect -> Effect -> Bool
  | .foreignCall _ .pure, _ => false
  | _, .foreignCall _ .pure => false
  | .foreignCall _ _, _ => true
  | _, .foreignCall _ _ => true
  | .read _, .read _ => false
  | .read a, .write b => decide (a = b)
  | .write a, .read b => decide (a = b)
  | .write a, .write b => decide (a = b)

/-
  `conflicts` is used as a symmetric commutation test, so its symmetry is a
  required property rather than an incidental one. The resource comparison uses
  `decide (a = b)` on `DecidableEq Resource`, which makes the symmetry provable
  from `eq_comm` instead of relying on an unproved `BEq` law.
-/

theorem decide_eq_symm {α : Type} [DecidableEq α] (a b : α) :
    decide (a = b) = decide (b = a) := by
  by_cases h : a = b
  · subst h; rfl
  · rw [decide_eq_false h, decide_eq_false (fun h' => h h'.symm)]

theorem conflicts_symm (a b : Effect) : conflicts a b = conflicts b a := by
  cases a with
  | read ra =>
    cases b with
    | read rb => rfl
    | write rb => simp only [conflicts]; exact decide_eq_symm ra rb
    | foreignCall nb pb => cases pb <;> rfl
  | write ra =>
    cases b with
    | read rb => simp only [conflicts]; exact decide_eq_symm ra rb
    | write rb => simp only [conflicts]; exact decide_eq_symm ra rb
    | foreignCall nb pb => cases pb <;> rfl
  | foreignCall na pa =>
    cases pa <;>
      cases b with
      | read rb => rfl
      | write rb => rfl
      | foreignCall nb pb => cases pb <;> rfl

end Effect

def effectsCommute (left right : List Effect) : Bool :=
  left.all fun a => right.all fun b => !(Effect.conflicts a b)

/-
  Pairwise commutation is the condition for leaving two incomparable loops
  unordered. If this check is false, planning must add an effect edge, merge the
  operations, or place them in a serial island.
-/

example : effectsCommute
    [.read (.table 0)] [.read (.table 0)] = true := by
  decide

example : effectsCommute
    [.read (.table 0)] [.write (.table 0)] = false := by
  decide

structure Decoration where
  valueTy : ValueTy
  rate : Rate
  vectorability : Vectorability
  clock : ClockId
  effects : List Effect
  deriving Repr

/-
  A decoration is the product judgment attached to a prepared signal. Keeping
  type, rate, vectorability, clock, and effects together prevents later passes
  from making locally inconsistent scheduling decisions.
-/

/-! ## Representative signal typing judgment -/

/-
  `HasType expression decoration` is an inductively generated judgment. A value
  exists only when one constructor proves it. These constructors are a compact,
  representative subset of Faust signals rather than a replacement AST:

  * literals and inputs introduce base facts;
  * `bin` joins rates and vectorability after checking clocks and promotion;
  * `delay` records both the state read and state write;
  * `proj` preserves recursion-group effects but forces scalar treatment;
  * `clocked` changes only the explicit clock domain.

  Adding a real signal node requires adding a constructor whose conclusion
  states all five decoration components, making omissions visible to Lean.
-/

inductive Expr where
  | intLit (value : Int)
  | realLit (value : Float)
  | input (channel : Nat)
  | bin (left right : Expr)
  | delay (value amount : Expr) (state : Resource)
  | proj (index : Nat) (group : Expr)
  | clocked (clock : ClockId) (value : Expr)
  deriving Repr

/-
  Base signals live in a fixed root clock domain. This is what makes the typing
  judgment a total *function* of the expression: earlier drafts let each literal
  and input carry a free `clock`, so a single expression admitted many judgments
  and the companion document's `Totality` claim ("exactly one judgment exists")
  was false. `Clocked` is the sole clock-changing constructor, and
  `hasType_functional` below now proves determinism.
-/
def rootClock : ClockId := 0

inductive HasType : Expr -> Decoration -> Prop where
  | intLit (value : Int) :
      HasType (.intLit value)
        { valueTy := .int
          rate := .konst
          vectorability := .vect
          clock := rootClock
          effects := [] }
  | realLit (value : Float) :
      HasType (.realLit value)
        { valueTy := .real
          rate := .konst
          vectorability := .vect
          clock := rootClock
          effects := [] }
  | input (channel : Nat) :
      HasType (.input channel)
        { valueTy := .real
          rate := .samp
          vectorability := .vect
          clock := rootClock
          effects := [] }
  | bin {left right : Expr} {dl dr : Decoration} {resultTy : ValueTy}
      (leftTyped : HasType left dl)
      (rightTyped : HasType right dr)
      (sameClock : dl.clock = dr.clock)
      (promoted : promoteNumeric dl.valueTy dr.valueTy = some resultTy) :
      HasType (.bin left right)
        { valueTy := resultTy
          rate := Rate.join dl.rate dr.rate
          vectorability := Vectorability.join dl.vectorability dr.vectorability
          clock := dl.clock
          effects := dl.effects ++ dr.effects }
  | delay {value amount : Expr} {dv da : Decoration} (state : Resource)
      (valueTyped : HasType value dv)
      (amountTyped : HasType amount da)
      (amountIsInt : da.valueTy = .int)
      (sameClock : dv.clock = da.clock) :
      HasType (.delay value amount state)
        { valueTy := dv.valueTy
          rate := Rate.join dv.rate da.rate
          vectorability := Vectorability.join dv.vectorability da.vectorability
          clock := dv.clock
          effects := dv.effects ++ da.effects ++ [.read state, .write state] }
  | proj {index : Nat} {group : Expr} {dg : Decoration}
      {components : List ValueTy} {component : ValueTy}
      (groupTyped : HasType group dg)
      (groupIsTuple : dg.valueTy = .tuple components)
      (componentAt : components[index]? = some component) :
      HasType (.proj index group)
        { valueTy := component
          rate := .samp
          vectorability := .scal
          clock := dg.clock
          effects := dg.effects }
  | clocked {value : Expr} {d : Decoration} (clock : ClockId)
      (valueTyped : HasType value d) :
      HasType (.clocked clock value) { d with clock := clock }

/-
  `delay` deliberately over-approximates: it emits `ReadState`/`WriteState` for
  every delay, including the `n = 0` case the companion document says "may
  discharge its state effects." Over-approximation is the sound direction for
  scheduling (it can only add ordering constraints), and this constructor cannot
  inspect the amount, so the discharge is left to the concrete delay analysis.

  `hasType_functional` establishes the `Totality`/uniqueness property: the
  representative judgment assigns at most one decoration to any expression. The
  proof inducts on the first derivation and inverts the second; each expression
  head admits a single constructor, and every decoration component is fixed
  either by a sub-derivation (via the induction hypothesis) or by a function
  (`promoteNumeric`, list projection).
-/

theorem hasType_functional {e : Expr} {d₁ d₂ : Decoration}
    (h₁ : HasType e d₁) (h₂ : HasType e d₂) : d₁ = d₂ := by
  induction h₁ generalizing d₂ with
  | intLit => cases h₂; rfl
  | realLit => cases h₂; rfl
  | input => cases h₂; rfl
  | bin _ _ _ promoted ihl ihr =>
    cases h₂ with
    | bin _ _ _ promoted₂ =>
      have hl := ihl ‹_›
      have hr := ihr ‹_›
      subst hl; subst hr
      rw [promoted] at promoted₂
      cases promoted₂
      rfl
  | delay _ _ _ _ _ ihv iha =>
    cases h₂ with
    | delay _ _ _ _ _ =>
      have hv := ihv ‹_›
      have ha := iha ‹_›
      subst hv; subst ha
      rfl
  | proj _ groupIsTuple componentAt ih =>
    cases h₂ with
    | proj _ groupIsTuple₂ componentAt₂ =>
      have hg := ih ‹_›
      subst hg
      rw [groupIsTuple] at groupIsTuple₂
      cases groupIsTuple₂
      rw [componentAt] at componentAt₂
      cases componentAt₂
      rfl
  | clocked _ _ ih =>
    cases h₂ with
    | clocked _ _ =>
      have hd := ih ‹_›
      subst hd
      rfl

/-! ### Structural isomorphism modulo leaves (lockstep prerequisite) -/

/-
  Lockstep instance bundling runs k serial computations in SIMD lanes, one lane
  per instance. Its structural precondition is that the k bodies are the *same
  computation up to the leaves*: literal values, input channels, and state
  resources may differ per lane, while the constructor skeleton, projection
  indices, and clock annotations must match.

  Rather than defining a binary relation and proving it is an equivalence, the
  skeleton is extracted as a first-class value: `Expr.shape` erases exactly the
  leaf payloads. Isomorphism is then *equality of shapes*, so reflexivity,
  symmetry, and transitivity are inherited from `Eq` for free, and the
  executable checker is decidable equality on `Shape`. This also mirrors the
  planned Rust implementation, which detects bundling candidates with a shape
  hash over prepared signals.
-/

inductive Shape where
  | intLit
  | realLit
  | input
  | bin (left right : Shape)
  | delay (value amount : Shape)
  | proj (index : Nat) (group : Shape)
  | clocked (clock : ClockId) (value : Shape)
  deriving Repr, DecidableEq, BEq

def Expr.shape : Expr -> Shape
  | .intLit _ => .intLit
  | .realLit _ => .realLit
  | .input _ => .input
  | .bin l r => .bin l.shape r.shape
  | .delay v n _ => .delay v.shape n.shape
  | .proj i g => .proj i g.shape
  | .clocked c v => .clocked c v.shape

def IsoModuloLeaves (a b : Expr) : Prop := a.shape = b.shape

/-- Executable form of the isomorphism test (the reference for the Rust
    shape-hash detector: hash collisions must be confirmed by this equality). -/
def isoB (a b : Expr) : Bool := decide (a.shape = b.shape)

theorem isoB_iff (a b : Expr) : isoB a b = true ↔ IsoModuloLeaves a b := by
  simp [isoB, IsoModuloLeaves]

theorem iso_refl (e : Expr) : IsoModuloLeaves e e := rfl

theorem iso_symm {a b : Expr} (h : IsoModuloLeaves a b) : IsoModuloLeaves b a :=
  h.symm

theorem iso_trans {a b c : Expr} (hab : IsoModuloLeaves a b)
    (hbc : IsoModuloLeaves b c) : IsoModuloLeaves a c :=
  hab.trans hbc

/-
  `iso_decorations_agree` is the semantic payoff of the definition: isomorphic
  lanes receive the same value type, rate, vectorability, and clock. This is
  what lets one bundle share a single transport element type, rate analysis,
  and clock placement across lanes. Effects are deliberately *not* claimed
  equal: each lane touches its own state resources, and lockstep legality
  handles effects separately through pairwise commutation.
-/

theorem iso_decorations_agree {a b : Expr} {da db : Decoration}
    (ha : HasType a da) (hb : HasType b db) (iso : IsoModuloLeaves a b) :
    da.valueTy = db.valueTy ∧ da.rate = db.rate
      ∧ da.vectorability = db.vectorability ∧ da.clock = db.clock := by
  unfold IsoModuloLeaves at iso
  induction ha generalizing b db with
  | intLit v =>
    cases b <;> simp [Expr.shape] at iso
    cases hb
    exact ⟨rfl, rfl, rfl, rfl⟩
  | realLit v =>
    cases b <;> simp [Expr.shape] at iso
    cases hb
    exact ⟨rfl, rfl, rfl, rfl⟩
  | input channel =>
    cases b <;> simp [Expr.shape] at iso
    cases hb
    exact ⟨rfl, rfl, rfl, rfl⟩
  | bin _ _ _ promoted ihl ihr =>
    cases b <;> simp [Expr.shape] at iso
    cases hb with
    | @bin _ _ dl' dr' resultTy' lT' rT' _ promoted' =>
      obtain ⟨isoL, isoR⟩ := iso
      obtain ⟨vl, rl, xl, cl⟩ := ihl lT' isoL
      obtain ⟨vr, rr, xr, _⟩ := ihr rT' isoR
      rw [vl, vr] at promoted
      rw [promoted] at promoted'
      cases promoted'
      exact ⟨rfl, by simp [rl, rr], by simp [xl, xr], by simp [cl]⟩
  | delay state _ _ _ _ ihv iha =>
    cases b <;> simp [Expr.shape] at iso
    cases hb with
    | @delay _ _ dv' da' state' vT' aT' _ _ =>
      obtain ⟨isoV, isoA⟩ := iso
      obtain ⟨vv, rv, xv, cv⟩ := ihv vT' isoV
      obtain ⟨_, ra, xa, _⟩ := iha aT' isoA
      exact ⟨vv, by simp [rv, ra], by simp [xv, xa], cv⟩
  | proj groupTyped groupIsTuple componentAt ih =>
    cases b <;> simp [Expr.shape] at iso
    cases hb with
    | @proj _ _ dg' components' component' gT' groupIsTuple' componentAt' =>
      obtain ⟨isoIdx, isoG⟩ := iso
      obtain ⟨vg, _, _, cg⟩ := ih gT' isoG
      rw [groupIsTuple, groupIsTuple'] at vg
      cases vg
      subst isoIdx
      rw [componentAt] at componentAt'
      cases componentAt'
      exact ⟨rfl, rfl, rfl, cg⟩
  | clocked clock _ ih =>
    cases b <;> simp [Expr.shape] at iso
    cases hb with
    | clocked _ vT' =>
      obtain ⟨isoC, isoV⟩ := iso
      obtain ⟨vv, rv, xv, _⟩ := ih vT' isoV
      subst isoC
      exact ⟨vv, rv, xv, rfl⟩

/-
  `Expr` and `HasType` document representative rules; faust-rs must not rebuild
  its production forest in this miniature AST. The structures below are the
  actual cross-language boundary: one record is exported for every reachable
  prepared `SigId`, together with the labelled dependencies used by planning.
-/

structure SignalDecorationRecord where
  signal : SigId
  decoration : Decoration
  executionCondition : Nat
  maxDelay : Nat
  delayRead : Bool
  recursiveProjection : Bool
  multipleOccurrences : Bool
  verySimple : Bool
  deriving Repr

inductive AnalysisDependencyKind where
  | immediate
  | delayed (amount : Nat)
  | control
  | clockBoundary
  | effect (atom : Effect)
  deriving Repr

structure AnalysisDependency where
  consumer : SigId
  kind : AnalysisDependencyKind
  dependency : SigId
  deriving Repr

def AllAnalysisEndpoints (signals : List SigId) :
    List AnalysisDependency -> Prop
  | [] => True
  | edge :: rest =>
      edge.consumer ∈ signals ∧ edge.dependency ∈ signals
        ∧ AllAnalysisEndpoints signals rest

structure DecorationCertificate (reachable : List SigId) where
  records : List SignalDecorationRecord
  dependencies : List AnalysisDependency
  recordsNodup : (records.map (fun record => record.signal)).Nodup
  recordsCover : (records.map (fun record => record.signal)).Perm reachable
  dependencyEndpoints : AllAnalysisEndpoints reachable dependencies

/-
  A concrete `verifyDecorations` must additionally validate each record against
  the authoritative type, clock, occurrence, delay, and effect analyses. Those
  compiler-specific maps are intentionally parameters of the future checker,
  not axioms asserted by this standalone specification.
-/

/-! ## Dependency graph and schedule certificates -/

/-
  `DependencyGraph Node` is finite because `nodes` is a list. For a consumer
  `u`, `dependencies u` lists every `v` required by `u`; this is the C++ Faust
  edge convention `u -> v`. A well-formed adapter must list every dependency in
  `nodes`. The schedule checker detects an omitted endpoint because such a node
  cannot be found in the proposed order.

  `BEq` provides executable equality and `LawfulBEq` proves that this Boolean
  equality agrees with Lean equality. Requiring both avoids certificates built
  with a pathological comparison function.
-/

structure DependencyGraph (Node : Type) where
  nodes : List Node
  dependencies : Node -> List Node

def position? [BEq Node] [LawfulBEq Node] (needle : Node) : List Node -> Option Nat
  | [] => none
  | item :: rest =>
      if item == needle then
        some 0
      else
        match position? needle rest with
        | none => none
        | some index => some (index + 1)

/- `position?` is intentionally total: an absent node returns `none`. -/

def beforeB [BEq Node] [LawfulBEq Node]
    (order : List Node) (first second : Node) : Bool :=
  match position? first order, position? second order with
  | some firstIndex, some secondIndex => firstIndex < secondIndex
  | _, _ => false

def Before [BEq Node] [LawfulBEq Node]
    (order : List Node) (first second : Node) : Prop :=
  beforeB order first second = true

def noDuplicatesB [BEq Node] [LawfulBEq Node] : List Node -> Bool
  | [] => true
  | node :: rest => !rest.contains node && noDuplicatesB rest

/-
  A schedule covers a graph exactly when it is a duplicate-free permutation of
  `graph.nodes`. The two `all` clauses check both inclusions instead of assuming
  that equal list lengths imply equality of finite sets.

  `noDuplicatesB graph.nodes` is required as well: without it a graph whose node
  list contained duplicates would be "covered" by its deduplicated order, so the
  reference checker would be strictly weaker than the Rust R1 checker (which
  demands unique nodes). Both inclusions plus both `Nodup` facts give a genuine
  set bijection, matched below by `CoversRel`.
-/

def coversB [BEq Node] [LawfulBEq Node]
    (graph : DependencyGraph Node) (order : List Node) : Bool :=
  noDuplicatesB graph.nodes
    && noDuplicatesB order
    && graph.nodes.all (fun node => order.contains node)
    && order.all (fun node => graph.nodes.contains node)

def respectsDependenciesB [BEq Node] [LawfulBEq Node] (graph : DependencyGraph Node)
    (order : List Node) : Bool :=
  graph.nodes.all fun consumer =>
    (graph.dependencies consumer).all fun dependency =>
      beforeB order dependency consumer

/-
  This is the executable form of

      forall consumer, dependency in deps(consumer),
        position(dependency) < position(consumer).

  `validScheduleB` combines that causal condition with permutation coverage.
-/

def validScheduleB [BEq Node] [LawfulBEq Node] (graph : DependencyGraph Node)
    (order : List Node) : Bool :=
  coversB graph order && respectsDependenciesB graph order

def ValidSchedule [BEq Node] [LawfulBEq Node] (graph : DependencyGraph Node)
    (order : List Node) : Prop :=
  validScheduleB graph order = true

def verifySchedule [BEq Node] [LawfulBEq Node] (graph : DependencyGraph Node)
    (order : List Node) : Bool :=
  validScheduleB graph order

theorem verifySchedule_eq_true_iff [BEq Node] [LawfulBEq Node]
    (graph : DependencyGraph Node) (order : List Node) :
    verifySchedule graph order = true ↔ ValidSchedule graph order := by
  rfl

/-
  The theorem above is reflexive because `ValidSchedule` is *defined* as the
  checker returning `true`; on its own it says nothing about topological order.
  The predicates and lemmas below supply the missing semantic anchor: an
  independently stated relational meaning of "valid schedule", and a proof that
  the Boolean checker is both sound and complete against it. This is what makes
  `verifySchedule` a trustworthy reference oracle rather than a tautology.
-/

/-- Relational meaning of coverage: both lists are duplicate-free and denote the
    same finite set. Stated without reference to the Boolean checker. -/
def CoversRel [BEq Node] [LawfulBEq Node]
    (graph : DependencyGraph Node) (order : List Node) : Prop :=
  graph.nodes.Nodup ∧ order.Nodup
    ∧ (∀ n ∈ graph.nodes, n ∈ order)
    ∧ (∀ n ∈ order, n ∈ graph.nodes)

/-- Relational meaning of causality: every dependency precedes its consumer in
    the order. `Before` is the position comparison, stated over the order. -/
def RespectsDepsRel [BEq Node] [LawfulBEq Node]
    (graph : DependencyGraph Node) (order : List Node) : Prop :=
  ∀ consumer ∈ graph.nodes, ∀ dependency ∈ graph.dependencies consumer,
    Before order dependency consumer

/-- The independent semantic notion of a valid topological schedule. -/
def ValidScheduleRel [BEq Node] [LawfulBEq Node]
    (graph : DependencyGraph Node) (order : List Node) : Prop :=
  CoversRel graph order ∧ RespectsDepsRel graph order

theorem noDuplicatesB_iff_nodup [BEq Node] [LawfulBEq Node] (l : List Node) :
    noDuplicatesB l = true ↔ l.Nodup := by
  induction l with
  | nil => simp [noDuplicatesB]
  | cons a t ih =>
    rw [noDuplicatesB, Bool.and_eq_true, Bool.not_eq_true', List.nodup_cons, ih]
    simp

theorem coversB_iff [BEq Node] [LawfulBEq Node]
    (graph : DependencyGraph Node) (order : List Node) :
    coversB graph order = true ↔ CoversRel graph order := by
  rw [coversB]
  simp only [Bool.and_eq_true, noDuplicatesB_iff_nodup, List.all_eq_true,
             List.contains_iff_mem, CoversRel, and_assoc]

theorem respectsDependenciesB_iff [BEq Node] [LawfulBEq Node]
    (graph : DependencyGraph Node) (order : List Node) :
    respectsDependenciesB graph order = true ↔ RespectsDepsRel graph order := by
  rw [respectsDependenciesB]
  simp only [List.all_eq_true, RespectsDepsRel, Before]

/-- Soundness and completeness of the Boolean checker against the independent
    relational specification. Unlike `verifySchedule_eq_true_iff`, this is not a
    `rfl`: it connects the checker to `ValidScheduleRel`, a definition that never
    mentions `validScheduleB`. -/
theorem validScheduleB_iff [BEq Node] [LawfulBEq Node]
    (graph : DependencyGraph Node) (order : List Node) :
    validScheduleB graph order = true ↔ ValidScheduleRel graph order := by
  rw [validScheduleB, Bool.and_eq_true, coversB_iff, respectsDependenciesB_iff]
  exact Iff.rfl

theorem verifySchedule_sound [BEq Node] [LawfulBEq Node]
    {graph : DependencyGraph Node} {order : List Node}
    (accepted : verifySchedule graph order = true) : ValidScheduleRel graph order :=
  (validScheduleB_iff graph order).mp accepted

theorem verifySchedule_complete [BEq Node] [LawfulBEq Node]
    {graph : DependencyGraph Node} {order : List Node}
    (valid : ValidScheduleRel graph order) : verifySchedule graph order = true :=
  (validScheduleB_iff graph order).mpr valid

/-
  This small trusted checker can validate orders produced by a more complicated
  scheduler without reusing that scheduler's algorithm, and `validScheduleB_iff`
  proves the checker's `true` result is exactly the mathematical validity
  predicate.
-/

inductive SchedulingStrategy where
  | depthFirst
  | breadthFirst
  | special
  | reverseBreadthFirst
  deriving Repr, DecidableEq, BEq

/-
  The constructors are the public `-ss` contract:

  * `depthFirst`          = `-ss 0`;
  * `breadthFirst`        = `-ss 1`;
  * `special`             = `-ss 2`;
  * `reverseBreadthFirst` = `-ss n` for every `n >= 3`.

  Strategy-specific order generation is intentionally behind `Scheduler.run`.
  All strategies share the same independently checked validity predicate.
-/

def decodeStrategy : Nat -> SchedulingStrategy
  | 0 => .depthFirst
  | 1 => .breadthFirst
  | 2 => .special
  | _ => .reverseBreadthFirst

example : decodeStrategy 0 = .depthFirst := rfl
example : decodeStrategy 1 = .breadthFirst := rfl
example : decodeStrategy 2 = .special := rfl
example : decodeStrategy 3 = .reverseBreadthFirst := rfl
example : decodeStrategy 42 = .reverseBreadthFirst := rfl

structure ScheduleCertificate [BEq Node] [LawfulBEq Node]
    (graph : DependencyGraph Node) where
  strategy : SchedulingStrategy
  orderedNodes : List Node
  edgeHash : Nat
  valid : ValidSchedule graph orderedNodes

/-
  A certificate couples the candidate order to the exact graph (`edgeHash`) and
  records the requested strategy for diagnostics. Its `valid` field is proof,
  not metadata: a certificate cannot be constructed without satisfying the
  checker. In production, `edgeHash` must be derived deterministically from the
  graph snapshot rather than supplied by an untrusted caller.
-/

def certify? [BEq Node] [LawfulBEq Node] (graph : DependencyGraph Node)
    (strategy : SchedulingStrategy) (edgeHash : Nat) (order : List Node) :
    Option (ScheduleCertificate graph) :=
  if valid : verifySchedule graph order = true then
    some {
      strategy
      orderedNodes := order
      edgeHash
      valid := (verifySchedule_eq_true_iff graph order).mp valid }
  else
    none

/- `certify?` is the trust boundary: invalid candidate orders become `none`. -/

theorem certificate_is_valid [BEq Node] [LawfulBEq Node]
    {graph : DependencyGraph Node}
    (certificate : ScheduleCertificate graph) :
    ValidSchedule graph certificate.orderedNodes :=
  certificate.valid

def HasValidSchedule [BEq Node] [LawfulBEq Node]
    (graph : DependencyGraph Node) : Prop :=
  ∃ order, ValidSchedule graph order

inductive ScheduleError (Node : Type) where
  | cycle (witness : List Node)
  | invalidGraph (message : String)
  deriving Repr

/-
  A scheduler must terminate with either a checked order or a typed error. A
  cycle carries nodes useful for diagnostics; malformed adapters use the second
  case. The full port may strengthen the cycle witness to prove adjacency.
-/

structure Scheduler (Node : Type) [BEq Node] [LawfulBEq Node] where
  run : SchedulingStrategy -> DependencyGraph Node ->
    Except (ScheduleError Node) (List Node)
  sound : ∀ strategy graph order,
    run strategy graph = .ok order -> ValidSchedule graph order
  complete : ∀ strategy graph,
    HasValidSchedule graph -> ∃ order, run strategy graph = .ok order

/-
  The `run` field is a total Lean function, so termination is part of the
  implementation boundary. `sound` and `complete` are the S-Sound and
  S-Complete obligations. Determinism follows from `run` being a function.

  This structure is an L4 refinement target for a future mechanized scheduler;
  it is not a structure that the progressive Rust port must construct. The
  production L2 path is `run candidate -> certify? candidate`: Rust may use an
  ordinary scheduler, but no candidate reaches lowering unless the independent
  checker constructs a `ScheduleCertificate`. Proving `Scheduler.complete`
  concerns the producer and does not change or weaken that trust boundary.
-/

def diamondGraph : DependencyGraph Nat where
  nodes := [0, 1, 2, 3]
  dependencies
    | 3 => [1, 2]
    | 1 => [0]
    | 2 => [0]
    | _ => []

/-
  The diamond is a minimal nontrivial example. Both `[0,1,2,3]` and
  `[0,2,1,3]` are valid because nodes 1 and 2 are incomparable. The order
  `[1,0,2,3]` is rejected because consumer 1 precedes dependency 0.
-/

example : ValidSchedule diamondGraph [0, 1, 2, 3] := by
  rfl

example : ValidSchedule diamondGraph [0, 2, 1, 3] := by
  rfl

example : verifySchedule diamondGraph [1, 0, 2, 3] = false := by
  decide

def diamondCertificate : ScheduleCertificate diamondGraph where
  strategy := .depthFirst
  orderedNodes := [0, 1, 2, 3]
  edgeHash := 0
  valid := by rfl

/-! ## Exact loop-separation decision -/

/-
  `SignalFacts` contains only the inputs used by the C++ `needSeparateLoop`
  decision. The nested `if` expression is normative: it preserves first-match
  priority rather than treating the conditions as an unordered Boolean formula.
  In particular, a positive delayed use always creates a loop, even for a very
  simple or slow signal.
-/

structure SignalFacts where
  maxDelay : Nat
  verySimple : Bool
  rate : Rate
  delayRead : Bool
  recursiveProjection : Bool
  multipleOccurrences : Bool
  deriving Repr, DecidableEq

def separateLoop (facts : SignalFacts) : Bool :=
  if 0 < facts.maxDelay then
    true
  else if facts.verySimple || facts.rate.isSlow then
    false
  else if facts.delayRead then
    false
  else if facts.recursiveProjection then
    true
  else if facts.multipleOccurrences then
    true
  else
    false

def separateLoopFormula (facts : SignalFacts) : Bool :=
  (0 < facts.maxDelay)
    || (!facts.verySimple
      && !facts.rate.isSlow
      && !facts.delayRead
      && (facts.recursiveProjection || facts.multipleOccurrences))

theorem separateLoop_complete (facts : SignalFacts) :
    separateLoop facts = separateLoopFormula facts := by
  rcases facts with ⟨maxDelay, verySimple, rate, delayRead,
    recursiveProjection, multipleOccurrences⟩
  by_cases hasDelay : 0 < maxDelay
  · simp [separateLoop, separateLoopFormula, hasDelay]
  · cases verySimple <;> cases rate <;> cases delayRead <;>
      cases recursiveProjection <;> cases multipleOccurrences <;>
      simp [separateLoop, separateLoopFormula, hasDelay, Rate.isSlow]

theorem maxDelay_dominates (facts : SignalFacts) (hasDelay : 0 < facts.maxDelay) :
    separateLoop facts = true := by
  simp [separateLoop, hasDelay]

theorem verySimple_without_delay_is_inline (facts : SignalFacts)
    (noDelay : facts.maxDelay = 0) (simple : facts.verySimple = true) :
    separateLoop facts = false := by
  simp [separateLoop, noDelay, simple]

/-
  `separateLoop_complete` exhausts the finite Boolean/rate cases after splitting
  `maxDelay` into zero and positive branches. It pins all six priority rows, not
  only the two historically regression-prone examples below it.
-/

example : separateLoop
    { maxDelay := 1
      verySimple := true
      rate := .block
      delayRead := true
      recursiveProjection := false
      multipleOccurrences := false } = true := by
  decide

/-! ## Placement, epochs, transports, and vector-plan certificates -/

/-
  Placement separates ownership from execution order:

  * `control` is materialized in a fixed slower/lifecycle region;
  * `inline` may be rebuilt in several loops, but only when duplicable;
  * `owned loop` has one materialized producer loop.

  Loop kind records whether a loop may use vector FIR, must preserve a recursive
  group's serial semantics, is a serial clock/effect island, or is a lockstep
  bundle of `width` isomorphic serial instances (serial in time, parallel
  across SIMD lanes — a fourth kind, neither `vectorizable` nor plain
  `recursive`; see the lockstep section below).
-/

inductive Placement where
  | control
  | inline
  | owned (loop : LoopId)
  deriving Repr, DecidableEq, BEq

inductive LoopKind where
  | vectorizable
  | recursive (group : Nat)
  | island (clock : ClockId)
  | lockstep (width : Nat)
  deriving Repr, DecidableEq, BEq

structure ExecutionEpoch where
  id : EpochId
  rank : Nat
  loops : List LoopId
  deriving Repr, DecidableEq

/-
  Epochs are fixed semantic phases, for example forward and reverse AD passes.
  `id` is stable identity; `rank` is execution order. Scheduling may reorder
  loops inside an epoch but must never reorder or interleave epochs.
-/

structure Transport where
  signal : SigId
  producer : LoopId
  consumer : LoopId
  elementTy : ValueTy
  length : Nat
  deriving Repr

/-
  A transport is the explicit chunk-local storage required when an owned signal
  is consumed by another loop. Its element type must match the signal type and
  its array length must equal the vector chunk size.
-/

def Transport.WellTyped (typeOf : SigId -> ValueTy) (vecSize : Nat)
    (transport : Transport) : Prop :=
  transport.producer ≠ transport.consumer
    ∧ transport.elementTy = typeOf transport.signal
    ∧ transport.length = vecSize

def chunkIndex (i0 vindex : Nat) : Nat := i0 - vindex

theorem chunkIndex_lt (i0 vindex vecSize : Nat)
    (lower : vindex ≤ i0) (upper : i0 < vindex + vecSize) :
    chunkIndex i0 vindex < vecSize := by
  simp [chunkIndex]
  omega

/-
  `chunkIndex_lt` proves the array-safety part of `j = i0 - vindex`: when `i0`
  lies in the current half-open chunk `[vindex, vindex + vecSize)`, `j` is a
  valid transport index. The lower bound also prevents truncated subtraction
  from hiding an invalid sample position.
-/

/-- A signal is duplicable when re-evaluating it in another region at the same
    logical sample yields identical bits and observations. Sufficient structural
    condition used here: its aggregated effect set contains only pure foreign
    calls — no state/table/UI/output read or write, and no impure/unknown call.
    Because effects propagate to parents (see `HasType`), this also discharges
    the "recursively duplicable operands" requirement. It is conservative: it may
    reject a genuinely duplicable signal, but never accepts a non-duplicable one,
    so `P-Duplicate` can never be satisfied vacuously. -/
def duplicableEffectsB (es : List Effect) : Bool :=
  es.all fun e =>
    match e with
    | .foreignCall _ .pure => true
    | _ => false

/-- An effect is reorderable across samples when it carries no per-sample state.
    Reads and writes of loop-carried state are exactly the accesses that loop
    fission must not reorder, so they are excluded here. -/
def sampleReorderableB (es : List Effect) : Bool :=
  es.all fun e =>
    match e with
    | .read (.state _) => false
    | .write (.state _) => false
    | _ => true

structure VectorPlan where
  signals : List SigId
  loops : List LoopId
  vecSize : Nat
  vecSizePositive : 0 < vecSize
  signalType : SigId -> ValueTy
  placement : SigId -> Placement
  effects : SigId -> List Effect
  vectorability : SigId -> Vectorability
  roots : LoopId -> List SigId
  loopKind : LoopId -> LoopKind
  epochs : List ExecutionEpoch
  transports : List Transport
  dataEdges : List (LoopId × LoopId)
  effectEdges : List (LoopId × LoopId)
  stableName : LoopId -> String
  placementDuplicate : ∀ signal,
    placement signal = .inline -> duplicableEffectsB (effects signal) = true
  rootsNodup : ∀ loop, (roots loop).Nodup
  ownedHasRoot : ∀ signal loop,
    placement signal = .owned loop -> signal ∈ roots loop
  rootHasOwner : ∀ signal loop,
    signal ∈ roots loop -> placement signal = .owned loop

/-
  `VectorPlan` is the strategy-independent result of signal-level analysis.
  Proof fields enforce local construction invariants immediately:

  * an inline signal is duplicable (checked against its effects, not asserted);
  * roots contain no duplicates within a loop;
  * ownership and root membership agree in both directions.

  Duplicability and vectorization-safety are no longer opaque `Prop` parameters
  (which could be discharged by `fun _ => True`). They are *defined* below from
  the per-signal `effects` and `vectorability` data the analysis actually
  produces, so the corresponding invariants have real content.

  Functions such as `signalType`, `placement`, and `loopKind` stand for immutable
  tables in the Rust implementation. The absence of `SchedulingStrategy` is
  intentional and makes P-Strategy visible in the type itself.
-/

/-- `P-Duplicate` content: an inline/duplicated signal has only pure-foreign
    effects. Defined from `effects`, so it is never vacuously true. -/
def VectorPlan.Duplicable (plan : VectorPlan) (s : SigId) : Prop :=
  duplicableEffectsB (plan.effects s) = true

/-- `VecSafe` content (a conservative sufficient condition): every materialized
    root of the loop is locally vectorizable and carries no cross-sample state
    effect. This can classify more loops as unsafe than C++, which is the sound
    direction; a relaxation must add and test a new discharge rule. -/
def VectorPlan.VecSafe (plan : VectorPlan) (l : LoopId) : Prop :=
  (∀ s ∈ plan.roots l, plan.vectorability s = Vectorability.vect)
    ∧ (∀ s ∈ plan.roots l, sampleReorderableB (plan.effects s) = true)

def VectorPlan.allEdges (plan : VectorPlan) : List (LoopId × LoopId) :=
  plan.dataEdges ++ plan.effectEdges

def VectorPlan.epochGraph (plan : VectorPlan)
    (epoch : ExecutionEpoch) : DependencyGraph LoopId where
  nodes := epoch.loops
  dependencies consumer :=
    plan.allEdges.filterMap fun edge =>
      if edge.1 == consumer && epoch.loops.contains edge.2 then
        some edge.2
      else
        none

/-
  `epochGraph` is the induced graph scheduled by `-ss`: it keeps only edges whose
  consumer and dependency belong to the selected epoch. Edges crossing epoch
  boundaries are enforced by barriers, not passed to the local scheduler.
-/

/-
  SchedulingStrategy is intentionally absent from VectorPlan. This type-level
  boundary encodes P-Strategy: changing -ss cannot alter ownership, epochs,
  edges, transports, loop ids, or stable names.
-/

def epochRankOf? (epochs : List ExecutionEpoch) (loop : LoopId) : Option Nat :=
  match epochs.find? (fun epoch => loop ∈ epoch.loops) with
  | none => none
  | some epoch => some epoch.rank

def EpochBarriersValid (epochs : List ExecutionEpoch) :
    List (LoopId × LoopId) -> Prop
  | [] => True
  | edge :: rest =>
      (match epochRankOf? epochs edge.1, epochRankOf? epochs edge.2 with
       | some consumerRank, some dependencyRank => dependencyRank ≤ consumerRank
       | _, _ => False)
      ∧ EpochBarriersValid epochs rest

/-
  Since edges are `consumer -> dependency`, every dependency epoch must have a
  rank no greater than its consumer epoch. Equality permits an intra-epoch edge;
  a smaller rank represents an already completed semantic phase. Unique ranks
  in `VectorPlanCertificate` prevent two distinct epochs from masquerading as
  the same phase.
-/

def AllTransportsWellTyped (typeOf : SigId -> ValueTy) (vecSize : Nat) :
    List Transport -> Prop
  | [] => True
  | transport :: rest =>
      transport.WellTyped typeOf vecSize
        ∧ AllTransportsWellTyped typeOf vecSize rest

def AllEdgesHaveEndpoints (loops : List LoopId) :
    List (LoopId × LoopId) -> Prop
  | [] => True
  | edge :: rest =>
      edge.1 ∈ loops ∧ edge.2 ∈ loops
        ∧ AllEdgesHaveEndpoints loops rest

structure VectorPlanCertificate (plan : VectorPlan) where
  loopsNodup : plan.loops.Nodup
  signalsNodup : plan.signals.Nodup
  epochIdsNodup : (plan.epochs.map (fun epoch => epoch.id)).Nodup
  epochRanksNodup : (plan.epochs.map (fun epoch => epoch.rank)).Nodup
  epochLoopsNodup : (plan.epochs.flatMap (fun epoch => epoch.loops)).Nodup
  epochLoopsCover : ∀ loop,
    loop ∈ plan.loops ↔ loop ∈ plan.epochs.flatMap (fun epoch => epoch.loops)
  edgeEndpoints : AllEdgesHaveEndpoints plan.loops plan.allEdges
  epochDag : ∀ epoch,
    epoch ∈ plan.epochs -> HasValidSchedule (plan.epochGraph epoch)
  transportTyped : AllTransportsWellTyped plan.signalType plan.vecSize plan.transports
  barriersValid : EpochBarriersValid plan.epochs plan.allEdges
  vectorizableSafe : ∀ loop,
    plan.loopKind loop = .vectorizable -> plan.VecSafe loop

/-
  This certificate is the finite gate between planning and FIR lowering:

  * ids and epoch membership are unique and cover all loops;
  * every edge endpoint exists;
  * each induced epoch graph admits a valid topological schedule;
  * transports have the expected signal type and chunk size;
  * all edges respect fixed epoch order;
  * every loop labelled vectorizable carries its `VecSafe` witness.

  The fields are propositions because this file specifies required evidence.
  The Rust port should implement an executable `verify_vector_plan` whose
  successful result constructs the corresponding evidence or mirrors it in
  differential/exhaustive tests.
-/

/-! ## Region-aware lowering and fission obligations -/

/-
  The four lowering rules correspond directly to the mathematical rewrite
  rules R-INLINE, R-LOCAL, R-CROSS, and R-CONTROL. `LoweringWitness` records the
  selected case and the two semantic obligations common to every case: effects
  are emitted exactly once and storage does not alter value bits. A cross-loop
  rule additionally carries its producer and transport.
-/

inductive LoweringRule where
  | inline
  | local
  | cross
  | control
  deriving Repr, DecidableEq, BEq

structure LoweringWitness where
  rule : LoweringRule
  signal : SigId
  region : LoopId
  firType : ValueTy
  producer : Option LoopId
  transport : Option Transport
  effectsEmittedExactlyOnce : Prop
  valueBitsPreserved : Prop

structure Event where
  sample : Nat
  loop : LoopId
  statement : Nat
  deriving Repr, DecidableEq, BEq

def FissionSafe : List (Event × Event) ->
    (Event -> Event -> Prop) -> (Event -> Event -> Prop) -> Prop
  | [], _, _ => True
  | edge :: rest, scalarBefore, vectorBefore =>
      (scalarBefore edge.1 edge.2 -> vectorBefore edge.1 edge.2)
        ∧ FissionSafe rest scalarBefore vectorBefore

/-
  An event identifies a dynamic sample, loop, and statement. `FissionSafe`
  states that every true dependence ordered by scalar execution remains ordered
  after loop fission. The dependence list is an abstract semantic object; the
  compiler checks the finite sufficient facts below instead of enumerating all
  runtime events.
-/

structure StaticFissionFacts where
  loopDag : Prop
  transportsComplete : Prop
  effectsOrdered : Prop
  barriersOrdered : Prop
  carriedDependenciesInternalToSerialLoops : Prop

def StaticFissionSafe (facts : StaticFissionFacts) : Prop :=
  facts.loopDag
    ∧ facts.transportsComplete
    ∧ facts.effectsOrdered
    ∧ facts.barriersOrdered
    ∧ facts.carriedDependenciesInternalToSerialLoops

/-
  The five static facts summarize the implementable legality test. Their
  implication to dynamic `FissionSafe` is deliberately not asserted globally:
  it must be proved for the concrete faust-rs plan, dependence extraction, and
  execution relations.
-/

def StaticImpliesDynamicFission
    (Plan : Type) (staticSafe dynamicSafe : Plan -> Prop) : Prop :=
  ∀ plan, staticSafe plan -> dynamicSafe plan

/-
  The faust-rs port must instantiate `Plan`, `staticSafe`, and `dynamicSafe`,
  then prove `StaticImpliesDynamicFission`. It is intentionally a proof
  obligation here, not an assumed theorem.
-/

/-! ## Lockstep instance bundling (planned schema-v2 extension) -/

/-
  A lockstep bundle merges k structurally isomorphic, mutually independent
  serial loops into one loop that advances all k instances sample by sample,
  one SIMD lane per instance. The recursion stays serial in time; the
  parallelism is across lanes. Each lane executes exactly its scalar IEEE op
  sequence, so bundling is bit-exact per lane (given one FMA-contraction
  policy shared by scalar and lockstep lowering).

  Legality reuses obligations this file already defines. Lockstep execution
  interleaves the bundled loops' per-sample events — `A(0), B(0), A(1), B(1)`
  instead of `A(0..q); B(0..q)` — so it is one more linearization of the same
  dependence relation, admissible exactly when the loops are independent and
  their effects commute. The single genuinely new obligation is the structural
  isomorphism witness (`IsoModuloLeaves` at the representative `Expr` layer, a
  shape hash plus per-lane leaf maps in the production certificate).
-/

def VectorPlan.loopEffects (plan : VectorPlan) (l : LoopId) : List Effect :=
  (plan.roots l).flatMap plan.effects

/-
  `Reaches` is plain reachability over the plan's data and effect edges. Two
  loops are `Independent` when neither reaches the other; only such loops may
  be bundled, because lockstep executes them simultaneously per sample.
-/

inductive Reaches (edges : List (LoopId × LoopId)) : LoopId -> LoopId -> Prop where
  | here {a b : LoopId} : (a, b) ∈ edges -> Reaches edges a b
  | step {a b c : LoopId} : (a, b) ∈ edges -> Reaches edges b c ->
      Reaches edges a c

def Independent (plan : VectorPlan) (a b : LoopId) : Prop :=
  ¬Reaches plan.allEdges a b ∧ ¬Reaches plan.allEdges b a

def PairwiseBundlable (plan : VectorPlan) : List LoopId -> Prop
  | [] => True
  | l :: rest =>
      (∀ m ∈ rest,
        Independent plan l m
          ∧ effectsCommute (plan.loopEffects l) (plan.loopEffects m) = true
          ∧ epochRankOf? plan.epochs l = epochRankOf? plan.epochs m)
        ∧ PairwiseBundlable plan rest

/-
  The finite gate for one bundle. The isomorphism side lives at the signal
  layer: production certificates carry one leaf mapping per lane, checked by
  parallel traversal; `iso_decorations_agree` is the proved statement that
  isomorphic lanes share value type, rate, vectorability, and clock — which is
  what lets one bundle use a single transport element type and one clock/epoch
  placement for all lanes.
-/

structure LockstepObligations (plan : VectorPlan) (lanes : List LoopId) where
  widthAtLeastTwo : 2 ≤ lanes.length
  lanesNodup : lanes.Nodup
  lanesInPlan : ∀ l ∈ lanes, l ∈ plan.loops
  bundlable : PairwiseBundlable plan lanes

/-
  The semantic obligation needs no new machinery: lockstep order is one more
  execution order that must preserve every true dependence, which is literally
  the `FissionSafe` statement with the lockstep order substituted for the
  chunked vector order. No new axiom is introduced by the extension.
-/

def LockstepSafe (deps : List (Event × Event))
    (scalarBefore lockstepBefore : Event -> Event -> Prop) : Prop :=
  FissionSafe deps scalarBefore lockstepBefore

/-! ## Delay and transition semantics -/

/-
  The remaining definitions are polymorphic in the actual value and state
  representations. This keeps the specification usable for the interpreter,
  Cranelift, Wasm, AssemblyScript, and future backends.

  History is ordered newest first: `history[0]` is the previous sample. Delay
  zero reads the current value directly; delay `n > 0` reads slot `n - 1`.
  Out-of-range reads return `none`, forcing the concrete compiler proof to show
  that declared maximum delay bounds are respected.
-/

def delayRead (current : Value) (history : List Value) (delay : Nat) : Option Value :=
  if delay = 0 then some current else history[delay - 1]?

def historyStep (current : Value) (history : List Value) : List Value :=
  (current :: history).take history.length

/-
  `historyStep` inserts the current value and drops the oldest slot. The length
  theorem proves that one transition preserves the abstract delay-state shape.
-/

theorem delayRead_zero (current : Value) (history : List Value) :
    delayRead current history 0 = some current := by
  simp [delayRead]

theorem historyStep_length (current : Value) (history : List Value) :
    (historyStep current history).length = history.length := by
  simp [historyStep]

def iterate (count : Nat) (step : State -> State) (state : State) : State :=
  match count with
  | 0 => state
  | count + 1 => iterate count step (step state)

/-
  `iterate n step initial` is the abstract scalar `Run(n, ...)`. Defining it by
  structural recursion gives Lean an immediate termination proof and exposes
  zero/successor equations for later simulations.
-/

theorem iterate_zero (step : State -> State) (state : State) :
    iterate 0 step state = state := rfl

theorem iterate_succ (count : Nat) (step : State -> State) (state : State) :
    iterate (count + 1) step state = iterate count step (step state) := rfl

structure ExecutionResult (State Output Observation : Type) where
  finalState : State
  outputs : List Output
  observations : List Observation
  deriving Repr, DecidableEq

/-
  Equality of execution results covers persistent state, all output samples,
  and externally visible observations. Instantiations should include tables and
  UI zones in `State` or `Observation`, so simulation cannot ignore them.
-/

inductive LoopVariant where
  | fastest
  | simple
  deriving Repr, DecidableEq, BEq

def VSimulation
    (State Input Output Observation : Type)
    (scheduleValid : List (List LoopId) -> Prop)
    (scalarRun : State -> List Input -> ExecutionResult State Output Observation)
    (vectorRun : Nat -> LoopVariant -> List (List LoopId) -> State -> List Input ->
      ExecutionResult State Output Observation) : Prop :=
  ∀ vecSize variant epochSchedules initialState inputs,
    0 < vecSize ->
    scheduleValid epochSchedules ->
    vectorRun vecSize variant epochSchedules initialState inputs =
      scalarRun initialState inputs

/-
  `VSimulation` quantifies over both loop variants (`-lv 0` and `-lv 1`), every
  positive chunk size, every initial state, and input sequence.

  The `scheduleValid` premise is essential and was missing from an earlier draft:
  without it the statement demanded scalar/vector agreement even for a
  non-topological (invalid) per-epoch schedule, which is false. `scheduleValid`
  is the program-indexed refinement (each element valid for its epoch graph,
  via `ValidScheduleRel`) supplied by the faust-rs execution model; here it is an
  abstract parameter so the definition states the theorem that is actually true.
-/

def ScheduleIndependent
    (Schedule Observation : Type)
    (valid : Schedule -> Prop)
    (execute : Schedule -> Observation) : Prop :=
  ∀ left right, valid left -> valid right -> execute left = execute right

/-
  Schedule independence is the semantic requirement behind exposing `-ss`:
  any two valid orders must produce the same observation. Topological validity
  alone is insufficient; the vector-plan certificate's transport, effect, and
  barrier obligations are the premises needed by the eventual proof.
-/

/-! ## Executable regression guards -/

/-
  Unlike printed `#eval` output, every `#guard` is an assertion: changing edge
  direction, strategy decoding, or separation precedence makes elaboration fail.
-/

#guard verifySchedule diamondGraph [0, 1, 2, 3]
#guard !verifySchedule diamondGraph [1, 0, 2, 3]
#guard decodeStrategy 0 == .depthFirst
#guard decodeStrategy 3 == .reverseBreadthFirst
#guard separateLoop
  { maxDelay := 0
    verySimple := false
    rate := .samp
    delayRead := false
    recursiveProjection := false
    multipleOccurrences := true }

/-
  Two one-pole-style lanes: same skeleton, different input channels, literal
  values, and state resources — isomorphic modulo leaves. Changing the shape
  (a different delay-amount constructor) must break the isomorphism.
-/

private def isoLaneA : Expr :=
  .bin (.input 0) (.delay (.input 0) (.intLit 1) (.state 0))

private def isoLaneB : Expr :=
  .bin (.input 1) (.delay (.input 1) (.intLit 2) (.state 1))

private def isoLaneBad : Expr :=
  .bin (.input 1) (.delay (.input 1) (.realLit 2.0) (.state 1))

#guard isoB isoLaneA isoLaneB
#guard !isoB isoLaneA isoLaneBad
#guard !isoB isoLaneA (.input 0)

end Faust.VectorScheduling
