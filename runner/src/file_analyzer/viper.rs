//! Metrics of a Viper (`.vpr`) file that need no Viper parser: the text
//! columns of `viper_metrics` (`loc`, `lines`, `bytes`). The structural
//! columns come from Helium's `verify --viper-metrics`.

use super::text::TextMetrics;

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ViperMetrics {
    pub text: TextMetrics,
}

/// Analyzes the source of one `.vpr` file.
pub fn analyze(source: &str) -> ViperMetrics {
    ViperMetrics {
        text: TextMetrics::of(source),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_text() {
        let src = "// header\nfield f: Int\n\nmethod m(x: Ref)\n  requires acc(x.f) /* perm */\n{}\n";
        let m = analyze(src);
        assert_eq!((m.text.lines, m.text.loc), (6, 4));
    }
}
