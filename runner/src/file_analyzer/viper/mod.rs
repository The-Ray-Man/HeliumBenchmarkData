//! Metrics of a Viper (`.vpr`) file: every column of `viper_metrics`.
//!
//! The text columns (`loc`, `lines`, `bytes`) come from [`TextMetrics`]; the
//! rest is counted on the parsed AST, before any rewriting, so it measures
//! exactly what is in the file. The parser, AST and counting rules are
//! ViperHelium's (`src/viper/{parser,metrics}.rs`), so the numbers agree with
//! Helium's `verify --viper-metrics`.

// Copied from ViperHelium and kept close to it, so some of the AST is never
// built or read here.
#[allow(dead_code)]
mod ast;
mod parser;
#[allow(dead_code)]
mod walk;

use std::collections::HashSet;

use sea_orm::ActiveValue::Set;

use self::ast::*;
use self::walk::{AstWalkable, AstWalker};
use super::db_int;
use super::text::TextMetrics;
use crate::db::viper_metrics;

pub use peg::error::ParseError;
pub use peg::str::LineCol;

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ViperMetrics {
    pub text: TextMetrics,
    pub methods: u64,
    pub functions: u64,
    pub predicates: u64,
    pub domains: u64,
    pub domain_functions: u64,
    pub axioms: u64,
    pub fields: u64,
    pub adts: u64,
    /// Node counts summed over the whole program.
    pub counts: Counts,
}

impl From<ViperMetrics> for viper_metrics::ActiveModel {
    fn from(m: ViperMetrics) -> Self {
        (&m).into()
    }
}

/// The metric columns of a `viper_metrics` row. The rest is left unset for the
/// caller: the key `vpr_sha256`, `measured_at` and `metrics_commit`.
impl From<&ViperMetrics> for viper_metrics::ActiveModel {
    fn from(m: &ViperMetrics) -> Self {
        let int = |n: u64| Set(Some(db_int(n)));
        let c = &m.counts;
        viper_metrics::ActiveModel {
            loc: int(m.text.loc),
            lines: int(m.text.lines),
            bytes: Set(Some(i64::try_from(m.text.bytes).unwrap_or(i64::MAX))),
            methods: int(m.methods),
            functions: int(m.functions),
            predicates: int(m.predicates),
            domains: int(m.domains),
            domain_functions: int(m.domain_functions),
            axioms: int(m.axioms),
            fields: int(m.fields),
            adts: int(m.adts),
            stmts: int(c.stmts),
            exps: int(c.exps),
            folds: int(c.folds),
            unfolds: int(c.unfolds),
            heap_update_exps: int(c.heap_update_exps),
            inhales: int(c.inhales),
            exhales: int(c.exhales),
            asserts: int(c.asserts),
            assumes: int(c.assumes),
            accs: int(c.accs),
            quantifiers: int(c.quantifiers),
            labels: int(c.labels),
            gotos: int(c.gotos),
            ifs: int(c.ifs),
            whiles: int(c.whiles),
            assigns: int(c.assigns),
            method_calls: int(c.method_calls),
            function_apps: int(c.function_apps),
            old_exps: int(c.old_exps),
            contract_clauses: int(c.contract_clauses),
            refutes: int(c.refutes),
            var_decls: int(c.var_decls),
            packages: int(c.packages),
            applies: int(c.applies),
            other_stmts: int(c.other_stmts),
            magic_wands: int(c.magic_wands),
            ..Default::default()
        }
    }
}

/// Node counts over the whole program.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Counts {
    /// Statements of any kind, nested ones included.
    pub stmts: u64,
    /// Expression nodes.
    pub exps: u64,
    pub folds: u64,
    pub unfolds: u64,
    /// `unfolding ... in`, `folding ... in`, `applying ... in` and
    /// `packaging ... in` inside expressions.
    pub heap_update_exps: u64,
    pub inhales: u64,
    pub exhales: u64,
    pub asserts: u64,
    pub refutes: u64,
    pub assumes: u64,
    /// `var` declarations, with or without an initial value (one with a value
    /// also counts as an assignment or method call).
    pub var_decls: u64,
    /// `package` and `apply` statements on magic wands.
    pub packages: u64,
    pub applies: u64,
    /// Statements of no other kind: `quasihavoc`, `fresh`, `constraining`,
    /// local `define`, multi-target assignments from expressions.
    pub other_stmts: u64,
    /// `acc(...)` occurrences in expressions (contracts, inhale/exhale bodies).
    pub accs: u64,
    pub quantifiers: u64,
    /// `label` and `goto`: together, the size of a Prusti method's CFG.
    pub labels: u64,
    pub gotos: u64,
    pub ifs: u64,
    pub whiles: u64,
    pub assigns: u64,
    pub method_calls: u64,
    /// Calls in expressions: functions, domain functions, predicate instances.
    pub function_apps: u64,
    pub old_exps: u64,
    /// `A --* B` expressions.
    pub magic_wands: u64,
    /// Pre- and postconditions.
    pub contract_clauses: u64,
}

