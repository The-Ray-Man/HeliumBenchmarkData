//! A read-only visitor over the parsed AST, copied from ViperHelium
//! (`src/viper/walk.rs`) without the mutating walker. Override a `walk_*`
//! method to see that node; call `walk_children` to keep descending.

use num::{BigInt, BigRational};

use super::ast::*;

type ExpCall = Call<ExpCallKind>;
type StmtCall = Call<StmtCallKind>;

macro_rules! walk_children {
    ($name:ident, $l:lifetime, $ty:ident) => {
        fn $name(&mut self, ast: &$l $ty) {
            ast.walk_children(self);
        }
    };
}

pub trait AstWalker<'a>: Sized {
    walk_children!(walk_program, 'a, Program);
    walk_children!(walk_decreases, 'a, Decreases);
    walk_children!(walk_decreases_kind, 'a, DecreasesKind);
    walk_children!(walk_ident, 'a, Ident);
    walk_children!(walk_idn_decl, 'a, IdnDecl);
    walk_children!(walk_declaration, 'a, Declaration);
    walk_children!(walk_domain_element, 'a, DomainElement);
    walk_children!(walk_domain_element_kind, 'a, DomainElementKind);
    walk_children!(walk_axiom, 'a, Axiom);
    walk_children!(walk_import, 'a, Import);
    walk_children!(walk_define, 'a, Define);
    walk_children!(walk_exp_or_block, 'a, ExpOrBlock);
    walk_children!(walk_idn_decl_typed, 'a, IdnDeclTyped);
    walk_children!(walk_arg_or_type, 'a, ArgOrType);
    walk_children!(walk_exp_block, 'a, ExpBlock);
    walk_children!(walk_exp, 'a, Exp);
    walk_children!(walk_exp_kind, 'a, ExpKind);
    walk_children!(walk_exp_call_kind, 'a, ExpCallKind);
    walk_children!(walk_stmt_call_kind, 'a, StmtCallKind);
    walk_children!(walk_exp_call, 'a, ExpCall);
    walk_children!(walk_stmt_call, 'a, StmtCall);
    walk_children!(walk_const, 'a, ConstKind);
    walk_children!(walk_heap_op_kind, 'a, HeapUpdateOp);
    walk_children!(walk_quantifier_kind, 'a, QuantifierKind);
    walk_children!(walk_acc_exp, 'a, AccExp);
    walk_children!(walk_bin_op, 'a, BinOp);
    walk_children!(walk_un_op, 'a, UnOp);
    walk_children!(walk_trigger, 'a, Trigger);
    walk_children!(walk_res_access, 'a, ResAccess);
    walk_children!(walk_block, 'a, StmtBlock);
    walk_children!(walk_statement, 'a, Statement);
    walk_children!(walk_assign_lhs, 'a, AssignLhs);
    walk_children!(walk_assign_rhs, 'a, AssignRhs);
    walk_children!(walk_star_or_names, 'a, StarOrNames);
    walk_children!(walk_index_op, 'a, IndexOp);
    walk_children!(walk_invariant, 'a, Invariant);
    walk_children!(walk_field, 'a, Field);
    walk_children!(walk_domain, 'a, Domain);
    walk_children!(walk_domain_function, 'a, DomainFunction);
    walk_children!(walk_signature, 'a, Signature);
    walk_children!(walk_type, 'a, Type);
    walk_children!(walk_predicate, 'a, Predicate);
    walk_children!(walk_function, 'a, Function);
    walk_children!(walk_contract, 'a, Contract);
    walk_children!(walk_method, 'a, Method);
    walk_children!(walk_adt, 'a, Adt);
    walk_children!(walk_variant, 'a, Variant);
    walk_children!(walk_adt_constructor, 'a, AdtConstructor);

    walk_children!(walk_string, 'a, String);
    walk_children!(walk_bool, 'a, bool);
    walk_children!(walk_big_int, 'a, BigInt);
    walk_children!(walk_big_rational, 'a, BigRational);
    walk_children!(walk_usize, 'a, usize);
}

pub trait AstWalkable {
    fn walk<'a>(&'a self, walker: &mut impl AstWalker<'a>);
    fn walk_children<'a>(&'a self, walker: &mut impl AstWalker<'a>);
}

macro_rules! walk_struct {
    ($name:ident, $walk:ident$(, $field:tt)*) => {
        impl AstWalkable for $name {
            fn walk<'a>(&'a self, walker: &mut impl AstWalker<'a>) {
                walker.$walk(self);
            }
            #[allow(unused_variables)]
            fn walk_children<'a>(&'a self, walker: &mut impl AstWalker<'a>) {
                $(self.$field.walk(walker));*
            }
        }
    };
}

macro_rules! walk_enum {
    ($name:ident, $walk:ident$(, $variant:ident$(($($field:ident),+))?)*) => {
        impl AstWalkable for $name {
            fn walk<'a>(&'a self, walker: &mut impl AstWalker<'a>) {
                walker.$walk(self);
            }
            #[allow(unused_variables)]
            fn walk_children<'a>(&'a self, walker: &mut impl AstWalker<'a>) {
                match self {
                    $($name::$variant$(($($field),+))? => { $($($field.walk(walker));+)? })*
                }
            }
        }
    };
}

walk_struct!(Program, walk_program, 0);
walk_struct!(Decreases, walk_decreases, kind, guard);
walk_enum!(
    DecreasesKind,
    walk_decreases_kind,
    Star,
    Underscore,
    Exp(e)
);
walk_enum!(Ident, walk_ident, Raw(s));
walk_struct!(IdnDecl, walk_idn_decl, 0);
walk_enum!(
    Declaration,
    walk_declaration,
    Import(i),
    Define(d),
    Domain(d),
    DomainElement(f),
    Field(f),
    Function(f),
    Predicate(p),
    Method(m),
    Adt(a),
    AdtConstructor(c)
);
walk_struct!(
    DomainElement,
    walk_domain_element,
    domain,
    kind
);
walk_enum!(
    DomainElementKind,
    walk_domain_element_kind,
    Function(f),
    Axiom(a)
);
walk_struct!(Axiom, walk_axiom, name, exp);
walk_struct!(Import, walk_import, path);
walk_struct!(Define, walk_define, name, args, body);
walk_enum!(
    ExpOrBlock,
    walk_exp_or_block,
    Exp(e),
    Block(b)
);
walk_struct!(
    IdnDeclTyped,
    walk_idn_decl_typed,
    idn,
    ty
);
walk_enum!(
    ArgOrType,
    walk_arg_or_type,
    Arg(a),
    Type(t)
);
walk_struct!(ExpBlock, walk_exp_block, 0);
walk_struct!(Exp, walk_exp, kind);
walk_enum!(
    ExpKind,
    walk_exp_kind,
    Const(c),
    Ident(i),
    Result,
    Old(i, e),
    Ascribe(e, t),
    UnOp(op, e),
    BinOp(op, l, r),
    Ternary(c, t, e),
    LetIn(i, e1, e2),
    Index(e, op),
    Acc(a),
    Field(e, i),
    Call(a),
    AdtDestructor(e, i),
    AdtDiscriminator(e, i),
    HeapUpdate(kind, acc_exp, e),
    WandUpdate(kind, w, e),
    Quantifier(kind, vars, triggers, e),
    ForPerm(vars, p, e),
    MagicWand(l, r),
    Unsupported(what)
);
walk_enum!(
    ExpCallKind,
    walk_exp_call_kind,
    Function,
    DomainFunction,
    Predicate,
    AdtConstructor,
    Macro
);
walk_enum!(
    StmtCallKind,
    walk_stmt_call_kind,
    Method,
    Macro
);
walk_struct!(ExpCall, walk_exp_call, kind, name, args);
walk_struct!(
    StmtCall,
    walk_stmt_call,
    kind,
    name,
    args
);
walk_enum!(
    ConstKind,
    walk_const,
    Bool(b),
    Int(i),
    Real(r),
    Null,
    Epsilon,
    Wildcard
);
walk_enum!(
    HeapUpdateOp,
    walk_heap_op_kind,
    Unfold,
    Fold,
    Apply,
    Package
);
walk_enum!(
    QuantifierKind,
    walk_quantifier_kind,
    Forall,
    Exists
);
walk_struct!(AccExp, walk_acc_exp, loc, perm);
walk_enum!(
    BinOp,
    walk_bin_op,
    Implies,
    Iff,
    And,
    Or,
    Eq,
    Neq,
    Lt,
    Le,
    Gt,
    Ge,
    In,
    Plus,
    Minus,
    Mult,
    Div,
    IntDiv,
    Mod,
    Union,
    SetMinus,
    Intersection,
    Subset,
    Concat,
    Range,
    InhaleExhale
);
walk_enum!(UnOp, walk_un_op, Not, Neg, Perm);
walk_struct!(Trigger, walk_trigger, exp);
walk_enum!(
    ResAccess,
    walk_res_access,
    Loc(l),
    Exp(e)
);
walk_struct!(StmtBlock, walk_block, 0);
walk_enum!(
    Statement,
    walk_statement,
    Assume(e),
    Assert(e),
    Refute(e),
    Inhale(e),
    Exhale(e),
    Fold(e),
    Unfold(e),
    Goto(i),
    Label(i, invariants),
    Var(vars, e),
    While(e, invs, decs, b),
    If(e, then, else_),
    Package(w, b),
    Apply(w),
    Assign(lhs, rhs),
    Block(b),
    Unsupported(what)
);
walk_enum!(
    AssignLhs,
    walk_assign_lhs,
    Ident(i),
    Field(e, i)
);
walk_enum!(
    AssignRhs,
    walk_assign_rhs,
    New(n),
    Call(k),
    Exp(e)
);
walk_enum!(
    StarOrNames,
    walk_star_or_names,
    Star,
    Names(names)
);
walk_enum!(
    IndexOp,
    walk_index_op,
    Index(e),
    LowerBound(e),
    UpperBound(e),
    Range(e1, e2),
    Assign(e1, e2)
);
walk_struct!(Invariant, walk_invariant, 0);
walk_struct!(Field, walk_field, 0);
walk_struct!(
    Domain,
    walk_domain,
    name,
    params,
    interpretation
);
walk_struct!(
    Function,
    walk_function,
    signature,
    contract,
    body
);
walk_struct!(
    Contract,
    walk_contract,
    precondition,
    postcondition,
    decreases
);
walk_struct!(
    DomainFunction,
    walk_domain_function,
    unique,
    signature,
    interpretation
);
walk_struct!(
    Signature,
    walk_signature,
    name,
    args,
    ret
);
walk_enum!(
    Type,
    walk_type,
    Bool,
    Int,
    Real,
    Ref,
    Generic(g),
    Domain(i, ts)
);
walk_struct!(
    Predicate,
    walk_predicate,
    signature,
    body
);
walk_struct!(
    Method,
    walk_method,
    signature,
    contract,
    body
);
walk_struct!(Adt, walk_adt, name, params, variants, derives);
walk_struct!(Variant, walk_variant, name, fields);
walk_struct!(
    AdtConstructor,
    walk_adt_constructor,
    signature
);

impl<T: AstWalkable> AstWalkable for Vec<T> {
    fn walk<'a>(&'a self, walker: &mut impl AstWalker<'a>) {
        self.walk_children(walker);
    }
    fn walk_children<'a>(&'a self, walker: &mut impl AstWalker<'a>) {
        for item in self {
            item.walk(walker);
        }
    }
}

impl<T: AstWalkable> AstWalkable for Option<T> {
    fn walk<'a>(&'a self, walker: &mut impl AstWalker<'a>) {
        self.walk_children(walker);
    }
    fn walk_children<'a>(&'a self, walker: &mut impl AstWalker<'a>) {
        if let Some(item) = self {
            item.walk(walker);
        }
    }
}

impl<T: AstWalkable, U: AstWalkable> AstWalkable for Result<T, U> {
    fn walk<'a>(&'a self, walker: &mut impl AstWalker<'a>) {
        self.walk_children(walker);
    }
    fn walk_children<'a>(&'a self, walker: &mut impl AstWalker<'a>) {
        match self {
            Ok(item) => item.walk(walker),
            Err(item) => item.walk(walker),
        }
    }
}

impl<T: AstWalkable, U: AstWalkable> AstWalkable for (T, U) {
    fn walk<'a>(&'a self, walker: &mut impl AstWalker<'a>) {
        self.walk_children(walker);
    }
    fn walk_children<'a>(&'a self, walker: &mut impl AstWalker<'a>) {
        self.0.walk(walker);
        self.1.walk(walker);
    }
}

/// Leaf: the `&'static str` payload of the `Unsupported` variants. Carries no
/// AST children, so the walk stops here.
impl AstWalkable for &'static str {
    fn walk<'a>(&'a self, _walker: &mut impl AstWalker<'a>) {}
    fn walk_children<'a>(&'a self, _walker: &mut impl AstWalker<'a>) {}
}

walk_struct!(String, walk_string);
walk_struct!(bool, walk_bool);
walk_struct!(BigInt, walk_big_int);
walk_struct!(BigRational, walk_big_rational);
walk_struct!(usize, walk_usize);
