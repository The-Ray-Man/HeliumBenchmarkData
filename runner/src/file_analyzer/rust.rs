//! Metrics of a Rust source file, from a `syn` parse.
//!
//! Only the whole-file columns of `rust_metrics` so far. The per-function ones
//! (`fn_loc`, `paths`, `stmts`, ..., `max_struct_depth`) are syntactic too and
//! go here as well; ViperHelium's `bench/src/rust_metrics.rs` computes them.

use syn::visit::{self, Visit};

use super::text::TextMetrics;

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RustMetrics {
    pub text: TextMetrics,
    /// Functions with a body: free functions, methods and trait default
    /// methods, including those in nested modules and function bodies.
    pub fns: u64,
    pub structs: u64,
    pub enums: u64,
}

/// Analyzes the source of one `.rs` file. Fails if it is not valid Rust.
pub fn analyze(source: &str) -> syn::Result<RustMetrics> {
    let file = syn::parse_file(source)?;
    let mut items = ItemCounter::default();
    items.visit_file(&file);
    Ok(RustMetrics {
        text: TextMetrics::of(source),
        fns: items.fns,
        structs: items.structs,
        enums: items.enums,
    })
}

#[derive(Default)]
struct ItemCounter {
    fns: u64,
    structs: u64,
    enums: u64,
}

impl<'ast> Visit<'ast> for ItemCounter {
    fn visit_item_fn(&mut self, i: &'ast syn::ItemFn) {
        self.fns += 1;
        visit::visit_item_fn(self, i);
    }

    fn visit_impl_item_fn(&mut self, i: &'ast syn::ImplItemFn) {
        self.fns += 1;
        visit::visit_impl_item_fn(self, i);
    }

    fn visit_trait_item_fn(&mut self, i: &'ast syn::TraitItemFn) {
        if i.default.is_some() {
            self.fns += 1;
        }
        visit::visit_trait_item_fn(self, i);
    }

    fn visit_item_struct(&mut self, i: &'ast syn::ItemStruct) {
        self.structs += 1;
        visit::visit_item_struct(self, i);
    }

    fn visit_item_enum(&mut self, i: &'ast syn::ItemEnum) {
        self.enums += 1;
        visit::visit_item_enum(self, i);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_items() {
        let src = r#"
// a cell
pub struct Cell { pub v: i32 }
enum Shape { A, B(Cell) }

trait T {
    fn required(&self);
    fn provided(&self) {}
}

impl Cell {
    fn get(&self) -> i32 { self.v }
}

mod inner {
    pub fn f() {
        fn nested() {}
        struct Local;
    }
}
"#;
        let m = analyze(src).unwrap();
        assert_eq!((m.fns, m.structs, m.enums), (4, 2, 1));
        assert_eq!(m.text.loc, 15);
    }

    #[test]
    fn rejects_invalid_rust() {
        assert!(analyze("fn {").is_err());
    }
}
