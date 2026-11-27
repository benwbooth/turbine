//! tessh's tree-sitter grammar: `LANGUAGE` and its highlight query.

use tree_sitter_language::LanguageFn;

extern "C" {
    fn tree_sitter_tessh() -> *const ();
}

pub const LANGUAGE: LanguageFn = unsafe { LanguageFn::from_raw(tree_sitter_tessh) };

pub const HIGHLIGHTS_QUERY: &str = include_str!("../../queries/highlights.scm");

pub mod highlight;

#[cfg(test)]
mod tests {
    #[test]
    fn loads() {
        let mut p = tree_sitter::Parser::new();
        p.set_language(&super::LANGUAGE.into()).unwrap();
        let t = p.parse("ls -la | grep x\ndef f x\n  print $x\n", None).unwrap();
        assert!(!t.root_node().has_error());
        tree_sitter::Query::new(&super::LANGUAGE.into(), super::HIGHLIGHTS_QUERY).unwrap();
    }

    #[test]
    fn heredocs_and_interpolation() {
        let t = "cat <<END | tr a-z A-Z\nhi $x\nEND\nprint \"a $y\"\n";
        let sp = super::highlight::spans(t);
        let at = |w: &str| sp.iter().rev().find(|(s, e, _)| &t[*s..*e] == w).map(|x| x.2);
        assert_eq!(at("<<END"), Some("operator"));
        assert_eq!(at("\nhi "), Some("string"));
        assert_eq!(at("$y"), Some("variable"));
        assert_eq!(at("$x"), Some("variable"));
        let t = "cat <<'END'\nhi $x\nEND\n";
        let sp = super::highlight::spans(t);
        assert!(sp.iter().all(|(s, e, k)| &t[*s..*e] != "$x" || *k == "string"));
    }
}
