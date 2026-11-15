//! Article content from Wikipedia HTML as mwoffliner writes it (Parsoid
//! output inside a skin wrapper).
//!
//! The walk keeps what reads well on an e-reader: text, headings, lists,
//! quotes, tables, the infobox, and links resolved to article ids. It drops
//! navigation boxes, edit chrome, citation markers, reference lists and
//! lists of external links. Section headings left empty by that are
//! removed, so "References" and "External links" disappear in any language.

use std::collections::HashMap;

use ego_tree::NodeRef;
use scraper::node::Element;
use scraper::{Html, Node};
use xwa_format::content::{
    Article, Block, Cell, Formula, Infobox, InfoboxRow, Inline, ListKind, Style, Table,
};
use xwa_format::records::article_flags;

use crate::images::Kind;
use crate::links::resolve_href;
use crate::mathml;

/// What extraction needs from the rest of the archive.
pub trait Resolver: Sync {
    /// Article id for a decoded archive path, after redirects.
    fn article(&self, path: &str) -> Option<u32>;
    /// A key for the image at a decoded archive path, if it is stored and
    /// images are wanted.
    fn image(&self, _path: &str) -> Option<u32> {
        None
    }
    /// A drawn formula for LaTeX source, if formulas are drawn and this one
    /// lays out.
    fn formula(&self, _tex: &str, _display: bool) -> Option<Formula> {
        None
    }
}

/// An image an article uses. The article's content refers to images by
/// their index in [`Extracted::images`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImageUse {
    /// The key [`Resolver::image`] gave.
    pub key: u32,
    pub kind: Kind,
}

/// Counts for the conversion report.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Stats {
    pub links: u64,
    /// Internal links whose target is not in the archive (kept as text).
    pub unresolved_links: u64,
    pub formulas: u64,
    /// Formulas drawn as math; the rest are written as text.
    pub formulas_drawn: u64,
    /// Images placed in articles.
    pub images: u64,
    pub images_skipped: u64,
    pub tables: u64,
    /// Tables that only make sense as drawings (cladograms).
    pub tables_dropped: u64,
    pub citations_dropped: u64,
}

impl std::ops::AddAssign for Stats {
    fn add_assign(&mut self, o: Self) {
        self.links += o.links;
        self.unresolved_links += o.unresolved_links;
        self.formulas += o.formulas;
        self.formulas_drawn += o.formulas_drawn;
        self.images += o.images;
        self.images_skipped += o.images_skipped;
        self.tables += o.tables;
        self.tables_dropped += o.tables_dropped;
        self.citations_dropped += o.citations_dropped;
    }
}

pub struct Extracted {
    pub article: Article,
    /// Flags the writer cannot work out from the content (disambiguation).
    pub flags: u16,
    pub stats: Stats,
    /// The images the content refers to, by index.
    pub images: Vec<ImageUse>,
}

/// Self links wait for their section number in this range of `section`.
const PENDING: u32 = 1 << 31;

/// Extracts article `self_id`, whose archive path is `path`.
pub fn extract(
    html: &str,
    title: &str,
    path: &str,
    self_id: u32,
    resolver: &dyn Resolver,
) -> Extracted {
    let doc = Html::parse_document(html);
    let root = content_root(&doc);
    let mut cx = Cx {
        path,
        self_id,
        resolver,
        tables: Vec::new(),
        infobox: None,
        stats: Stats::default(),
        flags: 0,
        fragments: Vec::new(),
        in_cell: 0,
        images: Vec::new(),
        image_slots: HashMap::new(),
    };
    let mut flow = Flow::default();
    cx.flow_children(root, &mut flow);
    flow.flush();
    let (body, anchors) = drop_empty_headings(flow.blocks, flow.anchors);

    // Number the remaining top-level headings as the table of contents does.
    let mut sections: HashMap<&str, u32> = HashMap::new();
    let mut n = 0;
    for (b, a) in body.iter().zip(&anchors) {
        if matches!(b, Block::Heading { .. }) {
            n += 1;
            if let Some(a) = a {
                sections.entry(a.as_str()).or_insert(n);
            }
        }
    }
    let fragments = std::mem::take(&mut cx.fragments);
    let section_of = |pending: u32| {
        let f = &fragments[(pending - PENDING) as usize];
        sections
            .get(f.as_str())
            .or_else(|| sections.get(f.replace(' ', "_").as_str()))
            .copied()
            .unwrap_or(0)
    };
    let mut article = Article {
        title: title.to_string(),
        body,
        tables: cx.tables,
        infobox: cx.infobox,
    };
    for_each_link_mut(&mut article, &mut |section| {
        if *section >= PENDING {
            *section = section_of(*section);
        }
    });
    Extracted {
        article,
        flags: cx.flags,
        stats: cx.stats,
        images: cx.images,
    }
}

/// The element holding the article text: Parsoid's `mw-parser-output`,
/// else the skin's content container, else the body.
fn content_root(doc: &Html) -> NodeRef<'_, Node> {
    let mut content_text = None;
    let mut body = None;
    for n in doc.tree.root().descendants() {
        if let Some(e) = n.value().as_element() {
            if has_class(e, "mw-parser-output") {
                return n;
            }
            if content_text.is_none() && e.id() == Some("mw-content-text") {
                content_text = Some(n);
            }
            if body.is_none() && e.name() == "body" {
                body = Some(n);
            }
        }
    }
    content_text.or(body).unwrap_or_else(|| doc.tree.root())
}

fn has_class(e: &Element, class: &str) -> bool {
    e.classes().any(|c| c == class)
}

/// Elements that never carry article text.
fn is_chrome(e: &Element) -> bool {
    const TAGS: &[&str] = &[
        "style", "script", "link", "meta", "noscript", "template", "head", "title", "button",
        "input", "select", "textarea", "form", "audio", "video", "source", "track", "map", "area",
        "iframe", "object", "embed", "svg", "canvas",
    ];
    const CLASSES: &[&str] = &[
        "noprint",
        "navbox",
        "navbox-styles",
        "vertical-navbox",
        "sidebar",
        "metadata",
        "ambox",
        "tmbox",
        "ombox",
        "cmbox",
        "fmbox",
        "imbox",
        "mbox-small",
        "sistersitebox",
        "side-box",
        "portalbox",
        "portal",
        "mw-empty-elt",
        "mw-editsection",
        "reference",
        "mw-ref",
        "reflist",
        "refbegin",
        "mw-references-wrap",
        "references",
        "mw-cite-backlink",
        "catlinks",
        "printfooter",
        "toc",
        "mw-jump-link",
        "shortdescription",
        "authority-control",
        "asbox",
        "mw-indicators",
        "geo-nondefault",
        "mwe-math-mathml-a11y",
    ];
    if TAGS.contains(&e.name()) {
        return true;
    }
    if e.classes().any(|c| CLASSES.contains(&c)) {
        return true;
    }
    if e.attr("role") == Some("navigation")
        || matches!(e.id(), Some("toc" | "catlinks" | "coordinates"))
    {
        return true;
    }
    e.attr("style").is_some_and(|s| {
        let s: String = s
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect::<String>()
            .to_ascii_lowercase();
        s.contains("display:none")
    })
}

fn is_block_tag(name: &str) -> bool {
    matches!(
        name,
        "p" | "div"
            | "section"
            | "ul"
            | "ol"
            | "dl"
            | "table"
            | "figure"
            | "blockquote"
            | "pre"
            | "hr"
            | "h1"
            | "h2"
            | "h3"
            | "h4"
            | "h5"
            | "h6"
            | "center"
    )
}

fn heading_level(name: &str) -> Option<u8> {
    match name {
        "h1" => Some(1),
        "h2" => Some(2),
        "h3" => Some(3),
        "h4" => Some(4),
        "h5" => Some(5),
        "h6" => Some(6),
        _ => None,
    }
}

fn is_display_math(e: &Element) -> bool {
    e.name() == "span" && has_class(e, "mwe-math-element") && has_class(e, "mwe-math-element-block")
}

fn style_of(e: &Element) -> Option<Style> {
    if has_class(e, "smallcaps") {
        return Some(Style::SmallCaps);
    }
    let style: String = e
        .attr("style")?
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>()
        .to_ascii_lowercase();
    if style.contains("font-style:italic") {
        Some(Style::Italic)
    } else if style.contains("font-weight:bold") || style.contains("font-weight:700") {
        Some(Style::Bold)
    } else if style.contains("font-variant:small-caps") {
        Some(Style::SmallCaps)
    } else {
        None
    }
}

fn element<'a>(n: &NodeRef<'a, Node>) -> Option<&'a Element> {
    n.value().as_element()
}

