//! MathML to readable text, for formulas the math engine cannot draw.
//!
//! Scripts become superscript and subscript styles, single-letter
//! variables italic, fractions `a/b` or `(a + b)/(c)`, roots `√x`. Simple
//! formulas (x², α ≤ β, a² + b² = c²) read naturally; complex ones stay
//! understandable.

use ego_tree::NodeRef;
use scraper::Node;
use xwa_format::content::{Inline, Style};

/// Converts a `<math>` element.
pub fn to_inlines(math: NodeRef<'_, Node>, display: bool) -> Vec<Inline> {
    let mut out = Vec::new();
    let mut cx = Cx { display };
    cx.children(math, &mut out);
    trim(&mut out);
    out
}

struct Cx {
    display: bool,
}

fn name<'a>(n: &NodeRef<'a, Node>) -> Option<&'a str> {
    n.value().as_element().map(|e| e.name())
}

fn elements<'a>(n: NodeRef<'a, Node>) -> impl Iterator<Item = NodeRef<'a, Node>> {
    n.children().filter(|c| c.value().is_element())
}

fn text_of(n: NodeRef<'_, Node>) -> String {
    let mut s = String::new();
    for d in n.descendants() {
        if let Node::Text(t) = d.value() {
            s.push_str(t);
        }
    }
    s.trim().to_string()
}

/// Plain text length of inlines, to decide whether parentheses are needed.
fn plain_len(list: &[Inline]) -> usize {
    list.iter()
        .map(|i| match i {
            Inline::Text(t) => t.trim().chars().count(),
            Inline::Styled(_, c) | Inline::Link { content: c, .. } => plain_len(c),
            _ => 1,
        })
        .sum()
}

fn wrap(list: Vec<Inline>) -> Vec<Inline> {
    let text = plain(&list);
    let bracketed = matches!(
        (text.chars().next(), text.chars().last()),
        (Some('('), Some(')'))
            | (Some('['), Some(']'))
            | (Some('{'), Some('}'))
            | (Some('|'), Some('|'))
            | (Some('‖'), Some('‖'))
    ) && balanced(&text);
    if plain_len(&list) <= 1 || bracketed || text.chars().all(|c| c.is_ascii_digit() || c == '.') {
        return list;
    }
    let mut out = vec![Inline::Text("(".into())];
    out.extend(list);
    out.push(Inline::Text(")".into()));
    out
}

/// Relations and binary operators, written with a space on each side.
const SPACED: &[&str] = &[
    "=", "≠", "<", ">", "≤", "≥", "≦", "≧", "≮", "≯", "≰", "≱", "≈", "≉", "≡", "≢", "≅", "≇", "∼",
    "≁", "≃", "≄", "≍", "∝", "≜", "≝", "≐", "≔", ":=", "→", "←", "↔", "⇒", "⇐", "⇔", "⟶", "⟵", "⟷",
    "⟹", "⟸", "⟺", "↦", "⟼", "↪", "↠", "↣", "⇝", "∈", "∉", "∋", "∌", "⊂", "⊃", "⊄", "⊅", "⊆", "⊇",
    "⊈", "⊉", "⊊", "⊋", "⊏", "⊐", "⊑", "⊒", "⊲", "⊳", "⊴", "⊵", "≺", "≻", "⪯", "⪰", "≼", "≽", "⊢",
    "⊣", "⊨", "⊩", "∣", "∤", "∥", "∦", "⊥", "≪", "≫", "≲", "≳", "+", "−", "-", "±", "∓", "×", "÷",
    "·", "⋅", "∘", "∗", "⋆", "∪", "∩", "⊎", "⊓", "⊔", "∧", "∨", "⊕", "⊖", "⊗", "⊘", "⊙", "∖", "≀",
    "⋉", "⋊", "⋈",
];

const INVISIBLE: &[char] = &['\u{2061}', '\u{2062}', '\u{2063}', '\u{2064}'];

fn double_struck(c: char) -> Option<char> {
    Some(match c {
        'C' => 'ℂ',
        'H' => 'ℍ',
        'N' => 'ℕ',
        'P' => 'ℙ',
        'Q' => 'ℚ',
        'R' => 'ℝ',
        'Z' => 'ℤ',
        _ => return None,
    })
}

/// Combining mark for an accent drawn over a single character.
fn combining(accent: &str) -> Option<char> {
    Some(match accent {
        "¯" | "‾" | "_" | "\u{0305}" => '\u{0305}',
        "^" | "ˆ" => '\u{0302}',
        "~" | "˜" => '\u{0303}',
        "˙" | "." => '\u{0307}',
        "¨" => '\u{0308}',
        "→" | "⃗" => '\u{20D7}',
        "´" => '\u{0301}',
        "`" => '\u{0300}',
        "˘" => '\u{0306}',
        "ˇ" => '\u{030C}',
        _ => return None,
    })
}

