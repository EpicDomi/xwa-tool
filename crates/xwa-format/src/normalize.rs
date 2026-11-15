//! Search-key normalisation (spec section 6.1).

use unicode_normalization::UnicodeNormalization;
use unicode_normalization::char::is_combining_mark;

/// Folds a title into its search key: compatibility decomposition, accents
/// removed, case-folded, `_` treated as a space, whitespace collapsed.
pub fn search_key(title: &str) -> String {
    let stripped: String = title.nfkd().filter(|c| !is_combining_mark(*c)).collect();
    let folded = caseless::default_case_fold_str(&stripped);
    let mut out = String::with_capacity(folded.len());
    for word in folded
        .split(|c: char| c.is_whitespace() || c == '_')
        .filter(|w| !w.is_empty())
    {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(word);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::search_key;

    #[test]
    fn folds_case_accents_and_spacing() {
        assert_eq!(search_key("Albert_Einstein"), "albert einstein");
        assert_eq!(search_key("  Émile   Zola "), "emile zola");
        assert_eq!(search_key("Straße"), "strasse");
        assert_eq!(search_key("ﬁne"), "fine");
    }
}