fn child_elements<'a>(
    n: NodeRef<'a, Node>,
) -> impl Iterator<Item = (NodeRef<'a, Node>, &'a Element)> {
    n.children()
        .filter_map(|c| c.value().as_element().map(|e| (c, e)))
}

fn text_content(n: NodeRef<'_, Node>) -> String {
    let mut s = String::new();
    for d in n.descendants() {
        if let Node::Text(t) = d.value() {
            s.push_str(t);
        }
    }
    s
}

/// Blocks in progress, with loose inline content gathered into a paragraph.
#[derive(Default)]
struct Flow {
    blocks: Vec<Block>,
    /// Heading anchors, parallel to `blocks`.
    anchors: Vec<Option<String>>,
    run: Vec<Inline>,
}

impl Flow {
    fn flush(&mut self) {
        let mut run = std::mem::take(&mut self.run);
        normalize(&mut run);
        if !run.is_empty() {
            self.blocks.push(Block::Para(run));
            self.anchors.push(None);
        }
    }

    fn block(&mut self, b: Block) {
        self.anchored(b, None);
    }

    fn anchored(&mut self, b: Block, anchor: Option<String>) {
        self.flush();
        self.blocks.push(b);
        self.anchors.push(anchor);
    }

    fn finish(mut self) -> Vec<Block> {
        self.flush();
        self.blocks
    }
}

struct Cx<'a> {
    path: &'a str,
    self_id: u32,
    resolver: &'a dyn Resolver,
    tables: Vec<Table>,
    infobox: Option<Infobox>,
    stats: Stats,
    flags: u16,
    fragments: Vec<String>,
    /// Inside a table cell or infobox, where tables flatten to text.
    in_cell: u32,
    images: Vec<ImageUse>,
    /// Image key to its index in `images`.
    image_slots: HashMap<u32, u32>,
}

