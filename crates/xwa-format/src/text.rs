//! Plain-text (Markdown-like) rendering of an article, for checking
//! converter output and for previews.

use std::fmt::Write;

use crate::content::{
    Article, Block, Cell, Formula, Infobox, InfoboxRow, Inline, ListKind, Place, Style, Table,
    Tree, TreeNode,
};

/// Renders `a` as Markdown-like text. `title_of` names link targets.
pub fn article_text(a: &Article, title_of: &dyn Fn(u32) -> Option<String>) -> String {
    let mut r = Renderer {
        out: String::new(),
        a,
        title_of,
    };
    let _ = writeln!(r.out, "# {}", a.title);
    r.blocks(&a.body, "");
    r.out
}

struct Renderer<'a> {
    out: String,
    a: &'a Article,
    title_of: &'a dyn Fn(u32) -> Option<String>,
}

impl Renderer<'_> {
    fn blocks(&mut self, blocks: &[Block], indent: &str) {
        for b in blocks {
            self.block(b, indent);
        }
    }

    fn line(&mut self, indent: &str, text: &str) {
        let _ = writeln!(self.out, "{indent}{text}");
    }

    fn block(&mut self, b: &Block, indent: &str) {
        match b {
            Block::Para(c) => {
                let t = self.inlines(c);
                self.line(indent, &t);
                self.line(indent, "");
            }
            Block::Heading { level, content } => {
                let t = format!("{} {}", "#".repeat(*level as usize), self.inlines(content));
                self.line(indent, &t);
                self.line(indent, "");
            }
            Block::List { kind, items } => {
                for (n, item) in items.iter().enumerate() {
                    let marker = match kind {
                        ListKind::Bullet => "- ".to_string(),
                        ListKind::Number => format!("{}. ", n + 1),
                        ListKind::Definition => ": ".to_string(),
                    };
                    let mut sub = Renderer {
                        out: String::new(),
                        a: self.a,
                        title_of: self.title_of,
                    };
                    sub.blocks(item, "");
                    let text = sub.out.trim_end();
                    let pad = " ".repeat(marker.chars().count());
                    for (i, l) in text.lines().filter(|l| !l.is_empty()).enumerate() {
                        let lead = if i == 0 { &marker } else { &pad };
                        let _ = writeln!(self.out, "{indent}{lead}{l}");
                    }
                    if text.is_empty() {
                        self.line(indent, marker.trim_end());
                    }
                }
                self.line(indent, "");
            }
            Block::Quote(inner) => self.blocks(inner, &format!("{indent}> ")),
            Block::Pre(t) => {
                for l in t.lines() {
                    self.line(indent, &format!("    {l}"));
                }
                self.line(indent, "");
            }
            Block::Rule => {
                self.line(indent, "----");
                self.line(indent, "");
            }
            Block::Figure { image, caption } => {
                let t = format!("[figure: image {image}] {}", self.inlines(caption));
                self.line(indent, t.trim_end());
                self.line(indent, "");
            }
            Block::MathBlock(f) => {
                self.line(indent, &format!("[display {}]", formula(f)));
                self.line(indent, "");
            }
            Block::TableRef(i) => match self.a.tables.get(*i as usize) {
                Some(t) => self.table(t, indent),
                None => self.line(indent, &format!("[missing table {i}]")),
            },
            Block::InfoboxRef => match &self.a.infobox {
                Some(ib) => self.infobox(ib, indent),
                None => self.line(indent, "[missing infobox]"),
            },
            Block::Gallery(items) => {
                self.line(indent, "[gallery]");
                for (image, caption) in items {
                    let t = format!("  image {image}: {}", self.inlines(caption));
                    self.line(indent, t.trim_end());
                }
                self.line(indent, "");
            }
            Block::Tree(t) => self.tree(t, indent),
        }
    }

    /// A tree drawn with box-drawing characters, one leaf or label to a
    /// line, as a reader might lay it out.
    fn tree(&mut self, t: &Tree, indent: &str) {
        let caption = self.inlines(&t.caption);
        if caption.is_empty() {
            self.line(indent, "[tree]");
        } else {
            self.line(indent, &format!("[tree: {caption}]"));
        }
        // A stem with nothing on it leads to the tree proper.
        let mut root = &t.root;
        while root.label.is_empty() && root.sublabel.is_empty() && root.children.len() == 1 {
            root = &root.children[0];
        }
        for l in self.tree_lines(root) {
            self.line(indent, &l);
        }
        self.line(indent, "");
    }

    fn tree_lines(&self, n: &TreeNode) -> Vec<String> {
        let mut head = self.inlines(if n.children.is_empty() {
            &n.leaf
        } else {
            &n.label
        });
        let sub = self.inlines(&n.sublabel);
        if !sub.is_empty() {
            head = format!("{head} [{sub}]").trim_start().to_string();
        }
        if n.children.is_empty() {
            return vec![head];
        }
        let mut out = Vec::new();
        if !head.is_empty() {
            out.push(head.clone());
        }
        let count = n.children.len();
        for (i, c) in n.children.iter().enumerate() {
            let last = i + 1 == count;
            for (j, l) in self.tree_lines(c).into_iter().enumerate() {
                let lead = match (j, last) {
                    (0, false) => "├─",
                    (0, true) => "└─",
                    (_, false) => "│ ",
                    (_, true) => "  ",
                };
                let sep = if j == 0 && !l.starts_with(['├', '└', '┬', '─']) {
                    " "
                } else {
                    ""
                };
                out.push(format!("{lead}{sep}{l}"));
            }
        }
        if head.is_empty() {
            // An unnamed branching point: its first branch goes on from
            // the line that leads to it.
            let first = &mut out[0];
            let joined = match first.chars().next() {
                Some('├') => first.replacen('├', "┬", 1),
                Some('└') => first.replacen('└', "─", 1),
                _ => first.clone(),
            };
            *first = joined;
        }
        out
    }

    fn table(&mut self, t: &Table, indent: &str) {
        let caption = self.inlines(&t.caption);
        let caption = if caption.is_empty() {
            String::new()
        } else {
            format!(": {caption}")
        };
        self.line(indent, &format!("[table {}x{}{caption}]", t.rows, t.cols));
        for row in table_rows(t) {
            let cells: Vec<String> = row
                .iter()
                .map(|c| {
                    let mut s = self.inlines(&c.content);
                    if c.header {
                        s = format!("**{s}**");
                    }
                    if c.rowspan > 1 || c.colspan > 1 {
                        s.push_str(&format!(" ({}x{})", c.rowspan, c.colspan));
                    }
                    s
                })
                .collect();
            self.line(indent, &format!("| {} |", cells.join(" | ")));
        }
        self.line(indent, "");
    }

    fn infobox(&mut self, ib: &Infobox, indent: &str) {
        self.line(indent, "[infobox]");
        for row in &ib.rows {
            let t = match row {
                InfoboxRow::Header(c) => format!("== {} ==", self.inlines(c)),
                InfoboxRow::Image { image, caption } => {
                    format!("[image {image}] {}", self.inlines(caption))
                }
                InfoboxRow::Field { label, value } => {
                    format!("{}: {}", self.inlines(label), self.inlines(value))
                }
                InfoboxRow::Full(c) => self.inlines(c),
            };
            self.line(indent, &format!("  {}", t.trim_end()));
        }
        self.line(indent, "");
    }

    fn inlines(&self, list: &[Inline]) -> String {
        let mut s = String::new();
        for i in list {
            match i {
                Inline::Text(t) => s.push_str(t),
                Inline::Styled(style, c) => {
                    let inner = self.inlines(c);
                    let _ = match style {
                        Style::Bold => write!(s, "**{inner}**"),
                        Style::Italic => write!(s, "*{inner}*"),
                        Style::Subscript => write!(s, "_{{{inner}}}"),
                        Style::Superscript => write!(s, "^{{{inner}}}"),
                        Style::Monospace => write!(s, "`{inner}`"),
                        Style::SmallCaps => write!(s, "<sc>{inner}</sc>"),
                    };
                }
                Inline::Link {
                    article,
                    place,
                    content,
                } => {
                    let target = (self.title_of)(*article)
                        .unwrap_or_else(|| format!("missing article {article}"));
                    let section = match place {
                        Place::Section(0) => String::new(),
                        Place::Section(n) => format!(" §{n}"),
                        Place::Anchor(hash) => format!(" §#{hash:08x}"),
                    };
                    let _ = write!(
                        s,
                        "[{}](#{article} {target}{section})",
                        self.inlines(content)
                    );
                }
                Inline::Math(f) => {
                    let _ = write!(s, "[{}]", formula(f));
                }
                Inline::Icon { image, height } => {
                    let _ = write!(s, "[icon: image {image}, {height}/16 em]");
                }
                Inline::Break => s.push_str(" / "),
                Inline::NoteRef(n) => {
                    let _ = write!(s, "[^{n}]");
                }
            }
        }
        s
    }
}