impl Cx {
    fn children(&mut self, n: NodeRef<'_, Node>, out: &mut Vec<Inline>) {
        let mut first = true;
        for c in elements(n) {
            self.node(c, out, first);
            first = false;
        }
    }

    fn sub(&mut self, n: Option<NodeRef<'_, Node>>) -> Vec<Inline> {
        let mut v = Vec::new();
        if let Some(n) = n {
            self.node(n, &mut v, true);
        }
        trim(&mut v);
        v
    }

    fn node(&mut self, n: NodeRef<'_, Node>, out: &mut Vec<Inline>, first: bool) {
        let Some(tag) = name(&n) else { return };
        let e = n.value().as_element().unwrap();
        let kids: Vec<NodeRef<Node>> = elements(n).collect();
        match tag {
            "annotation" | "annotation-xml" | "mphantom" | "none" | "mprescripts" => {}
            "mi" => {
                let t = text_of(n);
                let variant = e.attr("mathvariant").unwrap_or("");
                let single = t.chars().count() == 1;
                if variant == "double-struck"
                    && let Some(ds) = t.chars().next().and_then(double_struck).filter(|_| single)
                {
                    out.push(Inline::Text(ds.to_string()));
                } else if variant.contains("bold") {
                    out.push(Inline::Styled(Style::Bold, vec![Inline::Text(t)]));
                } else if single && variant != "normal" && t.chars().all(char::is_alphabetic) {
                    out.push(Inline::Styled(Style::Italic, vec![Inline::Text(t)]));
                } else {
                    out.push(Inline::Text(t));
                }
            }
            "mn" | "mtext" => out.push(Inline::Text(text_of(n))),
            "ms" => out.push(Inline::Text(format!("\"{}\"", text_of(n)))),
            "mo" => {
                let t: String = text_of(n)
                    .chars()
                    .filter(|c| !INVISIBLE.contains(c))
                    .collect();
                if t.is_empty() {
                    return;
                }
                if !first && SPACED.contains(&t.as_str()) && e.attr("form") != Some("prefix") {
                    out.push(Inline::Text(format!(" {t} ")));
                } else if t == "," || t == ";" {
                    out.push(Inline::Text(format!("{t} ")));
                } else {
                    out.push(Inline::Text(t));
                }
            }
            "mspace" => {
                let wide = e
                    .attr("width")
                    .is_some_and(|w| !w.starts_with('-') && w != "0" && w != "0em");
                if wide {
                    out.push(Inline::Text(" ".into()));
                }
            }
            "msup" | "msub" | "msubsup" => {
                out.extend(self.sub(kids.first().copied()));
                let (sub, sup) = match tag {
                    "msup" => (None, kids.get(1)),
                    "msub" => (kids.get(1), None),
                    _ => (kids.get(1), kids.get(2)),
                };
                if let Some(s) = sub {
                    out.push(Inline::Styled(Style::Subscript, self.sub(Some(*s))));
                }
                if let Some(s) = sup {
                    out.push(Inline::Styled(Style::Superscript, self.sub(Some(*s))));
                }
            }
            "munder" | "mover" | "munderover" => {
                let base = self.sub(kids.first().copied());
                let accent_attr = if tag == "munder" {
                    "accentunder"
                } else {
                    "accent"
                };
                let script = kids.get(1).map(|k| text_of(*k)).unwrap_or_default();
                // MathJax often leaves the attribute off a hat or bar.
                let is_accent = e.attr(accent_attr) == Some("true")
                    || (tag != "munderover" && combining(&script).is_some());
                if tag == "mover" && is_accent {
                    match combining(&script) {
                        Some(mark) if plain_len(&base) == 1 => {
                            let mut s = plain(&base);
                            s.push(mark);
                            out.push(styled_like(&base, s));
                        }
                        _ => {
                            out.extend(wrap(base));
                            out.push(Inline::Styled(
                                Style::Superscript,
                                vec![Inline::Text(script)],
                            ));
                        }
                    }
                    return;
                }
                if tag == "munder" && is_accent {
                    out.extend(base);
                    return;
                }
                out.extend(base);
                let (under, over) = match tag {
                    "munder" => (kids.get(1), None),
                    "mover" => (None, kids.get(1)),
                    _ => (kids.get(1), kids.get(2)),
                };
                if let Some(u) = under {
                    out.push(Inline::Styled(Style::Subscript, self.sub(Some(*u))));
                }
                if let Some(o) = over {
                    out.push(Inline::Styled(Style::Superscript, self.sub(Some(*o))));
                }
            }
            "mmultiscripts" => {
                out.extend(self.sub(kids.first().copied()));
                let mut i = 1;
                while i + 1 < kids.len() && name(&kids[i]) != Some("mprescripts") {
                    let (s, p) = (self.sub(Some(kids[i])), self.sub(Some(kids[i + 1])));
                    if !s.is_empty() {
                        out.push(Inline::Styled(Style::Subscript, s));
                    }
                    if !p.is_empty() {
                        out.push(Inline::Styled(Style::Superscript, p));
                    }
                    i += 2;
                }
            }
            "mfrac" => {
                let num = self.sub(kids.first().copied());
                let den = self.sub(kids.get(1).copied());
                if e.attr("linethickness")
                    .is_some_and(|l| l == "0" || l.starts_with("0p") || l.starts_with("0e"))
                {
                    // Binomial coefficients and stacked limits: no bar.
                    out.extend(num);
                    out.push(Inline::Text(" ".into()));
                    out.extend(den);
                } else {
                    out.extend(wrap(num));
                    out.push(Inline::Text("/".into()));
                    out.extend(wrap(den));
                }
            }
            "msqrt" => {
                let mut inner = Vec::new();
                self.children(n, &mut inner);
                trim(&mut inner);
                out.push(Inline::Text("√".into()));
                out.extend(wrap(inner));
            }
            "mroot" => {
                let radicand = self.sub(kids.first().copied());
                let index = self.sub(kids.get(1).copied());
                out.push(Inline::Styled(Style::Superscript, index));
                out.push(Inline::Text("√".into()));
                out.extend(wrap(radicand));
            }
            "mtable" => {
                let rows: Vec<NodeRef<Node>> = kids
                    .iter()
                    .copied()
                    .filter(|k| matches!(name(k), Some("mtr" | "mlabeledtr")))
                    .collect();
                for (i, row) in rows.iter().enumerate() {
                    if i > 0 {
                        if self.display {
                            out.push(Inline::Break);
                        } else {
                            out.push(Inline::Text("; ".into()));
                        }
                    }
                    let cells: Vec<NodeRef<Node>> =
                        elements(*row).filter(|c| name(c) == Some("mtd")).collect();
                    for (j, cell) in cells.iter().enumerate() {
                        if j > 0 {
                            out.push(Inline::Text("  ".into()));
                        }
                        let mut v = Vec::new();
                        self.children(*cell, &mut v);
                        trim(&mut v);
                        out.extend(v);
                    }
                }
            }
            "maction" => {
                if let Some(k) = kids.first() {
                    self.node(*k, out, first);
                }
            }
            // math, semantics, mrow, mstyle, mpadded, menclose, merror, mfenced, unknown
            _ => self.children(n, out),
        }
    }
}