impl Cx<'_> {
    fn flow_children(&mut self, node: NodeRef<'_, Node>, flow: &mut Flow) {
        for child in node.children() {
            self.flow_node(child, flow);
        }
    }

    fn flow_node(&mut self, node: NodeRef<'_, Node>, flow: &mut Flow) {
        let e = match node.value() {
            Node::Text(t) => {
                flow.run.push(Inline::Text(t.to_string()));
                return;
            }
            Node::Element(e) => e,
            _ => return,
        };
        if e.name() == "meta" && e.attr("property") == Some("mw:PageProp/disambiguation") {
            self.flags |= article_flags::DISAMBIGUATION;
        }
        if is_chrome(e) {
            self.count_dropped(node, e);
            return;
        }
        if let Some(level) = heading_level(e.name()) {
            self.heading(node, e, level, flow);
            return;
        }
        match e.name() {
            "p" => {
                flow.flush();
                self.flow_children(node, flow);
                flow.flush();
            }
            "ul" | "ol" => {
                if has_class(e, "gallery") {
                    self.gallery(node, flow);
                } else if self.in_cell == 0 && is_link_or_citation_list(node) {
                    self.stats.citations_dropped += 1;
                } else if let Some(b) = self.list(node, e) {
                    flow.block(b);
                }
            }
            "dl" => {
                if let Some(b) = self.definition_list(node) {
                    flow.block(b);
                }
            }
            "blockquote" => {
                let mut inner = Flow::default();
                self.flow_children(node, &mut inner);
                let blocks = inner.finish();
                if !blocks.is_empty() {
                    flow.block(Block::Quote(blocks));
                }
            }
            "pre" => {
                let text = text_content(node);
                let text = text
                    .trim_end_matches(['\n', '\r', ' '])
                    .trim_start_matches(['\n', '\r']);
                if !text.is_empty() {
                    flow.block(Block::Pre(text.to_string()));
                }
            }
            "hr" => flow.block(Block::Rule),
            "table" => self.table(node, e, flow),
            "figure" => {
                let caption = child_elements(node)
                    .find(|(_, ce)| ce.name() == "figcaption")
                    .map(|(c, _)| c);
                self.figure(node, caption, flow);
            }
            "br" => flow.run.push(Inline::Break),
            _ if e.name() == "img"
                || e.attr("typeof").is_some_and(|t| t.starts_with("mw:File")) =>
            {
                // An image on its own in running text: an icon if small, else a figure.
                if let Some(image) = self.image(node, true) {
                    if self.images[image as usize].kind == Kind::Icon {
                        flow.run.push(Inline::Icon(image));
                    } else {
                        flow.block(Block::Figure {
                            image,
                            caption: Vec::new(),
                        });
                    }
                }
            }
            _ if is_display_math(e) => match self.formula(node, true) {
                Ok(f) => flow.block(Block::MathBlock(f)),
                Err(mut run) => {
                    normalize(&mut run);
                    if !run.is_empty() {
                        flow.block(Block::Para(run));
                    }
                }
            },
            "div" if has_class(e, "mw-heading") => {
                // The heading element inside carries the level and anchor.
                match child_elements(node)
                    .find_map(|(c, ce)| heading_level(ce.name()).map(|l| (c, ce, l)))
                {
                    Some((c, ce, level)) => self.heading(c, ce, level, flow),
                    None => self.flow_children(node, flow),
                }
            }
            "div" if has_class(e, "hatnote") => {
                let mut run = Vec::new();
                self.inline_children(node, &mut run);
                normalize(&mut run);
                if !run.is_empty() {
                    flow.block(Block::Para(vec![styled(Style::Italic, run)]));
                }
            }
            "div" if has_class(e, "gallery") => self.gallery(node, flow),
            "div" if has_class(e, "thumb") => {
                let caption = node
                    .descendants()
                    .find(|d| element(d).is_some_and(|de| has_class(de, "thumbcaption")));
                self.figure(node, caption, flow);
            }
            name if is_block_tag(name) => self.flow_children(node, flow),
            // Inline wrappers that hold blocks (a transclusion span around a
            // div, say) are walked as blocks so paragraphs stay separate.
            _ if node.descendants().skip(1).any(|d| {
                element(&d).is_some_and(|de| is_block_tag(de.name()) || is_display_math(de))
            }) =>
            {
                self.flow_children(node, flow);
            }
            _ => self.inline_node(node, &mut flow.run),
        }
    }

    /// The first image in `node` (or `node` itself), as an index into
    /// `self.images`. `inline` images that are small become icons.
    fn image(&mut self, node: NodeRef<'_, Node>, inline: bool) -> Option<u32> {
        let img = node.descendants().find_map(|d| {
            element(&d).filter(|e| {
                e.name() == "img" && !e.classes().any(|c| c.starts_with("mwe-math-fallback"))
            })
        })?;
        let key = img
            .attr("src")
            .and_then(|src| resolve_href(self.path, src))
            .and_then(|target| self.resolver.image(&target.path));
        let Some(key) = key else {
            self.stats.images_skipped += 1;
            return None;
        };
        let dim = |name: &str| img.attr(name).and_then(|v| v.trim().parse::<u32>().ok());
        let small =
            matches!((dim("width"), dim("height")), (Some(w), Some(h)) if w <= 64 && h <= 64);
        let drawing = img.attr("data-file-type") == Some("drawing")
            || img.attr("src").is_some_and(|s| s.contains(".svg."));
        let kind = if inline && small {
            Kind::Icon
        } else if drawing {
            Kind::Drawing
        } else {
            Kind::Photo
        };
        self.stats.images += 1;
        let next = self.images.len() as u32;
        let images = &mut self.images;
        Some(*self.image_slots.entry(key).or_insert_with(|| {
            images.push(ImageUse { key, kind });
            next
        }))
    }

    fn figure(
        &mut self,
        node: NodeRef<'_, Node>,
        caption: Option<NodeRef<'_, Node>>,
        flow: &mut Flow,
    ) {
        let Some(image) = self.image(node, false) else {
            return;
        };
        let mut run = Vec::new();
        if let Some(c) = caption {
            self.inline_children(c, &mut run);
            normalize(&mut run);
        }
        flow.block(Block::Figure {
            image,
            caption: run,
        });
    }

    fn gallery(&mut self, node: NodeRef<'_, Node>, flow: &mut Flow) {
        let mut items = Vec::new();
        let boxes: Vec<NodeRef<Node>> = node
            .descendants()
            .filter(|d| element(d).is_some_and(|e| has_class(e, "gallerybox")))
            .collect();
        for b in boxes {
            let Some(image) = self.image(b, false) else {
                continue;
            };
            let mut caption = Vec::new();
            if let Some(t) = b
                .descendants()
                .find(|d| element(d).is_some_and(|e| has_class(e, "gallerytext")))
            {
                self.inline_children(t, &mut caption);
                normalize(&mut caption);
            }
            items.push((image, caption));
        }
        if !items.is_empty() {
            flow.block(Block::Gallery(items));
        }
    }

    fn heading(&mut self, node: NodeRef<'_, Node>, e: &Element, level: u8, flow: &mut Flow) {
        let mut content = Vec::new();
        self.inline_children(node, &mut content);
        normalize(&mut content);
        if content.is_empty() {
            return;
        }
        // Parsoid puts the anchor on the heading; the old parser on a span inside.
        let anchor = e.id().map(str::to_string).or_else(|| {
            node.descendants().find_map(|d| {
                element(&d)
                    .filter(|de| has_class(de, "mw-headline"))
                    .and_then(|de| de.id())
                    .map(str::to_string)
            })
        });
        flow.anchored(Block::Heading { level, content }, anchor);
    }

    fn count_dropped(&mut self, node: NodeRef<'_, Node>, e: &Element) {
        if has_class(e, "reference") || has_class(e, "mw-ref") {
            self.stats.citations_dropped += 1;
        } else if !matches!(e.name(), "style" | "script" | "link" | "meta") {
            self.stats.images_skipped += count_images(node);
        }
    }

    fn list(&mut self, node: NodeRef<'_, Node>, e: &Element) -> Option<Block> {
        let kind = if e.name() == "ol" {
            ListKind::Number
        } else {
            ListKind::Bullet
        };
        let mut items = Vec::new();
        for (child, ce) in child_elements(node) {
            if ce.name() != "li" || is_chrome(ce) {
                continue;
            }
            let mut f = Flow::default();
            self.flow_children(child, &mut f);
            let blocks = f.finish();
            if !blocks.is_empty() {
                items.push(blocks);
            }
        }
        if items.is_empty() {
            return None;
        }
        // Horizontal lists inside tables and infoboxes read best on one line.
        let horizontal = has_class(e, "hlist")
            || node
                .parent()
                .and_then(|p| element(&p).map(|pe| has_class(pe, "hlist")))
                .unwrap_or(false);
        if self.in_cell > 0 && horizontal {
            let mut run = Vec::new();
            for (i, item) in items.into_iter().enumerate() {
                if i > 0 {
                    run.push(Inline::Text(" · ".into()));
                }
                flatten(item, &mut run);
            }
            return Some(Block::Para(run));
        }
        Some(Block::List { kind, items })
    }

    fn definition_list(&mut self, node: NodeRef<'_, Node>) -> Option<Block> {
        let mut items = Vec::new();
        for (child, ce) in child_elements(node) {
            if is_chrome(ce) {
                continue;
            }
            match ce.name() {
                "dt" => {
                    let mut run = Vec::new();
                    self.inline_children(child, &mut run);
                    normalize(&mut run);
                    if !run.is_empty() {
                        items.push(vec![Block::Para(vec![styled(Style::Bold, run)])]);
                    }
                }
                "dd" => {
                    let mut f = Flow::default();
                    self.flow_children(child, &mut f);
                    let blocks = f.finish();
                    if !blocks.is_empty() {
                        items.push(blocks);
                    }
                }
                _ => {}
            }
        }
        (!items.is_empty()).then_some(Block::List {
            kind: ListKind::Definition,
            items,
        })
    }

    fn table(&mut self, node: NodeRef<'_, Node>, e: &Element, flow: &mut Flow) {
        if has_class(e, "clade") {
            // A cladogram draws a tree with nested tables; as text it is noise.
            self.stats.tables_dropped += 1;
            return;
        }
        if has_class(e, "infobox") && self.infobox.is_none() && self.in_cell == 0 {
            let ib = self.infobox(node);
            if !ib.rows.is_empty() {
                self.infobox = Some(ib);
                flow.block(Block::InfoboxRef);
            }
            return;
        }
        if e.attr("role") == Some("presentation") {
            // A layout table: its cells are ordinary content.
            for row in rows(node) {
                for (cell, _) in
                    child_elements(row).filter(|(_, ce)| matches!(ce.name(), "td" | "th"))
                {
                    self.flow_children(cell, flow);
                }
            }
            return;
        }
        let caption = child_elements(node)
            .find(|(_, ce)| ce.name() == "caption")
            .map(|(c, _)| {
                let mut run = Vec::new();
                self.inline_children(c, &mut run);
                normalize(&mut run);
                run
            });
        let table = self.data_table(node);
        if table.cells.is_empty() {
            return;
        }
        if self.in_cell > 0 {
            // Nested tables flatten: rows on their own lines, cells separated.
            let mut run = caption.unwrap_or_default();
            for (i, row) in table_rows(&table).into_iter().enumerate() {
                if i > 0 || !run.is_empty() {
                    run.push(Inline::Break);
                }
                for (j, cell) in row.into_iter().enumerate() {
                    if j > 0 {
                        run.push(Inline::Text(" · ".into()));
                    }
                    run.extend(cell.content.clone());
                }
            }
            flow.run.extend(run);
            return;
        }
        if let Some(c) = caption.filter(|c| !c.is_empty()) {
            flow.block(Block::Para(vec![styled(Style::Bold, c)]));
        }
        self.stats.tables += 1;
        self.tables.push(table);
        flow.block(Block::TableRef(self.tables.len() as u32 - 1));
    }

    fn data_table(&mut self, node: NodeRef<'_, Node>) -> Table {
        let rows = rows(node);
        let row_count = rows.len();
        let mut cells = Vec::new();
        // Occupancy grid to work out the column count, as a browser would.
        let mut taken: Vec<Vec<bool>> = vec![Vec::new(); row_count];
        let mut cols = 0usize;
        self.in_cell += 1;
        for (r, row) in rows.iter().enumerate() {
            let mut c = 0usize;
            for (cell, ce) in child_elements(*row) {
                if !matches!(ce.name(), "td" | "th") || is_chrome(ce) {
                    continue;
                }
                let span = |name: &str| {
                    ce.attr(name)
                        .and_then(|v| v.trim().parse::<usize>().ok())
                        .unwrap_or(1)
                        .clamp(1, 1000)
                };
                let colspan = span("colspan");
                let rowspan = span("rowspan").min(row_count - r);
                while taken[r].get(c).copied().unwrap_or(false) {
                    c += 1;
                }
                for row_taken in taken.iter_mut().skip(r).take(rowspan) {
                    if row_taken.len() < c + colspan {
                        row_taken.resize(c + colspan, false);
                    }
                    row_taken[c..c + colspan].iter_mut().for_each(|t| *t = true);
                }
                cols = cols.max(c + colspan);
                c += colspan;
                let content = self.cell_inlines(cell);
                cells.push(Cell {
                    header: ce.name() == "th",
                    rowspan: rowspan as u32,
                    colspan: colspan as u32,
                    content,
                });
            }
        }
        self.in_cell -= 1;
        // Rows and columns with nothing in them add nothing on a small screen.
        if cells.iter().all(|c| c.content.is_empty()) {
            cells.clear();
        }
        Table {
            rows: row_count as u32,
            cols: cols as u32,
            flags: 0,
            cells,
        }
    }

    fn cell_inlines(&mut self, cell: NodeRef<'_, Node>) -> Vec<Inline> {
        let mut f = Flow::default();
        self.flow_children(cell, &mut f);
        let mut run = Vec::new();
        flatten(f.finish(), &mut run);
        normalize(&mut run);
        run
    }

    fn infobox(&mut self, node: NodeRef<'_, Node>) -> Infobox {
        let mut out = Vec::new();
        self.in_cell += 1;
        for (c, ce) in child_elements(node) {
            if ce.name() == "caption" {
                let mut run = Vec::new();
                self.inline_children(c, &mut run);
                normalize(&mut run);
                if !run.is_empty() {
                    out.push(InfoboxRow::Header(run));
                }
            }
        }
        for row in rows(node) {
            let cells: Vec<(NodeRef<Node>, &Element)> = child_elements(row)
                .filter(|(_, ce)| matches!(ce.name(), "td" | "th") && !is_chrome(ce))
                .collect();
            match cells.as_slice() {
                [] => {}
                [(cell, ce)] => {
                    if count_images(*cell) > 0
                        && (has_class(ce, "infobox-image")
                            || text_content(*cell).trim().len() < 200)
                    {
                        // An image row. Without the image (pictures off, or
                        // not in the ZIM) its caption means little.
                        if let Some(image) = self.image(*cell, false) {
                            let caption = cell
                                .descendants()
                                .find(|d| {
                                    element(d).is_some_and(|de| has_class(de, "infobox-caption"))
                                })
                                .map(|c| {
                                    let mut run = Vec::new();
                                    self.inline_children(c, &mut run);
                                    normalize(&mut run);
                                    run
                                })
                                .unwrap_or_default();
                            out.push(InfoboxRow::Image { image, caption });
                        }
                        continue;
                    }
                    let content = self.cell_inlines(*cell);
                    if content.is_empty() {
                        continue;
                    }
                    let header = ce.name() == "th"
                        || [
                            "infobox-above",
                            "infobox-header",
                            "infobox-title",
                            "infobox-subheader",
                        ]
                        .iter()
                        .any(|c| has_class(ce, c));
                    out.push(if header {
                        InfoboxRow::Header(content)
                    } else {
                        InfoboxRow::Full(content)
                    });
                }
                [(label, _), values @ ..] => {
                    let label = self.cell_inlines(*label);
                    let mut value = Vec::new();
                    for (i, (v, _)) in values.iter().enumerate() {
                        let part = self.cell_inlines(*v);
                        if part.is_empty() {
                            continue;
                        }
                        if i > 0 && !value.is_empty() {
                            value.push(Inline::Text(" · ".into()));
                        }
                        value.extend(part);
                    }
                    match (label.is_empty(), value.is_empty()) {
                        (_, true) if label.is_empty() => {}
                        (false, false) => out.push(InfoboxRow::Field { label, value }),
                        (true, false) => out.push(InfoboxRow::Full(value)),
                        (false, true) => out.push(InfoboxRow::Header(label)),
                        (true, true) => {}
                    }
                }
            }
        }
        self.in_cell -= 1;
        for row in &mut out {
            if let InfoboxRow::Field { label, .. } = row {
                // Taxoboxes write "Kingdom:"; the reader adds its own separator.
                trim_trailing(label, ':');
            }
        }
        // Section headers whose rows were all images or empty.
        let header = |r: Option<&InfoboxRow>| matches!(r, None | Some(InfoboxRow::Header(_)));
        let keep: Vec<bool> = (0..out.len())
            .map(|i| i == 0 || !header(out.get(i)) || !header(out.get(i + 1)))
            .collect();
        let rows = out
            .into_iter()
            .zip(keep)
            .filter(|(_, k)| *k)
            .map(|(r, _)| r)
            .collect();
        Infobox { rows }
    }

    fn inline_children(&mut self, node: NodeRef<'_, Node>, out: &mut Vec<Inline>) {
        for child in node.children() {
            self.inline_node(child, out);
        }
    }

    fn inline_node(&mut self, node: NodeRef<'_, Node>, out: &mut Vec<Inline>) {
        let e = match node.value() {
            Node::Text(t) => {
                out.push(Inline::Text(t.to_string()));
                return;
            }
            Node::Element(e) => e,
            _ => return,
        };
        if is_chrome(e) {
            self.count_dropped(node, e);
            return;
        }
        let style = match e.name() {
            "b" | "strong" => Some(Style::Bold),
            "i" | "em" | "var" | "dfn" | "cite" => Some(Style::Italic),
            "sub" => Some(Style::Subscript),
            "sup" => Some(Style::Superscript),
            "code" | "kbd" | "samp" | "tt" => Some(Style::Monospace),
            _ => style_of(e),
        };
        match e.name() {
            "br" if !is_script_stack(node) => out.push(Inline::Break),
            "br" => {}
            "a" => self.link(node, e, out),
            "img" => {
                if let Some(image) = self.image(node, true) {
                    out.push(Inline::Icon(image));
                }
            }
            "figure" => {
                if let Some(image) = self.image(node, true) {
                    out.push(Inline::Icon(image));
                }
            }
            "math" => match self.formula(node, false) {
                Ok(f) => out.push(Inline::Math(f)),
                Err(run) => out.extend(run),
            },
            "span" if has_class(e, "mwe-math-element") => {
                match self.formula(node, is_display_math(e)) {
                    Ok(f) => out.push(Inline::Math(f)),
                    Err(run) => out.extend(run),
                }
            }
            "table" => {
                let mut f = Flow::default();
                self.in_cell += 1;
                self.table(node, e, &mut f);
                self.in_cell -= 1;
                if !out.is_empty() {
                    out.push(Inline::Break);
                }
                flatten(f.finish(), out);
            }
            "ul" | "ol" | "dl" => {
                let mut f = Flow::default();
                self.flow_node(node, &mut f);
                if !out.is_empty() {
                    out.push(Inline::Break);
                }
                flatten(f.finish(), out);
            }
            name => {
                if is_block_tag(name) && !out.is_empty() {
                    out.push(Inline::Break);
                }
                let mut inner = Vec::new();
                self.inline_children(node, &mut inner);
                match style {
                    Some(s) => out.push(Inline::Styled(s, inner)),
                    None => out.extend(inner),
                }
            }
        }
    }

    fn link(&mut self, node: NodeRef<'_, Node>, e: &Element, out: &mut Vec<Inline>) {
        let mut content = Vec::new();
        self.inline_children(node, &mut content);
        let internal = e
            .attr("rel")
            .is_some_and(|r| r.split_whitespace().any(|t| t == "mw:WikiLink"))
            || (!has_class(e, "external") && e.attr("rel").is_none());
        let target = match e.attr("href") {
            Some(href) if internal && !has_class(e, "mw-selflink") => resolve_href(self.path, href),
            _ => None,
        };
        let Some(target) = target else {
            out.extend(content);
            return;
        };
        self.stats.links += 1;
        let Some(article) = self.resolver.article(&target.path) else {
            self.stats.unresolved_links += 1;
            out.extend(content);
            return;
        };
        let section = match target.fragment {
            Some(f) if article == self.self_id => {
                self.fragments.push(f);
                PENDING + self.fragments.len() as u32 - 1
            }
            _ => 0,
        };
        out.push(Inline::Link {
            article,
            section,
            content,
        });
    }

    /// A formula as text, from its MathML; LaTeX source if there is none.
    /// A formula: drawn from its LaTeX source when possible, else as text
    /// from its MathML (or the LaTeX itself if there is no MathML).
    fn formula(&mut self, node: NodeRef<'_, Node>, display: bool) -> Result<Formula, Vec<Inline>> {
        self.stats.formulas += 1;
        let math = node
            .descendants()
            .find(|d| element(d).is_some_and(|e| e.name() == "math"));
        let tex = math
            .and_then(|m| element(&m).and_then(|e| e.attr("alttext")))
            .or_else(|| {
                node.descendants().find_map(|d| {
                    element(&d)
                        .filter(|e| e.name() == "img")
                        .and_then(|e| e.attr("alt"))
                })
            })
            .map(str::trim);
        if let Some(f) = tex.and_then(|t| self.resolver.formula(t, display)) {
            self.stats.formulas_drawn += 1;
            return Ok(f);
        }
        let mut run = math
            .map(|m| mathml::to_inlines(m, display))
            .unwrap_or_default();
        if run.is_empty()
            && let Some(tex) = tex
        {
            let tex = tex
                .strip_prefix("{\\displaystyle")
                .and_then(|t| t.strip_suffix('}'))
                .unwrap_or(tex);
            run.push(Inline::Text(tex.trim().to_string()));
        }
        Err(run)
    }
}