/// Analyzes the source of one `.vpr` file. Fails if it does not parse.
pub fn analyze(source: &str) -> Result<ViperMetrics, ParseError<LineCol>> {
    let program = parser::viper_parser::vpr_program(source)?;
    let mut m = ViperMetrics {
        text: TextMetrics::of(source),
        ..Default::default()
    };
    for decl in &program.0 {
        match decl {
            Declaration::Method(_) => m.methods += 1,
            Declaration::Function(_) => m.functions += 1,
            Declaration::Predicate(_) => m.predicates += 1,
            Declaration::Domain(_) => m.domains += 1,
            Declaration::DomainElement(DomainElement { kind, .. }) => match kind {
                DomainElementKind::Function(_) => m.domain_functions += 1,
                DomainElementKind::Axiom(_) => m.axioms += 1,
            },
            Declaration::Field(_) => m.fields += 1,
            Declaration::Adt(_) => m.adts += 1,
            Declaration::Import(_) | Declaration::Define(_) | Declaration::AdtConstructor(_) => {}
        }
    }
    let methods = program
        .0
        .iter()
        .filter_map(|d| match d {
            Declaration::Method(m) => Some(ident_name(&m.signature.name.0)),
            _ => None,
        })
        .collect();
    program.walk(&mut Counter {
        counts: &mut m.counts,
        methods: &methods,
    });
    Ok(m)
}

fn ident_name(ident: &Ident) -> &str {
    let Ident::Raw(name) = ident;
    name
}

/// Counts nodes into `counts`. The parser cannot tell `x := f(a)` (a function
/// application) from `x := m(a)` (a method call) — both come out as
/// [`AssignRhs::Call`] — so the walker is handed the program's method names to
/// decide.
struct Counter<'c, 'm> {
    counts: &'c mut Counts,
    methods: &'m HashSet<&'m str>,
}

impl Counter<'_, '_> {
    fn call_rhs(&mut self, call: &Call<StmtCallKind>) {
        if self.methods.contains(ident_name(&call.name)) {
            self.counts.method_calls += 1;
        } else {
            // Walking `args` does not visit the call node itself.
            self.counts.assigns += 1;
            self.counts.function_apps += 1;
            self.counts.exps += 1;
        }
    }
}

impl<'a> AstWalker<'a> for Counter<'_, '_> {
    fn walk_statement(&mut self, stmt: &'a Statement) {
        let c = &mut *self.counts;
        c.stmts += 1;
        match stmt {
            Statement::Assume(_) => c.assumes += 1,
            Statement::Assert(_) => c.asserts += 1,
            Statement::Refute(_) => c.refutes += 1,
            Statement::Inhale(_) => c.inhales += 1,
            Statement::Exhale(_) => c.exhales += 1,
            Statement::Fold(_) => c.folds += 1,
            Statement::Unfold(_) => c.unfolds += 1,
            Statement::Goto(_) => c.gotos += 1,
            Statement::Label(..) => c.labels += 1,
            Statement::While(..) => c.whiles += 1,
            Statement::If(..) => c.ifs += 1,
            Statement::Package(..) => c.packages += 1,
            Statement::Apply(_) => c.applies += 1,
            Statement::Unsupported(_) => c.other_stmts += 1,
            Statement::Assign(_, AssignRhs::Call(call)) => self.call_rhs(call),
            Statement::Assign(..) => c.assigns += 1,
            Statement::Var(_, rhs) => {
                c.var_decls += 1;
                match rhs {
                    Some(AssignRhs::Call(call)) => self.call_rhs(call),
                    Some(_) => c.assigns += 1,
                    None => {}
                }
            }
            Statement::Block(_) => {}
        }
        stmt.walk_children(self);
    }