/// Whether the brackets of `text` pair up, so "(a)(b)" is not one group.
fn balanced(text: &str) -> bool {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() < 2 {
        return false;
    }
    let (open, close) = (chars[0], chars[chars.len() - 1]);
    if open == close {
        // |x| or ‖x‖: no bar of the same kind inside.
        return !chars[1..chars.len() - 1].contains(&open);
    }
    let mut depth = 0i32;
    for (i, c) in chars.iter().enumerate() {
        if *c == open {
            depth += 1;
        } else if *c == close {
            depth -= 1;
            if depth == 0 && i != chars.len() - 1 {
                return false;
            }
        }
    }
    depth == 0
}

fn plain(list: &[Inline]) -> String {
    let mut s = String::new();
    for i in list {
        match i {
            Inline::Text(t) => s.push_str(t.trim()),
            Inline::Styled(_, c) | Inline::Link { content: c, .. } => s.push_str(&plain(c)),
            _ => {}
        }
    }
    s
}

/// Keeps the base's style (an italic variable stays italic under its accent).
fn styled_like(base: &[Inline], text: String) -> Inline {
    match base.first() {
        Some(Inline::Styled(style, _)) => Inline::Styled(*style, vec![Inline::Text(text)]),
        _ => Inline::Text(text),
    }
}

