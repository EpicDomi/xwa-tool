//! Search keys (spec 6.1).
//!
//! A key folds a title one code point at a time: ASCII by rule, Hangul
//! syllables by arithmetic, anything else by a table. The table is the
//! archive's FOLD section, so a reader that folds what the user types
//! through it folds exactly as the converter folded the titles, without
//! Unicode tables of its own.

use std::sync::OnceLock;

use unicode_normalization::UnicodeNormalization;

/// Marks and invisible characters a key leaves out, after decomposition:
/// the accents and other diacritics that sit on letters (Latin, Greek,
/// Cyrillic, ...), the optional vowel points and cantillation of Arabic and
/// Hebrew, variation selectors, and format characters that do not show
/// (soft hyphen, zero-width joiners, direction marks). Marks that are letters
/// are kept: Indic vowel signs, kana voicing marks, Thai tone marks.
pub fn stripped(c: char) -> bool {
    matches!(
        c as u32,
        // Combining diacritical marks and their extensions and supplements,
        // the marks for symbols, and the half marks.
        0x0300..=0x036F
            | 0x1AB0..=0x1AFF
            | 0x1DC0..=0x1DFF
            | 0x20D0..=0x20FF
            | 0xFE20..=0xFE2F
            // Hebrew points and cantillation.
            | 0x0591..=0x05BD
            | 0x05BF
            | 0x05C1..=0x05C2
            | 0x05C4..=0x05C5
            | 0x05C7
            // Arabic harakat, Quranic annotation marks, superscript alef.
            | 0x0610..=0x061A
            | 0x064B..=0x065F
            | 0x0670
            | 0x06D6..=0x06DC
            | 0x06DF..=0x06E4
            | 0x06E7..=0x06E8
            | 0x06EA..=0x06ED
            // Variation selectors.
            | 0x180B..=0x180D
            | 0x180F
            | 0xFE00..=0xFE0F
            | 0xE0100..=0xE01EF
            // Invisible format characters.
            | 0x00AD
            | 0x061C
            | 0x200B..=0x200F
            | 0x202A..=0x202E
            | 0x2060..=0x2064
            | 0x2066..=0x206F
            | 0xFEFF
            | 0xE0001
            | 0xE0020..=0xE007F
    )
}

const HANGUL_FIRST: u32 = 0xAC00;
const HANGUL_LAST: u32 = 0xD7A3;

/// The Hangul syllables, which fold by arithmetic, not by table.
fn is_hangul_syllable(c: char) -> bool {
    (HANGUL_FIRST..=HANGUL_LAST).contains(&(c as u32))
}

/// White space as the ASCII rule sees it: tab, line feed, vertical tab,
/// form feed, carriage return and space.
fn is_ascii_space(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\u{0B}' | '\u{0C}' | '\r' | ' ')
}

/// One step of folding a string: decompose, leave out stripped
/// characters, case-fold, decompose again (case folding can compose),
/// leave out again; `_` and white space become spaces.
fn fold_step(s: &str) -> String {
    let d: String = s.nfkd().filter(|c| !stripped(*c)).collect();
    let f = caseless::default_case_fold_str(&d);
    f.nfkd()
        .filter(|c| !stripped(*c))
        .map(|c| {
            if c == '_' || c.is_whitespace() {
                ' '
            } else {
                c
            }
        })
        .collect()
}

/// What a code point other than ASCII and Hangul syllables folds to,
/// worked out from Unicode: [`fold_step`] repeated until nothing changes.
fn fold_unicode(c: char) -> String {
    use caseless::Caseless;
    // Most code points fold to themselves; tell those apart without
    // building strings.
    if !stripped(c)
        && !c.is_whitespace()
        && c != '_'
        && std::iter::once(c).nfkd().eq(std::iter::once(c))
        && std::iter::once(c)
            .default_case_fold()
            .eq(std::iter::once(c))
    {
        return c.to_string();
    }
    let mut s = String::from(c);
    for _ in 0..8 {
        let next = fold_step(&s);
        if next == s {
            return s;
        }
        s = next;
    }
    panic!("the fold of U+{:04X} does not settle", c as u32)
}