/// `style` around `run`, without nesting the same style inside.
fn styled(style: Style, run: Vec<Inline>) -> Inline {
    fn unwrap(list: Vec<Inline>, style: Style) -> Vec<Inline> {
        let mut out = Vec::with_capacity(list.len());
        for i in list {
            match i {
                Inline::Styled(s, c) if s == style => out.extend(unwrap(c, style)),
                Inline::Styled(s, c) => out.push(Inline::Styled(s, unwrap(c, style))),
                Inline::Link {
                    article,
                    section,
                    content,
                } => out.push(Inline::Link {
                    article,
                    section,
                    content: unwrap(content, style),
                }),
                other => out.push(other),
            }
        }
        out
    }
    let mut inner = unwrap(run, style);
    merge_text(&mut inner);
    Inline::Styled(style, inner)
}

/// A `<br>` between a superscript and a subscript stacks them (chemical
/// formula templates); it is not a line break.
fn is_script_stack(br: NodeRef<'_, Node>) -> bool {
    let Some(parent) = br.parent() else {
        return false;
    };
    let names: Vec<&str> = child_elements(parent).map(|(_, e)| e.name()).collect();
    names.contains(&"sup")
        && names.contains(&"sub")
        && names.iter().all(|n| matches!(*n, "sup" | "sub" | "br"))
}

