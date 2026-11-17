//! The article for each day of the year (DAYS, spec 5.3), found by the
//! titles Wikidata gives each Wikipedia's day articles ("January 1",
//! "1er janvier", "1月1日"). `cargo xtask days` makes the table.

use xwa_format::DAYS_IN_YEAR;

/// One line per wiki: its code, then the 366 titles, tab-separated.
static TABLE: &[u8] = include_bytes!("../data/days.tsv.zst");

/// The titles of wiki `code`'s day articles, January 1 to December 31
/// with February 29; empty where it has none.
pub fn titles(code: &str) -> Option<Vec<String>> {
    let table = zstd::decode_all(TABLE).ok()?;
    let table = std::str::from_utf8(&table).ok()?;
    let line = table.lines().find(|l| l.split('\t').next() == Some(code))?;
    let titles: Vec<String> = line.split('\t').skip(1).map(str::to_string).collect();
    (titles.len() == DAYS_IN_YEAR).then_some(titles)
}

/// The archive path of an article with this title.
pub fn path(title: &str) -> String {
    title.replace(' ', "_")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_has_the_days_of_the_big_wikipedias() {
        let days = |code| titles(code).unwrap_or_else(|| panic!("{code}"));
        let en = days("en");
        assert_eq!(
            (en[0].as_str(), en[59].as_str()),
            ("January 1", "February 29")
        );
        assert_eq!(en[365], "December 31");
        assert_eq!(days("fr")[0], "1er janvier");
        assert_eq!(days("de")[0], "1. Januar");
        assert_eq!(days("ja")[0], "1月1日");
        assert_eq!(days("sv")[365], "31 december");
        for code in [
            "en", "de", "fr", "es", "it", "ja", "ru", "zh", "pt", "pl", "sv", "ar",
        ] {
            assert!(days(code).iter().all(|t| !t.is_empty()), "{code}");
        }
        assert!(titles("not-a-wiki").is_none());
        assert_eq!(path("1er janvier"), "1er_janvier");
    }
}
