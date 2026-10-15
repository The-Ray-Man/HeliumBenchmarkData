//! The parsed Viper AST, copied from ViperHelium (`src/viper/parsed/ast/`)
//! without what only its later passes need: interned identifiers and the type
//! slot on expressions.

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Program(pub Vec<Declaration>);

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Ident {
    /// ViperHelium also has an interned variant; the grammar only builds this one.
    Raw(String),
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct IdnDecl(pub Ident);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Declaration {
    Import(Import),
    Define(Define),
    Domain(Domain),
    DomainElement(DomainElement),
    Field(Field),
    Function(Function),
    Predicate(Predicate),
    Method(Method),
    Adt(Adt),
    AdtConstructor(AdtConstructor),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DomainElement {
    pub domain: Ident,
    pub kind: DomainElementKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum DomainElementKind {
    Function(DomainFunction),
    Axiom(Axiom),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Axiom {
    pub name: Option<IdnDecl>,
    pub exp: ExpBlock,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Import {
    pub path: String,
    pub local: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Define {
    pub name: IdnDecl,
    pub args: Vec<IdnDecl>,
    pub body: ExpOrBlock,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ExpOrBlock {
    Exp(Exp),
    Block(StmtBlock),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct IdnDeclTyped {
    pub idn: IdnDecl,
    pub ty: Type,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ArgOrType {
    Arg(IdnDeclTyped),
    Type(Type),
}

impl ArgOrType {
    pub fn idn(&self) -> Option<&IdnDecl> {
        match self {
            ArgOrType::Arg(id) => Some(&id.idn),
            ArgOrType::Type(_) => None,
        }
    }

    pub fn ty(&self) -> &Type {
        match self {
            ArgOrType::Arg(id) => &id.ty,
            ArgOrType::Type(ty) => ty,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Block<T>(pub T);

pub type ExpBlock = Block<Exp>;
pub type StmtBlock = Block<Vec<Statement>>;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Field(pub IdnDeclTyped);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Domain {
    pub name: IdnDecl,
    pub params: Vec<IdnDecl>,
    pub interpretation: Vec<(Ident, String)>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Function {
    pub signature: Signature,
    pub contract: Contract,
    pub body: Option<ExpBlock>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Contract {
    pub precondition: Vec<Exp>,
    pub postcondition: Vec<Exp>,
    pub decreases: Vec<Decreases>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DomainFunction {
    pub unique: bool,
    pub signature: Signature,
    pub interpretation: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Signature {
    pub name: IdnDecl,
    pub args: Vec<ArgOrType>,
    pub ret: Vec<ArgOrType>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Type {
    Bool,
    Int,
    Real,
    Ref,
    Generic(Ident),
    Domain(Ident, Vec<Type>),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Predicate {
    pub signature: Signature,
    pub body: Option<ExpBlock>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Method {
    pub signature: Signature,
    pub contract: Contract,
    pub body: Option<StmtBlock>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Adt {
    pub name: IdnDecl,
    pub params: Vec<IdnDecl>,
    pub variants: Vec<Variant>,
    pub derives: Vec<String>,
}

/// Duplicated data with `AdtConstructor`
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Variant {
    pub name: IdnDecl,
    pub fields: Vec<ArgOrType>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AdtConstructor {
    pub signature: Signature,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Decreases {
    pub kind: Option<DecreasesKind>,
    pub guard: Option<Exp>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum DecreasesKind {
    Star,
    Underscore,
    Exp(Vec<Exp>),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Exp {
    pub kind: Box<ExpKind>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ExpKind {
    Const(ConstKind),
    /// x
    Ident(Ident),
    /// result keyword
    Result,
    // old(e) or old[label](e)
    Old(Option<Ident>, Exp),
    // e : Type
    Ascribe(Exp, Type),

    /// op e
    UnOp(UnOp, Exp),
    /// e1 op e2
    BinOp(BinOp, Exp, Exp),
    /// c ? e1 : e2
    Ternary(Exp, Exp, Exp),
    /// let x == (e1) in e2
    LetIn(IdnDecl, Exp, Exp),
    /// e[e1]
    Index(Exp, IndexOp),

    /// acc(e1, 1/1)
    Acc(AccExp),
    /// e.f
    Field(Exp, Ident),

    /// Application of a function, predicate, macro, or adt constructor.
    /// ident(e1, ..., en)
    Call(Call<ExpCallKind>),

    /// adt.member
    AdtDestructor(Exp, Ident),
    /// adt.isCons
    AdtDiscriminator(Exp, Ident),

    /// unfolding(e) in E, and similarly for folding.
    HeapUpdate(HeapUpdateOp, AccExp, Exp),
    /// applying (w) in E and packaging (w) in E (`Apply` / `Package`), where
    /// `w` is a magic wand rather than a predicate access.
    WandUpdate(HeapUpdateOp, Exp, Exp),
    /// forall/exists x: T, y: U, ... :: { trigger } e
    Quantifier(QuantifierKind, Vec<IdnDeclTyped>, Vec<Trigger>, Exp),
    /// Quantified permissions. forperm x: T, y: U, ... [Perm] :: e1
    ForPerm(Vec<IdnDeclTyped>, ResAccess, Exp),
    /// e1 --* e2
    MagicWand(Exp, Exp),

    /// A construct the verifier does not implement, parsed only so that the
    /// declaration containing it can be reported as unsupported instead of
    /// taking the whole file down with a parse error. Carries the construct's
    /// name (`Seq`, `lhs`, `folding`, ...). Never lowered: typecheck turns it
    /// into `TypeError::Unsupported`.
    Unsupported(&'static str),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ExpCallKind {
    Function,
    DomainFunction,
    Predicate,
    AdtConstructor,
    Macro,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum StmtCallKind {
    Method,
    Macro,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Call<KnownCallKind> {
    pub kind: Option<KnownCallKind>,
    pub name: Ident,
    pub args: Vec<Exp>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ConstKind {
    Bool(bool),
    Int(num::BigInt),
    Real(num::BigRational),
    Null,
    Epsilon,
    Wildcard,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HeapUpdateOp {
    Unfold,
    Fold,
    Apply,
    Package,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum QuantifierKind {
    Forall,
    Exists,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AccExp {
    /// Either a field or a predicate call
    pub loc: Exp,
    /// The permission amount
    pub perm: Exp,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum BinOp {
    Implies,
    Or,
    And,
    Iff,
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
    /// Integer division (`\`). Silver reserves `/` for permission division, so
    /// `Int` division has its own token.
    IntDiv,
    Mod,
    Union,
    SetMinus,
    Intersection,
    Subset,
    Concat,
    Range,
    InhaleExhale,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum UnOp {
    Not,
    Neg,
    Perm,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Trigger {
    pub exp: Vec<Exp>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ResAccess {
    Loc(Exp),
    Exp(AccExp),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum IndexOp {
    Index(Exp),
    LowerBound(Exp),
    UpperBound(Exp),
    Range(Exp, Exp),
    Assign(Exp, Exp),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Statement {
    Assume(Exp),
    Assert(Exp),
    Refute(Exp),
    Inhale(Exp),
    Exhale(Exp),
    Fold(AccExp),
    Unfold(AccExp),
    Goto(Ident),
    Label(IdnDecl, Vec<Invariant>),
    Var(Vec<IdnDeclTyped>, Option<AssignRhs>),
    While(Exp, Vec<Invariant>, Vec<Decreases>, StmtBlock),
    If(Exp, StmtBlock, Option<StmtBlock>),
    /// package w { proof script }
    Package(Exp, Option<StmtBlock>),
    /// apply w
    Apply(Exp),
    Assign(Vec<AssignLhs>, AssignRhs),
    Block(StmtBlock),
    /// A statement the verifier does not implement (`quasihavoc`, `fresh`,
    /// ...), parsed only so the enclosing declaration can be reported as
    /// unsupported. See [`ExpKind::Unsupported`].
    Unsupported(&'static str),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum AssignLhs {
    Ident(Ident),
    Field(Exp, Ident),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum AssignRhs {
    Exp(Exp),
    Call(Call<StmtCallKind>),
    New(StarOrNames),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum StarOrNames {
    Star,
    Names(Vec<Ident>),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Invariant(pub Exp);

impl Exp {
    pub fn new(kind: ExpKind) -> Self {
        Exp { kind: Box::new(kind) }
    }
}

impl ExpKind {
    /// The full permission `write`, as the grammar fills in for `acc(e)`.
    pub fn write() -> Exp {
        Exp::new(ExpKind::Const(ConstKind::Real(num::BigInt::from(1).into())))
    }
}

impl Adt {
    /// The type the ADT's constructors return.
    pub fn identity(&self) -> Type {
        Type::Domain(
            self.name.0.clone(),
            self.params
                .iter()
                .map(|p| Type::Domain(p.0.clone(), Vec::new()))
                .collect(),
        )
    }
}