/// Every code point outside ASCII and the Hangul syllables whose fold is
/// not itself, with its fold, in code point order.
pub fn fold_table() -> &'static [(char, Box<str>)] {
    static TABLE: OnceLock<Vec<(char, Box<str>)>> = OnceLock::new();
    TABLE.get_or_init(|| {
        (0x80u32..=0x10FFFF)
            .filter_map(char::from_u32)
            .filter(|&c| !is_hangul_syllable(c))
            .filter_map(|c| {
                let f = fold_unicode(c);
                let mut one = [0u8; 4];
                (f != *c.encode_utf8(&mut one)).then(|| (c, f.into_boxed_str()))
            })
            .collect()
    })
}

/// Appends the fold of one code point (spec 6.1). `table` is the FOLD
/// table to use, in code point order.
pub fn fold_char_with(c: char, table: &[(char, Box<str>)], out: &mut String) {
    if c.is_ascii() {
        out.push(match c {
            'A'..='Z' => c.to_ascii_lowercase(),
            '_' => ' ',
            c if is_ascii_space(c) => ' ',
            c => c,
        });
    } else if is_hangul_syllable(c) {
        // Unicode's arithmetic decomposition into conjoining jamo.
        let s = c as u32 - HANGUL_FIRST;
        let (l, v, t) = (s / (21 * 28), (s % (21 * 28)) / 28, s % 28);
        for (base, n, keep) in [(0x1100, l, true), (0x1161, v, true), (0x11A7, t, t > 0)] {
            if keep {
                out.push(char::from_u32(base + n).expect("a jamo"));
            }
        }
    } else {
        match table.binary_search_by(|(k, _)| k.cmp(&c)) {
            Ok(i) => out.push_str(&table[i].1),
            Err(_) => out.push(c),
        }
    }
}

/// Folds a title or a query into its search key: every code point folded
/// on its own, then runs of spaces made one, none at the ends. `table` is
/// the FOLD table, in code point order.
pub fn search_key_with(title: &str, table: &[(char, Box<str>)]) -> String {
    let mut folded = String::with_capacity(title.len());
    for c in title.chars() {
        fold_char_with(c, table, &mut folded);
    }
    let mut out = String::with_capacity(folded.len());
    for word in folded.split(' ').filter(|w| !w.is_empty()) {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(word);
    }
    out
}

/// [`search_key_with`] the full table, as the converter folds titles.
pub fn search_key(title: &str) -> String {
    search_key_with(title, fold_table())
}

/// A set of code points, as a bit per code point: the characters an
/// index's keys use, or an archive's text.
#[derive(Clone)]
pub struct CharSet(Vec<u64>);

impl Default for CharSet {
    fn default() -> Self {
        Self(vec![0; 0x11_0000_usize.div_ceil(64)])
    }
}

impl std::fmt::Debug for CharSet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "CharSet({} code points)", self.len())
    }
}

impl CharSet {
    pub fn insert(&mut self, c: char) {
        let c = c as usize;
        self.0[c / 64] |= 1 << (c % 64);
    }

    pub fn insert_str(&mut self, s: &str) {
        s.chars().for_each(|c| self.insert(c));
    }

    pub fn contains(&self, c: char) -> bool {
        let c = c as usize;
        self.0[c / 64] & (1 << (c % 64)) != 0
    }

    /// Adds every code point of `other`.
    pub fn union(&mut self, other: &CharSet) {
        for (a, b) in self.0.iter_mut().zip(&other.0) {
            *a |= b;
        }
    }

    pub fn len(&self) -> usize {
        self.0.iter().map(|w| w.count_ones() as usize).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.0.iter().all(|w| *w == 0)
    }

    /// The code points, in order.
    pub fn iter(&self) -> impl Iterator<Item = char> + '_ {
        self.0.iter().enumerate().flat_map(|(i, &w)| {
            (0..64)
                .filter(move |b| w & (1 << b) != 0)
                .filter_map(move |b| char::from_u32((i * 64 + b) as u32))
        })
    }
}

/// Folds of at most this many bytes sit in their FOLD record; longer ones
/// are in the string pool after the records.
const INLINE_FOLD: usize = 3;