/// Removes a trailing `c` (and spaces) from the end of a run.
fn trim_trailing(run: &mut Vec<Inline>, c: char) {
    match run.last_mut() {
        Some(Inline::Text(t)) => {
            let trimmed = t.trim_end().trim_end_matches(c).trim_end().len();
            t.truncate(trimmed);
            if t.is_empty() {
                run.pop();
            }
        }
        Some(Inline::Styled(_, inner)) | Some(Inline::Link { content: inner, .. }) => {
            trim_trailing(inner, c);
            if inner.is_empty() {
                run.pop();
            }
        }
        _ => {}
    }
}

/// Table rows that belong to this table (not to tables nested in its cells).
fn rows(table: NodeRef<'_, Node>) -> Vec<NodeRef<'_, Node>> {
    let mut out = Vec::new();
    for (c, ce) in child_elements(table) {
        match ce.name() {
            "tr" if !is_chrome(ce) => out.push(c),
            "thead" | "tbody" | "tfoot" => out.extend(
                child_elements(c)
                    .filter(|(_, re)| re.name() == "tr" && !is_chrome(re))
                    .map(|(r, _)| r),
            ),
            _ => {}
        }
    }
    out
}

fn count_images(node: NodeRef<'_, Node>) -> u64 {
    node.descendants()
        .filter(|d| {
            element(d).is_some_and(|e| {
                e.name() == "img" && !e.classes().any(|c| c.starts_with("mwe-math-fallback"))
            })
        })
        .count() as u64
}

/// A list whose every item is a citation or an external link: a
/// bibliography or an "External links" section.
fn is_link_or_citation_list(list: NodeRef<'_, Node>) -> bool {
    let mut items = 0;
    for (li, le) in child_elements(list) {
        if le.name() != "li" {
            continue;
        }
        items += 1;
        let mut citation = false;
        let mut external = false;
        let mut internal = false;
        for d in li.descendants() {
            let Some(e) = element(&d) else { continue };
            match e.name() {
                "cite" => citation = true,
                "span" if has_class(e, "citation") => citation = true,
                "a" if has_class(e, "external") => external = true,
                "a" if e.attr("rel").is_some_and(|r| r.contains("mw:WikiLink")) => internal = true,
                _ => {}
            }
        }
        if !(citation || (external && !internal)) {
            return false;
        }
    }
    items > 0
}

/// Cells grouped by the row their top-left corner is in.
fn table_rows(t: &Table) -> Vec<Vec<&Cell>> {
    let cols = t.cols.max(1) as usize;
    let mut taken: Vec<Vec<bool>> = Vec::new();
    let mut rows: Vec<Vec<&Cell>> = Vec::new();
    let (mut r, mut c) = (0usize, 0usize);
    for cell in &t.cells {
        loop {
            if taken.len() <= r {
                taken.push(vec![false; cols]);
            }
            if c >= cols {
                r += 1;
                c = 0;
            } else if taken[r][c] {
                c += 1;
            } else {
                break;
            }
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

/// Blocks as one inline run, for table cells and infobox values.
fn flatten(blocks: Vec<Block>, out: &mut Vec<Inline>) {
    for b in blocks {
        let start = out.len();
        let sep = |out: &mut Vec<Inline>| {
            if !out.is_empty() && !matches!(out.last(), Some(Inline::Break)) {
                out.push(Inline::Break);
            }
        };
        match b {
            Block::Para(run)
            | Block::Heading { content: run, .. }
            | Block::Figure { caption: run, .. } => {
                sep(out);
                out.extend(run);
            }
            Block::List { kind, items } => {
                let single = items.len() == 1;
                for (i, item) in items.into_iter().enumerate() {
                    let mut text = Vec::new();
                    flatten(item, &mut text);
                    if text.is_empty() {
                        continue;
                    }
                    sep(out);
                    match kind {
                        _ if single => {}
                        ListKind::Number => out.push(Inline::Text(format!("{}. ", i + 1))),
                        ListKind::Bullet => out.push(Inline::Text("• ".into())),
                        ListKind::Definition => {}
                    }
                    out.extend(text);
                }
            }
            Block::Quote(inner) => {
                sep(out);
                flatten(inner, out);
            }
            Block::Pre(t) => {
                sep(out);
                out.push(Inline::Styled(Style::Monospace, vec![Inline::Text(t)]));
            }
            Block::MathBlock(f) => {
                sep(out);
                out.push(Inline::Math(f));
            }
            Block::Rule | Block::TableRef(_) | Block::InfoboxRef | Block::Gallery(_) => {}
        }
        if out.len() == start + 1 && matches!(out.last(), Some(Inline::Break)) {
            out.pop();
        }
    }
}

/// Collapses whitespace the way a browser does, merges adjacent text,
/// drops empty styles and links, and trims the ends of the run.
fn normalize(run: &mut Vec<Inline>) {
    fn walk(list: &mut Vec<Inline>, prev_space: &mut bool) {
        let mut out: Vec<Inline> = Vec::with_capacity(list.len());
        for mut item in list.drain(..) {
            match &mut item {
                Inline::Text(t) => {
                    let mut s = String::with_capacity(t.len());
                    // Word joiners and byte-order marks are invisible but
                    // may draw as boxes in a device font.
                    for c in t.chars().filter(|c| !matches!(c, '\u{2060}' | '\u{feff}')) {
                        if c.is_whitespace() && c != '\u{a0}' {
                            if !*prev_space {
                                s.push(' ');
                                *prev_space = true;
                            }
                        } else {
                            s.push(c);
                            *prev_space = false;
                        }
                    }
                    if s.is_empty() {
                        continue;
                    }
                    if let Some(Inline::Text(p)) = out.last_mut() {
                        p.push_str(&s);
                        continue;
                    }
                    *t = s;
                }
                Inline::Styled(style, c) => {
                    walk(c, prev_space);
                    if c.is_empty() {
                        continue;
                    }
                    // A style around nothing but (non-breaking) spaces, such
                    // as a superscript that held an icon, is just a space.
                    if c.iter()
                        .all(|i| matches!(i, Inline::Text(t) if t.chars().all(char::is_whitespace)))
                    {
                        if !*prev_space && !out.is_empty() {
                            out.push(Inline::Text(" ".into()));
                            *prev_space = true;
                        }
                        continue;
                    }
                    // Adjacent runs in the same style become one ("d" "s" -> "ds").
                    if let Some(Inline::Styled(prev, pc)) = out.last_mut()
                        && prev == style
                    {
                        let mut merged = std::mem::take(pc);
                        merged.append(c);
                        merge_text(&mut merged);
                        *pc = merged;
                        continue;
                    }
                }
                Inline::Link { content: c, .. } => {
                    walk(c, prev_space);
                    if c.is_empty() {
                        continue;
                    }
                }
                Inline::Break => {
                    trim_end(&mut out);
                    if out.is_empty() || matches!(out.last(), Some(Inline::Break)) {
                        continue;
                    }
                    *prev_space = true;
                }
                Inline::Math(_) | Inline::Icon(_) | Inline::NoteRef(_) => *prev_space = false,
            }
            out.push(item);
        }
        *list = out;
    }
    let mut prev_space = true;
    walk(run, &mut prev_space);
    trim_end(run);
}

/// Joins adjacent text items (after a merge of two styled runs).
fn merge_text(list: &mut Vec<Inline>) {
    let mut out: Vec<Inline> = Vec::with_capacity(list.len());
    for item in list.drain(..) {
        if let (Some(Inline::Text(p)), Inline::Text(t)) = (out.last_mut(), &item) {
            p.push_str(t);
            continue;
        }
        out.push(item);
    }
    *list = out;
}

/// Removes trailing spaces and breaks, at any depth.
fn trim_end(list: &mut Vec<Inline>) {
    loop {
        match list.last_mut() {
            Some(Inline::Break) => {
                list.pop();
            }
            Some(Inline::Text(t)) => {
                let trimmed = t.trim_end_matches(' ').len();
                t.truncate(trimmed);
                if t.is_empty() {
                    list.pop();
                } else {
                    return;
                }
            }
            Some(Inline::Styled(_, c)) | Some(Inline::Link { content: c, .. }) => {
                trim_end(c);
                if c.is_empty() {
                    list.pop();
                } else {
                    return;
                }
            }
            _ => return,
        }
    }
}

/// Drops headings with nothing under them before the next heading of the
/// same or a higher level.
fn drop_empty_headings(
    blocks: Vec<Block>,
    anchors: Vec<Option<String>>,
) -> (Vec<Block>, Vec<Option<String>>) {
    let level = |b: &Block| match b {
        Block::Heading { level, .. } => Some(*level),
        _ => None,
    };
    let keep: Vec<bool> = (0..blocks.len())
        .map(|i| match level(&blocks[i]) {
            None => true,
            Some(l) => blocks[i + 1..]
                .iter()
                .map(level)
                .take_while(|n| n.is_none_or(|n| n > l))
                .any(|n| n.is_none()),
        })
        .collect();
    blocks
        .into_iter()
        .zip(anchors)
        .zip(keep)
        .filter(|(_, k)| *k)
        .map(|(ba, _)| ba)
        .unzip()
}

/// Calls `f` on every formula in the article.
pub fn for_each_formula_mut(a: &mut Article, f: &mut impl FnMut(&mut Formula)) {
    fn inlines(list: &mut [Inline], f: &mut impl FnMut(&mut Formula)) {
        for i in list {
            match i {
                Inline::Math(x) => f(x),
                Inline::Styled(_, c) | Inline::Link { content: c, .. } => inlines(c, f),
                Inline::Text(_) | Inline::Icon(_) | Inline::Break | Inline::NoteRef(_) => {}
            }
        }
    }
    fn blocks(list: &mut [Block], f: &mut impl FnMut(&mut Formula)) {
        for b in list {
            match b {
                Block::Para(c)
                | Block::Heading { content: c, .. }
                | Block::Figure { caption: c, .. } => inlines(c, f),
                Block::Gallery(items) => items.iter_mut().for_each(|(_, c)| inlines(c, f)),
                Block::List { items, .. } => items.iter_mut().for_each(|i| blocks(i, f)),
                Block::Quote(inner) => blocks(inner, f),
                Block::MathBlock(x) => f(x),
                Block::Pre(_) | Block::Rule | Block::TableRef(_) | Block::InfoboxRef => {}
            }
        }
    }
    blocks(&mut a.body, f);
    for t in &mut a.tables {
        t.cells.iter_mut().for_each(|c| inlines(&mut c.content, f));
    }
    if let Some(ib) = &mut a.infobox {
        for row in &mut ib.rows {
            match row {
                InfoboxRow::Header(c)
                | InfoboxRow::Full(c)
                | InfoboxRow::Image { caption: c, .. } => inlines(c, f),
                InfoboxRow::Field { label, value } => {
                    inlines(label, f);
                    inlines(value, f);
                }
            }
        }
    }
}

/// Calls `f` on every image id in the article.
pub fn for_each_image_mut(a: &mut Article, f: &mut impl FnMut(&mut u32)) {
    fn formula(x: &mut xwa_format::content::Formula, f: &mut impl FnMut(&mut u32)) {
        if let Some(i) = &mut x.fallback_image {
            f(i);
        }
    }
    fn inlines(list: &mut [Inline], f: &mut impl FnMut(&mut u32)) {
        for i in list {
            match i {
                Inline::Icon(image) => f(image),
                Inline::Math(x) => formula(x, f),
                Inline::Styled(_, c) | Inline::Link { content: c, .. } => inlines(c, f),
                Inline::Text(_) | Inline::Break | Inline::NoteRef(_) => {}
            }
        }
    }
    fn blocks(list: &mut [Block], f: &mut impl FnMut(&mut u32)) {
        for b in list {
            match b {
                Block::Para(c) | Block::Heading { content: c, .. } => inlines(c, f),
                Block::Figure { image, caption } => {
                    f(image);
                    inlines(caption, f);
                }
                Block::Gallery(items) => {
                    for (image, caption) in items {
                        f(image);
                        inlines(caption, f);
                    }
                }
                Block::List { items, .. } => items.iter_mut().for_each(|i| blocks(i, f)),
                Block::Quote(inner) => blocks(inner, f),
                Block::MathBlock(x) => formula(x, f),
                Block::Pre(_) | Block::Rule | Block::TableRef(_) | Block::InfoboxRef => {}
            }
        }
    }
    blocks(&mut a.body, f);
    for t in &mut a.tables {
        t.cells.iter_mut().for_each(|c| inlines(&mut c.content, f));
    }
    if let Some(ib) = &mut a.infobox {
        for row in &mut ib.rows {
            match row {
                InfoboxRow::Image { image, caption } => {
                    f(image);
                    inlines(caption, f);
                }
                InfoboxRow::Header(c) | InfoboxRow::Full(c) => inlines(c, f),
                InfoboxRow::Field { label, value } => {
                    inlines(label, f);
                    inlines(value, f);
                }
            }
        }
    }
}

fn for_each_link_mut(a: &mut Article, f: &mut impl FnMut(&mut u32)) {
    fn inlines(list: &mut [Inline], f: &mut impl FnMut(&mut u32)) {
        for i in list {
            match i {
                Inline::Link {
                    section, content, ..
                } => {
                    f(section);
                    inlines(content, f);
                }
                Inline::Styled(_, c) => inlines(c, f),
                _ => {}
            }
        }
    }
    fn blocks(list: &mut [Block], f: &mut impl FnMut(&mut u32)) {
        for b in list {
            match b {
                Block::Para(c)
                | Block::Heading { content: c, .. }
                | Block::Figure { caption: c, .. } => inlines(c, f),
                Block::List { items, .. } => items.iter_mut().for_each(|i| blocks(i, f)),
                Block::Quote(inner) => blocks(inner, f),
                Block::Gallery(items) => items.iter_mut().for_each(|(_, c)| inlines(c, f)),
                Block::Pre(_)
                | Block::Rule
                | Block::MathBlock(_)
                | Block::TableRef(_)
                | Block::InfoboxRef => {}
            }
        }
    }
    blocks(&mut a.body, f);
    for t in &mut a.tables {
        t.cells.iter_mut().for_each(|c| inlines(&mut c.content, f));
    }
    if let Some(ib) = &mut a.infobox {
        for row in &mut ib.rows {
            match row {
                InfoboxRow::Header(c)
                | InfoboxRow::Full(c)
                | InfoboxRow::Image { caption: c, .. } => inlines(c, f),
                InfoboxRow::Field { label, value } => {
                    inlines(label, f);
                    inlines(value, f);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Map(HashMap<&'static str, u32>);

    impl Resolver for Map {
        fn article(&self, path: &str) -> Option<u32> {
            self.0.get(path).copied()
        }
    }

    fn run(body: &str) -> Extracted {
        let html = format!(
            "<!DOCTYPE html><html><head><title>T</title></head><body><div id=\"mw-content-text\">\
             <div class=\"mw-parser-output\">{body}</div></div><div id=\"catlinks\">Categories: X</div></body></html>"
        );
        let map = Map(HashMap::from([
            ("Mathematics", 3),
            ("Euclid's_Elements", 4),
            ("Self", 0),
            ("Geometry", 5),
        ]));
        extract(&html, "Self", "Self", 0, &map)
    }

    fn text(s: &str) -> Inline {
        Inline::Text(s.to_string())
    }

    #[test]
    fn paragraphs_links_and_styles() {
        let x = run(
            r##"<section data-mw-section-id="0"><p>In  <a rel="mw:WikiLink" href="Mathematics">mathematics</a>,
            the <b>theorem</b> of <a rel="mw:WikiLink" href="./Euclid%27s_Elements#Book_I"><i>Euclid</i></a> is
            <a rel="mw:WikiLink" href="Missing_page">missing</a> and <a rel="mw:ExtLink" class="external" href="https://x.org">out</a>.<sup class="mw-ref reference"><a href="#cite_note-1">[1]</a></sup> </p>
            <p> </p><p class="mw-empty-elt"></p></section>"##,
        );
        assert_eq!(
            x.article.body,
            vec![Block::Para(vec![
                text("In "),
                Inline::Link {
                    article: 3,
                    section: 0,
                    content: vec![text("mathematics")]
                },
                text(", the "),
                Inline::Styled(Style::Bold, vec![text("theorem")]),
                text(" of "),
                Inline::Link {
                    article: 4,
                    section: 0,
                    content: vec![Inline::Styled(Style::Italic, vec![text("Euclid")])]
                },
                text(" is missing and out."),
            ])]
        );
        assert_eq!(
            (
                x.stats.links,
                x.stats.unresolved_links,
                x.stats.citations_dropped
            ),
            (3, 1, 1)
        );
    }

    #[test]
    fn headings_sections_and_self_links() {
        let x = run(
            r##"<section data-mw-section-id="0"><p>See <a rel="mw:WikiLink" href="#Proofs">below</a> and <a rel="mw:WikiLink" href="Self#History">history</a>.</p></section>
            <section data-mw-section-id="1"><div class="mw-heading mw-heading2"><h2 id="History">History</h2></div><p>Old.</p>
              <section data-mw-section-id="2"><div class="mw-heading mw-heading3"><h3 id="Empty">Empty</h3></div></section></section>
            <section data-mw-section-id="3"><div class="mw-heading mw-heading2"><h2 id="Proofs">Proofs</h2></div><p>Many.</p></section>
            <section data-mw-section-id="4"><div class="mw-heading mw-heading2"><h2 id="References">References</h2></div>
              <div class="mw-references-wrap"><ol class="mw-references references"><li>A source</li></ol></div></section>
            <section data-mw-section-id="5"><div class="mw-heading mw-heading2"><h2 id="External_links">External links</h2></div>
              <ul><li><a rel="mw:ExtLink" class="external" href="https://a.org">A site</a></li><li><a class="external" href="https://b.org">B</a> at B</li></ul></section>"##,
        );
        let headings: Vec<String> = x
            .article
            .body
            .iter()
            .filter_map(|b| match b {
                Block::Heading { content, .. } => Some(xwa_format::content::plain_text(content)),
                _ => None,
            })
            .collect();
        assert_eq!(headings, ["History", "Proofs"]);
        let Block::Para(first) = &x.article.body[0] else {
            panic!()
        };
        assert_eq!(
            first[1],
            Inline::Link {
                article: 0,
                section: 2,
                content: vec![text("below")]
            }
        );
        assert_eq!(
            first[3],
            Inline::Link {
                article: 0,
                section: 1,
                content: vec![text("history")]
            }
        );
    }

    #[test]
    fn display_math_splits_its_paragraph() {
        let x = run(r#"<p>The equation:
            <span class="mwe-math-element mwe-math-element-block"><span class="mwe-math-mathml-display mwe-math-mathml-a11y" style="display: none;"><math display="block" alttext="x^2"><semantics><msup><mi>x</mi><mn>2</mn></msup></semantics></math></span><img class="mwe-math-fallback-image-display" alt="x^2" src="./_assets_/a/b.svg"></span>
            holds. Also <span class="mwe-math-element mwe-math-element-inline"><span class="mwe-math-mathml-inline mwe-math-mathml-a11y" style="display: none;"><math alttext="y"><mi>y</mi></math></span><img class="mwe-math-fallback-image-inline" alt="y"></span>.</p>"#);
        assert_eq!(
            x.article.body,
            vec![
                Block::Para(vec![text("The equation:")]),
                Block::Para(vec![
                    Inline::Styled(Style::Italic, vec![text("x")]),
                    Inline::Styled(Style::Superscript, vec![text("2")])
                ]),
                Block::Para(vec![
                    text("holds. Also "),
                    Inline::Styled(Style::Italic, vec![text("y")]),
                    text(".")
                ]),
            ]
        );
        assert_eq!((x.stats.formulas, x.stats.images_skipped), (2, 0));
    }

    #[test]
    fn lists_and_definition_lists() {
        let x = run(
            r#"<ul><li>One<ul><li>One a</li></ul></li><li></li><li>Two</li></ul><dl><dt>Term</dt><dd>Meaning</dd></dl>"#,
        );
        assert_eq!(
            x.article.body,
            vec![
                Block::List {
                    kind: ListKind::Bullet,
                    items: vec![
                        vec![
                            Block::Para(vec![text("One")]),
                            Block::List {
                                kind: ListKind::Bullet,
                                items: vec![vec![Block::Para(vec![text("One a")])]]
                            }
                        ],
                        vec![Block::Para(vec![text("Two")])],
                    ]
                },
                Block::List {
                    kind: ListKind::Definition,
                    items: vec![
                        vec![Block::Para(vec![Inline::Styled(
                            Style::Bold,
                            vec![text("Term")]
                        )])],
                        vec![Block::Para(vec![text("Meaning")])]
                    ]
                },
            ]
        );
    }

    #[test]
    fn tables_with_spans_and_captions() {
        let x = run(r#"<table class="wikitable"><caption>Sides</caption><tbody>
            <tr><th rowspan="2">Name</th><th colspan="2">Lengths</th></tr>
            <tr><th>a</th><th>b</th></tr>
            <tr><td>Small</td><td>3</td><td><ul><li>4</li><li>four</li></ul></td></tr></tbody></table>"#);
        assert_eq!(
            x.article.body,
            vec![
                Block::Para(vec![Inline::Styled(Style::Bold, vec![text("Sides")])]),
                Block::TableRef(0)
            ]
        );
        let t = &x.article.tables[0];
        assert_eq!((t.rows, t.cols, t.cells.len()), (3, 3, 7));
        assert_eq!((t.cells[0].rowspan, t.cells[1].colspan), (2, 2));
        assert_eq!(
            t.cells[6].content,
            vec![text("• 4"), Inline::Break, text("• four")]
        );
    }

    #[test]
    fn infobox_rows() {
        let x = run(r#"<table class="infobox vcard"><tbody>
            <tr><th colspan="2" class="infobox-above">Pythagoras</th></tr>
            <tr><td colspan="2" class="infobox-image"><span typeof="mw:File"><img src="./_assets_/x/bust.jpg"></span><div class="infobox-caption">A bust</div></td></tr>
            <tr><th class="infobox-label">Born</th><td class="infobox-data">c. 570 BC<br>Samos</td></tr>
            <tr><th class="infobox-header">Philosophy</th></tr>
            <tr><td colspan="2" class="infobox-full-data">Ancient philosophy</td></tr>
            <tr><th class="infobox-label">Students</th><td class="infobox-data"></td></tr>
            </tbody></table><p>Text.</p>"#);
        assert_eq!(x.article.body[0], Block::InfoboxRef);
        assert_eq!(
            x.article.infobox.unwrap().rows,
            vec![
                InfoboxRow::Header(vec![text("Pythagoras")]),
                InfoboxRow::Field {
                    label: vec![text("Born")],
                    value: vec![text("c. 570 BC"), Inline::Break, text("Samos")]
                },
                InfoboxRow::Header(vec![text("Philosophy")]),
                InfoboxRow::Full(vec![text("Ancient philosophy")]),
            ]
        );
        assert_eq!(x.stats.images_skipped, 1);
    }

    #[test]
    fn chrome_is_dropped_and_hatnotes_kept() {
        let x = run(
            r#"<div class="hatnote navigation-not-searchable">For the album, see <a rel="mw:WikiLink" href="Geometry">Geometry</a>.</div>
            <table class="sidebar nomobile" role="navigation"><tr><td>Series</td></tr></table>
            <style>.x{}</style><div class="navbox" role="navigation">Nav</div>
            <figure typeof="mw:File/Thumb"><a><img src="./_assets_/x/y.jpg"></a><figcaption>Caption</figcaption></figure>
            <p>Body<span style="display:none">hidden</span>.</p>
            <meta property="mw:PageProp/disambiguation">"#,
        );
        assert_eq!(
            x.article.body,
            vec![
                Block::Para(vec![Inline::Styled(
                    Style::Italic,
                    vec![
                        text("For the album, see "),
                        Inline::Link {
                            article: 5,
                            section: 0,
                            content: vec![text("Geometry")]
                        },
                        text(".")
                    ]
                )]),
                Block::Para(vec![text("Body.")]),
            ]
        );
        assert_eq!(x.flags, article_flags::DISAMBIGUATION);
        assert_eq!(x.stats.images_skipped, 1);
    }

    #[test]
    fn same_style_runs_merge_and_whitespace_collapses() {
        let x = run("<p><i>d</i><i>s</i>  <b> a </b> b<br> <br>c </p>");
        assert_eq!(
            x.article.body,
            vec![Block::Para(vec![
                Inline::Styled(Style::Italic, vec![text("ds")]),
                text(" "),
                Inline::Styled(Style::Bold, vec![text("a ")]),
                text("b"),
                Inline::Break,
                text("c"),
            ])]
        );
    }
}

#[cfg(test)]
mod more_tests {
    use super::*;

    struct Nothing;

    impl Resolver for Nothing {
        fn article(&self, _: &str) -> Option<u32> {
            None
        }
    }

    fn run(body: &str) -> Extracted {
        let html =
            format!("<html><body><div class=\"mw-parser-output\">{body}</div></body></html>");
        extract(&html, "T", "T", 0, &Nothing)
    }

    fn text(s: &str) -> Inline {
        Inline::Text(s.to_string())
    }

    #[test]
    fn chemical_formulas_keep_stacked_scripts_inline() {
        let x = run(
            r#"<p>C<span class="nowrap"><span style="display:inline-block"><sup style="x"></sup><br><sub style="x">2</sub></span></span>H<sub>5</sub></p>"#,
        );
        assert_eq!(
            x.article.body,
            vec![Block::Para(vec![
                text("C"),
                Inline::Styled(Style::Subscript, vec![text("2")]),
                text("H"),
                Inline::Styled(Style::Subscript, vec![text("5")]),
            ])]
        );
    }

    #[test]
    fn infobox_labels_lists_and_empty_headers() {
        let x = run(r#"<table class="infobox biota"><tbody>
            <tr><th colspan="2">Red fox</th></tr>
            <tr><th colspan="2">Conservation status</th></tr>
            <tr><td colspan="2"><span typeof="mw:File"><img src="./_assets_/x/status.png"></span></td></tr>
            <tr><th colspan="2">Scientific classification</th></tr>
            <tr><td>Kingdom:</td><td>Animalia</td></tr>
            <tr><td>CAS Number</td><td><ul><li><a rel="mw:ExtLink" class="external text" href="https://x.org/64-17-5">64-17-5</a></li></ul></td></tr>
            <tr><th colspan="2">Synonyms</th></tr>
            </tbody></table>
            <table class="clade"><tr><td>tree</td></tr></table>
            <table class="wikitable"><caption><b>Bold</b> caption</caption><tr><td>a</td></tr></table>"#);
        assert_eq!(
            x.article.infobox.unwrap().rows,
            vec![
                InfoboxRow::Header(vec![text("Red fox")]),
                InfoboxRow::Header(vec![text("Scientific classification")]),
                InfoboxRow::Field {
                    label: vec![text("Kingdom")],
                    value: vec![text("Animalia")]
                },
                InfoboxRow::Field {
                    label: vec![text("CAS Number")],
                    value: vec![text("64-17-5")]
                },
            ]
        );
        assert_eq!(x.stats.tables_dropped, 1);
        assert_eq!(
            x.article.body[1],
            Block::Para(vec![Inline::Styled(
                Style::Bold,
                vec![text("Bold caption")]
            )])
        );
    }
}

#[cfg(test)]
mod image_tests {
    use super::*;

    /// Resolves articles "A" (1) and images under `_assets_/` by name.
    struct WithImages;

    impl Resolver for WithImages {
        fn article(&self, path: &str) -> Option<u32> {
            (path == "A").then_some(1)
        }

        fn image(&self, path: &str) -> Option<u32> {
            match path {
                "_assets_/h/Photo.jpg" => Some(10),
                "_assets_/h/Diagram.svg.png" => Some(11),
                "_assets_/h/Flag.png" => Some(12),
                _ => None,
            }
        }
    }

    fn run(body: &str) -> Extracted {
        let html =
            format!("<html><body><div class=\"mw-parser-output\">{body}</div></body></html>");
        extract(&html, "T", "T", 0, &WithImages)
    }

    fn text(s: &str) -> Inline {
        Inline::Text(s.to_string())
    }

    #[test]
    fn figures_galleries_infobox_images_and_icons() {
        let x = run(r#"<table class="infobox"><tbody>
              <tr><td class="infobox-image"><span typeof="mw:File"><a><img src="./_assets_/h/Photo.jpg" width="220" height="150"></a></span><div class="infobox-caption">A bust</div></td></tr>
              <tr><th>Flag</th><td><span typeof="mw:File"><img src="./_assets_/h/Flag.png" width="23" height="15"></span> Greece</td></tr>
            </tbody></table>
            <figure typeof="mw:File/Thumb"><a><img src="./_assets_/h/Diagram.svg.png" data-file-type="drawing" width="300" height="200"></a><figcaption>The <a rel="mw:WikiLink" href="A">proof</a></figcaption></figure>
            <figure typeof="mw:File/Thumb"><a><img src="./_assets_/h/Missing.jpg"></a><figcaption>Gone</figcaption></figure>
            <ul class="gallery mw-gallery-traditional"><li class="gallerybox"><div class="thumb"><span typeof="mw:File"><img src="./_assets_/h/Photo.jpg"></span></div><div class="gallerytext">Again</div></li></ul>
            <p>See <span typeof="mw:File"><img src="./_assets_/h/Flag.png" width="20" height="14"></span> here.</p>"#);
        // The photo is used twice but listed once.
        assert_eq!(
            x.images,
            vec![
                ImageUse {
                    key: 10,
                    kind: Kind::Photo
                },
                ImageUse {
                    key: 12,
                    kind: Kind::Icon
                },
                ImageUse {
                    key: 11,
                    kind: Kind::Drawing
                },
            ]
        );
        assert_eq!(
            x.article.infobox.unwrap().rows,
            vec![
                InfoboxRow::Image {
                    image: 0,
                    caption: vec![text("A bust")]
                },
                InfoboxRow::Field {
                    label: vec![text("Flag")],
                    value: vec![Inline::Icon(1), text(" Greece")]
                },
            ]
        );
        assert_eq!(
            x.article.body,
            vec![
                Block::InfoboxRef,
                Block::Figure {
                    image: 2,
                    caption: vec![
                        text("The "),
                        Inline::Link {
                            article: 1,
                            section: 0,
                            content: vec![text("proof")]
                        }
                    ]
                },
                Block::Gallery(vec![(0, vec![text("Again")])]),
                Block::Para(vec![text("See "), Inline::Icon(1), text(" here.")]),
            ]
        );
        assert_eq!((x.stats.images, x.stats.images_skipped), (5, 1));
    }

    #[test]
    fn image_ids_can_be_remapped() {
        let mut x = run(
            r#"<figure typeof="mw:File/Thumb"><img src="./_assets_/h/Photo.jpg"></figure><p><img src="./_assets_/h/Flag.png" width="20" height="10"></p>"#,
        );
        for_each_image_mut(&mut x.article, &mut |i| *i += 100);
        assert_eq!(
            x.article.body,
            vec![
                Block::Figure {
                    image: 100,
                    caption: vec![]
                },
                Block::Para(vec![Inline::Icon(101)])
            ]
        );
    }
}

#[cfg(test)]
mod formula_tests {
    use super::*;
    use xwa_format::content::MathItem;

    /// Draws every formula except `\ce{...}` as one glyph.
    struct Draws;

    impl Resolver for Draws {
        fn article(&self, _: &str) -> Option<u32> {
            None
        }

        fn formula(&self, tex: &str, display: bool) -> Option<Formula> {
            (!tex.contains("\\ce")).then(|| Formula {
                display,
                width: 1000,
                ascent: 700,
                descent: 200,
                items: vec![MathItem::Glyph {
                    glyph: 7,
                    scale: 0,
                    dx: 0,
                    dy: 0,
                }],
                fallback_image: None,
            })
        }
    }

    fn math(class: &str, tex: &str, mathml: &str) -> String {
        format!(
            r#"<span class="mwe-math-element {class}"><span class="mwe-math-mathml-a11y" style="display: none;"><math alttext="{tex}">{mathml}</math></span><img class="mwe-math-fallback-image-inline" alt="{tex}"></span>"#
        )
    }

    #[test]
    fn formulas_are_drawn_with_a_text_fallback() {
        let body = format!(
            "<p>Since {} and{}we get {}.</p>",
            math(
                "mwe-math-element-inline",
                r"{\displaystyle a}",
                "<mi>a</mi>"
            ),
            math("mwe-math-element-block", r"{\displaystyle b}", "<mi>b</mi>"),
            math(
                "mwe-math-element-inline",
                r"{\displaystyle {\ce {H2O}}}",
                "<msub><mi>H</mi><mn>2</mn></msub><mi>O</mi>"
            ),
        );
        let html =
            format!("<html><body><div class=\"mw-parser-output\">{body}</div></body></html>");
        let x = extract(&html, "T", "T", 0, &Draws);
        let drawn = |display| Formula {
            display,
            width: 1000,
            ascent: 700,
            descent: 200,
            items: vec![MathItem::Glyph {
                glyph: 7,
                scale: 0,
                dx: 0,
                dy: 0,
            }],
            fallback_image: None,
        };
        assert_eq!(
            x.article.body,
            vec![
                Block::Para(vec![
                    Inline::Text("Since ".into()),
                    Inline::Math(drawn(false)),
                    Inline::Text(" and".into())
                ]),
                Block::MathBlock(drawn(true)),
                Block::Para(vec![
                    Inline::Text("we get ".into()),
                    Inline::Styled(Style::Italic, vec![Inline::Text("H".into())]),
                    Inline::Styled(Style::Subscript, vec![Inline::Text("2".into())]),
                    Inline::Styled(Style::Italic, vec![Inline::Text("O".into())]),
                    Inline::Text(".".into()),
                ]),
            ]
        );
        assert_eq!((x.stats.formulas, x.stats.formulas_drawn), (3, 2));
    }
}