/// Removes leading and trailing spaces of the whole run.
fn trim(list: &mut [Inline]) {
    fn first_text(list: &mut [Inline]) -> Option<&mut String> {
        match list.first_mut()? {
            Inline::Text(t) => Some(t),
            Inline::Styled(_, c) => first_text(c),
            _ => None,
        }
    }
    fn last_text(list: &mut [Inline]) -> Option<&mut String> {
        match list.last_mut()? {
            Inline::Text(t) => Some(t),
            Inline::Styled(_, c) => last_text(c),
            _ => None,
        }
    }
    if let Some(t) = first_text(list) {
        *t = t.trim_start().to_string();
    }
    if let Some(t) = last_text(list) {
        *t = t.trim_end().to_string();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use scraper::Html;

    fn render(mathml: &str) -> String {
        let doc = Html::parse_fragment(mathml);
        let math = doc
            .tree
            .root()
            .descendants()
            .find(|n| name(n) == Some("math"))
            .unwrap();
        fn show(list: &[Inline], s: &mut String) {
            for i in list {
                match i {
                    Inline::Text(t) => s.push_str(t),
                    Inline::Styled(Style::Superscript, c) => {
                        s.push('^');
                        s.push('{');
                        show(c, s);
                        s.push('}');
                    }
                    Inline::Styled(Style::Subscript, c) => {
                        s.push_str("_{");
                        show(c, s);
                        s.push('}');
                    }
                    Inline::Styled(Style::Italic, c) => {
                        s.push('*');
                        show(c, s);
                        s.push('*');
                    }
                    Inline::Styled(_, c) => show(c, s),
                    Inline::Break => s.push_str(" // "),
                    _ => {}
                }
            }
        }
        let mut s = String::new();
        show(&to_inlines(math, false), &mut s);
        s
    }

    #[test]
    fn pythagoras() {
        let m = r#"<math alttext="a^2+b^2=c^2"><semantics><mrow><mstyle displaystyle="true">
            <msup><mi>a</mi><mrow><mn>2</mn></mrow></msup><mo>+</mo>
            <msup><mi>b</mi><mn>2</mn></msup><mo>=</mo><msup><mi>c</mi><mn>2</mn></msup>
            </mstyle></mrow><annotation encoding="application/x-tex">a^2+b^2=c^2</annotation></semantics></math>"#;
        assert_eq!(render(m), "*a*^{2} + *b*^{2} = *c*^{2}");
    }

    #[test]
    fn fractions_roots_and_functions() {
        let m = r#"<math><mfrac><mrow><mo>−</mo><mi>b</mi><mo>±</mo><msqrt><msup><mi>b</mi><mn>2</mn></msup><mo>−</mo><mn>4</mn><mi>a</mi><mi>c</mi></msqrt></mrow><mrow><mn>2</mn><mi>a</mi></mrow></mfrac></math>"#;
        assert_eq!(render(m), "(−*b* ± √(*b*^{2} − 4*a**c*))/(2*a*)");
        let m = r#"<math><mi>sin</mi><mo>&#x2061;</mo><mi>x</mi><mo>,</mo><mi mathvariant="double-struck">R</mi></math>"#;
        assert_eq!(render(m), "sin*x*, ℝ");
    }

    #[test]
    fn accents_and_limits() {
        let m = r#"<math><mover accent="true"><mi>x</mi><mo>¯</mo></mover><mo>=</mo><munderover><mo>∑</mo><mrow><mi>i</mi><mo>=</mo><mn>1</mn></mrow><mi>n</mi></munderover><msub><mi>x</mi><mi>i</mi></msub></math>"#;
        assert_eq!(render(m), "*x\u{0305}* = ∑_{*i* = 1}^{*n*}*x*_{*i*}");
    }

    #[test]
    fn matrices() {
        let m = r#"<math><mrow><mo>(</mo><mtable><mtr><mtd><mn>1</mn></mtd><mtd><mn>0</mn></mtd></mtr><mtr><mtd><mn>0</mn></mtd><mtd><mn>1</mn></mtd></mtr></mtable><mo>)</mo></mrow></math>"#;
        assert_eq!(render(m), "(1  0; 0  1)");
    }

    #[test]
    fn hats_without_attributes_and_bracketed_denominators() {
        let m = r#"<math><msub><mover><mi>f</mi><mo stretchy="false">^</mo></mover><mn>1</mn></msub><mo>≜</mo><mfrac><mn>1</mn><mrow><mo>|</mo><mi>a</mi><mo>|</mo></mrow></mfrac><mo>+</mo><mfrac><mn>1</mn><mrow><mo>(</mo><mi>a</mi><mo>)</mo><mo>(</mo><mi>b</mi><mo>)</mo></mrow></mfrac></math>"#;
        assert_eq!(render(m), "*f\u{0302}*_{1} ≜ 1/|*a*| + 1/((*a*)(*b*))");
    }
}