    fn walk_exp_kind(&mut self, kind: &'a ExpKind) {
        let c = &mut *self.counts;
        c.exps += 1;
        match kind {
            ExpKind::Acc(_) => c.accs += 1,
            ExpKind::Call(_) => c.function_apps += 1,
            ExpKind::Quantifier(..) | ExpKind::ForPerm(..) => c.quantifiers += 1,
            ExpKind::HeapUpdate(..) | ExpKind::WandUpdate(..) => c.heap_update_exps += 1,
            ExpKind::MagicWand(..) => c.magic_wands += 1,
            ExpKind::Old(..) => c.old_exps += 1,
            _ => {}
        }
        kind.walk_children(self);
    }

    fn walk_contract(&mut self, contract: &'a Contract) {
        self.counts.contract_clauses +=
            (contract.precondition.len() + contract.postcondition.len()) as u64;
        contract.walk_children(self);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_text() {
        let src = "// header\nfield f: Int\n\nmethod m(x: Ref)\n  requires acc(x.f) /* perm */\n{}\n";
        let m = analyze(src).unwrap();
        assert_eq!((m.text.lines, m.text.loc), (6, 4));
    }

    // ViperHelium's `counts_statements_per_member`, over the whole program.
    #[test]
    fn counts_declarations_and_nodes() {
        let src = "\
field f: Int
predicate P(x: Ref) { acc(x.f) }
function g(x: Int): Int { x + 1 }
domain D {
  function d(x: Int): Int
  axiom ax { forall i: Int :: d(i) == i }
}
// a comment
method m(x: Ref)
  requires acc(P(x))
  ensures acc(P(x))
{
  unfold P(x)
  x.f := g(x.f)
  fold P(x)
  label l
  goto l
  inhale forall i: Int :: i == i
  n(x)
}
method n(x: Ref)
";
        let m = analyze(src).unwrap();
        assert_eq!(
            (m.methods, m.functions, m.predicates, m.fields),
            (2, 1, 1, 1)
        );
        assert_eq!((m.domains, m.domain_functions, m.axioms), (1, 1, 1));
        let c = &m.counts;
        assert_eq!((c.folds, c.unfolds, c.labels, c.gotos), (1, 1, 1, 1));
        assert_eq!((c.inhales, c.assigns, c.method_calls), (1, 1, 1));
        // The `inhale forall` and the axiom's `forall`.
        assert_eq!(c.quantifiers, 2);
        assert_eq!(c.contract_clauses, 2);
        // The predicate body's and both contract clauses'.
        assert_eq!(c.accs, 3);
        // `g(x.f)`, `P(x)` in both `acc`s, the `unfold` and the `fold`, and
        // `d(i)` in the axiom.
        assert_eq!(c.function_apps, 6);
    }

    #[test]
    fn counts_magic_wands_and_other_statements() {
        let src = "field f: Int
predicate P(x: Ref) { acc(x.f) }
method m(x: Ref)
  requires acc(x.f) --* P(x)
{
  var y: Int
  var z: Int := 1
  package acc(x.f) --* P(x) {
    fold P(x)
  }
  apply acc(x.f) --* P(x)
  assert applying (acc(x.f) --* P(x)) in true
  assert folding P(x) in true
  refute false
  quasihavoc x.f
}
";
        let c = analyze(src).unwrap().counts;
        assert_eq!(c.magic_wands, 4);
        assert_eq!((c.packages, c.applies, c.folds), (1, 1, 1));
        assert_eq!(c.heap_update_exps, 2);
        assert_eq!((c.var_decls, c.assigns), (2, 1));
        assert_eq!((c.asserts, c.refutes, c.other_stmts), (2, 1, 1));
        // Every statement, the `fold` inside the package block included.
        assert_eq!(c.stmts, 9);
    }

    #[test]
    fn type_names_with_builtin_prefix() {
        let src = "domain D {\n  function f(F: Set[Ref]): Interface\n  function g(x: Integer, y: Reference): Boolean\n}\n";
        assert!(analyze(src).is_ok());
    }

    #[test]
    fn predicate_instance_measure() {
        let src = "predicate tree(n: Ref)\nfunction f(n: Ref): Bool\n  requires acc(tree(n), write)\n  decreases @tree(n)\n";
        assert!(analyze(src).is_ok());
    }

    #[test]
    fn local_define() {
        let src = "method m(x: Int)\n{\n  define A x == 0 && true\n  assert x == x\n}\n";
        assert!(analyze(src).is_ok());
    }

    #[test]
    fn rejects_invalid_viper() {
        assert!(analyze("method m( {").is_err());
    }
}
