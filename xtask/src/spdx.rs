//! The licence expressions in Cargo.toml files: `MIT OR Apache-2.0`,
//! `(MIT OR Apache-2.0) AND Unicode-3.0`, `Apache-2.0 WITH LLVM-exception`,
//! and the old `MIT/Apache-2.0`.

#[derive(Debug, PartialEq, Eq)]
pub enum Expr {
    /// A licence, with its exception if it has one.
    Id(String),
    And(Vec<Expr>),
    Or(Vec<Expr>),
}

pub fn parse(s: &str) -> Result<Expr, String> {
    let spaced = s
        .replace('/', " OR ")
        .replace('(', " ( ")
        .replace(')', " ) ");
    let tokens: Vec<&str> = spaced.split_whitespace().collect();
    let mut pos = 0;
    let expr = parse_or(&tokens, &mut pos)?;
    if pos != tokens.len() {
        return Err(format!("unexpected {:?} in licence {s:?}", tokens[pos]));
    }
    Ok(expr)
}

fn is_word(token: Option<&&str>, word: &str) -> bool {
    token.is_some_and(|t| t.eq_ignore_ascii_case(word))
}

fn parse_or(t: &[&str], pos: &mut usize) -> Result<Expr, String> {
    let mut parts = vec![parse_and(t, pos)?];
    while is_word(t.get(*pos), "OR") {
        *pos += 1;
        parts.push(parse_and(t, pos)?);
    }
    Ok(if parts.len() == 1 {
        parts.remove(0)
    } else {
        Expr::Or(parts)
    })
}

fn parse_and(t: &[&str], pos: &mut usize) -> Result<Expr, String> {
    let mut parts = vec![parse_atom(t, pos)?];
    while is_word(t.get(*pos), "AND") {
        *pos += 1;
        parts.push(parse_atom(t, pos)?);
    }
    Ok(if parts.len() == 1 {
        parts.remove(0)
    } else {
        Expr::And(parts)
    })
}

fn parse_atom(t: &[&str], pos: &mut usize) -> Result<Expr, String> {
    match t.get(*pos) {
        Some(&"(") => {
            *pos += 1;
            let inner = parse_or(t, pos)?;
            if t.get(*pos) != Some(&")") {
                return Err("unbalanced parenthesis in licence".into());
            }
            *pos += 1;
            Ok(inner)
        }
        Some(id)
            if !["AND", "OR", "WITH", ")"]
                .iter()
                .any(|w| id.eq_ignore_ascii_case(w)) =>
        {
            *pos += 1;
            let mut id = id.to_string();
            if is_word(t.get(*pos), "WITH") {
                let exception = t.get(*pos + 1).ok_or("WITH without an exception")?;
                id = format!("{id} WITH {exception}");
                *pos += 2;
            }
            Ok(Expr::Id(id))
        }
        other => Err(format!("expected a licence, found {other:?}")),
    }
}

/// The licences to comply with to use code under `expr`, using only those
/// `allowed` accepts. Where the expression offers a choice, the one whose
/// least preferred licence ranks best wins. `None` if it cannot be met.
pub fn choose(
    expr: &Expr,
    allowed: &dyn Fn(&str) -> bool,
    rank: &dyn Fn(&str) -> usize,
) -> Option<Vec<String>> {
    match expr {
        Expr::Id(id) => allowed(id).then(|| vec![id.clone()]),
        Expr::And(parts) => {
            let mut all = Vec::new();
            for p in parts {
                for id in choose(p, allowed, rank)? {
                    if !all.contains(&id) {
                        all.push(id);
                    }
                }
            }
            Some(all)
        }
        Expr::Or(parts) => parts
            .iter()
            .filter_map(|p| choose(p, allowed, rank))
            .min_by_key(|ids| ids.iter().map(|id| rank(id)).max()),
    }
}

/// Every licence the expression names.
pub fn ids(expr: &Expr) -> Vec<&str> {
    match expr {
        Expr::Id(id) => vec![id.as_str()],
        Expr::And(parts) | Expr::Or(parts) => parts.iter().flat_map(ids).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(s: &str) -> Expr {
        Expr::Id(s.into())
    }

    #[test]
    fn parses_cargo_licence_fields() {
        assert_eq!(
            parse("MIT/Apache-2.0").unwrap(),
            Expr::Or(vec![id("MIT"), id("Apache-2.0")])
        );
        assert_eq!(
            parse("(MIT OR Apache-2.0) AND Unicode-3.0").unwrap(),
            Expr::And(vec![
                Expr::Or(vec![id("MIT"), id("Apache-2.0")]),
                id("Unicode-3.0")
            ])
        );
        // AND binds tighter than OR.
        assert_eq!(
            parse("Apache-2.0 WITH LLVM-exception OR Apache-2.0 AND MIT").unwrap(),
            Expr::Or(vec![
                id("Apache-2.0 WITH LLVM-exception"),
                Expr::And(vec![id("Apache-2.0"), id("MIT")])
            ])
        );
        assert!(parse("MIT OR").is_err());
        assert!(parse("(MIT").is_err());
        assert!(parse("MIT Apache-2.0").is_err());
    }

    #[test]
    fn chooses_the_preferred_acceptable_licences() {
        let order = ["MIT", "Apache-2.0", "Unicode-3.0", "IJG"];
        let accepted = |id: &str| order.contains(&id);
        let rank = |id: &str| order.iter().position(|o| *o == id).unwrap_or(usize::MAX);
        let pick = |s: &str| choose(&parse(s).unwrap(), &accepted, &rank);
        assert_eq!(pick("Zlib OR Apache-2.0 OR MIT"), Some(vec!["MIT".into()]));
        assert_eq!(
            pick("(MIT OR Apache-2.0) AND IJG"),
            Some(vec!["MIT".into(), "IJG".into()])
        );
        assert_eq!(
            pick("Apache-2.0 OR GPL-2.0-only"),
            Some(vec!["Apache-2.0".into()])
        );
        assert_eq!(pick("GPL-3.0-or-later"), None);
        assert_eq!(pick("MIT AND GPL-2.0-only"), None);
    }
}