/// The FOLD section (spec 6.1) for an index whose keys use `keys`: every
/// entry of [`fold_table`] whose fold could occur in a key, which is every
/// fold made only of spaces and characters the keys use. (A typed
/// character whose fold uses any other cannot match a key, folded or not.)
///
/// `u32 count`, `u32 reserved`, then `count` 8-byte records in code point
/// order, then the string pool. A record is `u32 code_point`, `u8 length`
/// (of the fold in UTF-8 bytes) and three bytes: the fold itself, padded
/// with zeros, when it is at most 3 bytes long, or else the `u24` offset
/// of the fold in the pool. Records never straddle a 4096-byte block.
pub fn encode_fold_section(keys: &CharSet) -> Vec<u8> {
    let wanted: Vec<&(char, Box<str>)> = fold_table()
        .iter()
        .filter(|(_, f)| f.chars().all(|d| d == ' ' || keys.contains(d)))
        .collect();
    let mut pool: Vec<u8> = Vec::new();
    let mut placed: std::collections::HashMap<&str, u32> = std::collections::HashMap::new();
    let mut out = Vec::with_capacity(8 + wanted.len() * 8);
    out.extend_from_slice(&(wanted.len() as u32).to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    for (c, f) in &wanted {
        let bytes = f.as_bytes();
        assert!(
            bytes.len() <= u8::MAX as usize,
            "a fold of U+{:04X} is too long",
            *c as u32
        );
        out.extend_from_slice(&(*c as u32).to_le_bytes());
        out.push(bytes.len() as u8);
        if bytes.len() <= INLINE_FOLD {
            let mut inline = [0u8; INLINE_FOLD];
            inline[..bytes.len()].copy_from_slice(bytes);
            out.extend_from_slice(&inline);
        } else {
            let offset = *placed.entry(f).or_insert_with(|| {
                let at = pool.len() as u32;
                pool.extend_from_slice(bytes);
                at
            });
            assert!(offset < 1 << 24, "the FOLD string pool is too large");
            out.extend_from_slice(&offset.to_le_bytes()[..3]);
        }
    }
    out.extend_from_slice(&pool);
    out
}

/// Reads a FOLD section back into a table for [`search_key_with`].
pub fn decode_fold_section(data: &[u8]) -> Option<Vec<(char, Box<str>)>> {
    let count = u32::from_le_bytes(data.get(..4)?.try_into().ok()?) as usize;
    let end = count.checked_mul(8)?.checked_add(8)?;
    let (records, pool) = (data.get(8..end)?, &data[end..]);
    let mut table = Vec::with_capacity(count);
    let mut last = None;
    for r in records.as_chunks::<8>().0 {
        let c = char::from_u32(u32::from_le_bytes(r[..4].try_into().ok()?))?;
        let len = r[4] as usize;
        let bytes = if len <= INLINE_FOLD {
            // The padding is zero.
            if r[5 + len..].iter().any(|&b| b != 0) {
                return None;
            }
            &r[5..5 + len]
        } else {
            let offset = u32::from_le_bytes([r[5], r[6], r[7], 0]) as usize;
            pool.get(offset..offset + len)?
        };
        let fold = std::str::from_utf8(bytes).ok()?;
        // In code point order, and outside what the rules fold.
        if last.is_some_and(|l| l >= c) || c.is_ascii() || is_hangul_syllable(c) {
            return None;
        }
        last = Some(c);
        table.push((c, Box::from(fold)));
    }
    Some(table)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folds_case_accents_and_spacing() {
        assert_eq!(search_key("Albert_Einstein"), "albert einstein");
        assert_eq!(search_key("  Émile   Zola "), "emile zola");
        assert_eq!(search_key("Straße"), "strasse");
        assert_eq!(search_key("ﬁne"), "fine");
        assert_eq!(
            search_key("Ａｌｂｅｒｔ＿Ｅｉｎｓｔｅｉｎ"),
            "albert einstein"
        );
        assert_eq!(search_key("Ἀθῆναι"), "αθηναι");
        assert_eq!(search_key("İstanbul"), "istanbul");
        assert_eq!(search_key("Ёлка"), "елка");
    }

    #[test]
    fn keeps_marks_that_are_letters() {
        // A Hindi vowel sign, Japanese voicing and a Thai tone mark make
        // other words; they stay.
        assert_ne!(search_key("कि"), search_key("क"));
        assert_ne!(search_key("が"), search_key("か"));
        assert_ne!(search_key("ไก่"), search_key("ไก"));
        // Arabic and Hebrew vowel points are optional, so they go, as do
        // invisible joiners and variation selectors.
        assert_eq!(search_key("كِتَاب"), search_key("كتاب"));
        assert_eq!(search_key("שָׁלוֹם"), search_key("שלום"));
        assert_eq!(search_key("کتاب\u{200C}ها"), search_key("کتابها"));
        assert_eq!(search_key("❤\u{FE0F}"), search_key("❤"));
    }

    #[test]
    fn hangul_decomposes_by_arithmetic_as_unicode_does() {
        for c in ['가', '각', '힣', '한'] {
            let mut by_rule = String::new();
            fold_char_with(c, fold_table(), &mut by_rule);
            let by_unicode: String = c.to_string().nfkd().collect();
            assert_eq!(by_rule, by_unicode, "{c}");
        }
        // A syllable typed so far is a prefix of the syllables it can become.
        assert!(search_key("각").starts_with(&search_key("가")));
    }

    #[test]
    fn the_table_is_stable_and_folds_into_itself() {
        let table = fold_table();
        assert!(table.windows(2).all(|w| w[0].0 < w[1].0), "in order");
        for (c, f) in table {
            // Folding a fold changes nothing, so keys fold to themselves.
            let mut again = String::new();
            for d in f.chars() {
                fold_char_with(d, table, &mut again);
            }
            assert_eq!(&again, &**f, "{:04X}", *c as u32);
            assert!(
                f.len() <= 255,
                "{:04X} folds to {} bytes",
                *c as u32,
                f.len()
            );
        }
    }

    #[test]
    fn every_fold_reads_back_from_the_section() {
        let mut keys = CharSet::default();
        for (_, f) in fold_table() {
            keys.insert_str(f);
        }
        let section = encode_fold_section(&keys);
        let table = decode_fold_section(&section).expect("reads back");
        assert_eq!(table.as_slice(), fold_table());
        let pooled = table.iter().filter(|(_, f)| f.len() > INLINE_FOLD).count();
        eprintln!(
            "FOLD: {} records, {} in the pool, {} bytes",
            table.len(),
            pooled,
            section.len()
        );
        // A damaged record is refused, not misread.
        let mut bad = section.clone();
        bad[8..12].copy_from_slice(&0x41u32.to_le_bytes());
        assert!(decode_fold_section(&bad).is_none());
    }

    #[test]
    fn damage_to_a_fold_section_is_never_a_panic() {
        let mut keys = CharSet::default();
        keys.insert_str(&search_key("Émile Zola ﷺ Straße"));
        let section = encode_fold_section(&keys);
        for cut in 0..section.len().min(4096) {
            let _ = decode_fold_section(&section[..cut]);
        }
        let mut damaged = section.clone();
        for i in 0..damaged.len() {
            damaged[i] ^= 0xA5;
            let _ = decode_fold_section(&damaged);
            damaged[i] ^= 0xA5;
        }
        let mut huge = section;
        huge[..4].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(decode_fold_section(&huge).is_none());
    }

    #[test]
    fn the_fold_section_folds_as_the_full_table_does() {
        let titles = ["Émile Zola", "Straße", "Ἀθῆναι", "東京", "كتاب", "가나다"];
        let mut keys = CharSet::default();
        for t in titles {
            keys.insert_str(&search_key(t));
        }
        let section = encode_fold_section(&keys);
        let table = decode_fold_section(&section).expect("reads back");
        assert!(
            table.len() < fold_table().len() / 2,
            "only what the keys need"
        );
        // Queries in other spellings fold to the same keys through it.
        for q in [
            "ÉMILE",
            "strasse",
            "STRAẞE",
            "ἈΘῆΝΑΙ",
            "ｅｍｉｌｅ",
            "كِتَاب",
            "가",
        ] {
            assert_eq!(search_key_with(q, &table), search_key(q), "{q}");
        }
    }

    #[test]
    fn a_key_is_its_own_key() {
        for t in [
            "Ölüdeniz",
            "Ǆungla",
            "ﷺ",
            "Ⅻ",
            "Ǳ",
            "ﬄ",
            "ᾈ",
            "한국어",
            "Ｗ",
        ] {
            let k = search_key(t);
            assert_eq!(search_key(&k), k, "{t}");
        }
    }
}