fn formula(f: &Formula) -> String {
    let mut s = format!(
        "math: {} items, {}x{}",
        f.items.len(),
        f.width,
        f.ascent + f.descent
    );
    if let Some(img) = f.fallback_image {
        let _ = write!(s, ", fallback image {img}");
    }
    s
}

/// Groups cells into the rows where their top-left corners sit.
pub fn table_rows(t: &Table) -> Vec<Vec<&Cell>> {
    let cols = t.cols.max(1) as usize;
    let mut taken: Vec<Vec<bool>> = Vec::new();
    let mut rows: Vec<Vec<&Cell>> = Vec::new();
    let (mut r, mut c) = (0usize, 0usize);
    for cell in &t.cells {
        // Next free slot in reading order.
        loop {
            if taken.len() <= r {
                taken.push(vec![false; cols]);
            }
            if c >= cols {
                r += 1;
                c = 0;
                continue;
            }
            if !taken[r][c] {
                break;
            }
            c += 1;
        }
        while rows.len() <= r {
            rows.push(Vec::new());
        }
        rows[r].push(cell);
        for dr in 0..cell.rowspan.max(1) as usize {
            while taken.len() <= r + dr {
                taken.push(vec![false; cols]);
            }
            for dc in 0..cell.colspan.max(1) as usize {
                if c + dc < cols {
                    taken[r + dr][c + dc] = true;
                }
            }
        }
        c += cell.colspan.max(1) as usize;
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cell(text: &str, rowspan: u32, colspan: u32) -> Cell {
        Cell {
            header: false,
            rowspan,
            colspan,
            content: vec![Inline::Text(text.into())],
        }
    }

    #[test]
    fn spans_place_cells_in_the_right_rows() {
        // | a (2 rows) | b     |
        // |            | c | d |   <- 3 columns
        let t = Table {
            caption: Vec::new(),
            rows: 2,
            cols: 3,
            flags: 0,
            cells: vec![
                cell("a", 2, 1),
                cell("b", 1, 2),
                cell("c", 1, 1),
                cell("d", 1, 1),
            ],
        };
        let rows: Vec<Vec<String>> = table_rows(&t)
            .iter()
            .map(|r| r.iter().map(|c| format!("{:?}", c.content)).collect())
            .collect();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].len(), 2);
        assert_eq!(rows[1].len(), 2);
    }

    #[test]
    fn renders_links_with_target_titles() {
        let a = Article {
            title: "T".into(),
            body: vec![Block::Para(vec![
                Inline::Text("see ".into()),
                Inline::Link {
                    article: 3,
                    place: Place::TOP,
                    content: vec![Inline::Styled(Style::Bold, vec![Inline::Text("x".into())])],
                },
            ])],
            ..Default::default()
        };
        let text = article_text(&a, &|id| (id == 3).then(|| "Three".to_string()));
        assert!(text.contains("see [**x**](#3 Three)"), "{text}");
    }
}
