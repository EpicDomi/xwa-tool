//! Converted articles kept in a temporary file during a single read of the
//! ZIM, whose links point at ZIM entries until every page has been
//! classified, then numbered and written in order.

use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::Path;

use xwa_format::content::{Article, Block, InfoboxRow, Inline, Place};

/// Articles in the order they were added, compressed.
pub struct ArticleSpool {
    out: zstd::stream::write::Encoder<'static, BufWriter<File>>,
}

/// An article read back: the ZIM entry it came from, and its content and
/// flags, or `None` for a page that could not be read.
pub type Spooled = (u32, Option<(u16, Article)>);

impl ArticleSpool {
    /// A new spool in a temporary file in `dir`, deleted when dropped.
    pub fn new(dir: &Path) -> std::io::Result<Self> {
        let file = tempfile::tempfile_in(dir)?;
        let out = zstd::stream::write::Encoder::new(BufWriter::with_capacity(1 << 20, file), 1)?;
        Ok(Self { out })
    }

    /// Adds the article from ZIM entry `entry`: its encoded stream and
    /// flags, or `None` when its page could not be read.
    pub fn push(&mut self, entry: u32, article: Option<(u16, &[u8])>) -> std::io::Result<()> {
        self.out.write_all(&entry.to_le_bytes())?;
        match article {
            Some((flags, stream)) => {
                self.out.write_all(&[1])?;
                self.out.write_all(&flags.to_le_bytes())?;
                self.out.write_all(&(stream.len() as u32).to_le_bytes())?;
                self.out.write_all(stream)?;
            }
            None => {
                self.out.write_all(&[0])?;
            }
        }
        Ok(())
    }

    /// Finishes writing and reads the articles back, in order. Also gives
    /// the size of the compressed file.
    pub fn read_back(self) -> std::io::Result<(SpoolReader, u64)> {
        let mut file = self
            .out
            .finish()?
            .into_inner()
            .map_err(|e| e.into_error())?;
        let size = file.stream_position()?;
        file.seek(SeekFrom::Start(0))?;
        let input = zstd::stream::read::Decoder::new(BufReader::with_capacity(1 << 20, file))?;
        Ok((SpoolReader { input }, size))
    }
}

pub struct SpoolReader {
    input: zstd::stream::read::Decoder<'static, BufReader<BufReader<File>>>,
}

impl SpoolReader {
    /// The next article, or `None` at the end.
    pub fn next_article(&mut self) -> crate::Result<Option<Spooled>> {
        let mut entry = [0u8; 4];
        match self.input.read_exact(&mut entry) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
            Err(e) => return Err(e.into()),
        }
        let entry = u32::from_le_bytes(entry);
        let mut kind = [0u8; 1];
        self.input.read_exact(&mut kind)?;
        if kind[0] == 0 {
            return Ok(Some((entry, None)));
        }
        let mut head = [0u8; 6];
        self.input.read_exact(&mut head)?;
        let flags = u16::from_le_bytes([head[0], head[1]]);
        let len = u32::from_le_bytes(head[2..6].try_into().unwrap()) as usize;
        let mut stream = vec![0u8; len];
        self.input.read_exact(&mut stream)?;
        // The stream starts with its own length, as the archive stores it.
        let start = stream
            .iter()
            .position(|b| b & 0x80 == 0)
            .map_or(0, |i| i + 1);
        let article = xwa_format::content::decode_article(&stream[start..])?;
        Ok(Some((entry, Some((flags, article)))))
    }
}

/// Calls `f` on every link's target and place; where it returns false the
/// link is taken away and its content left in its place, as extraction
/// does for links that lead nowhere.
pub fn relink(a: &mut Article, f: &mut impl FnMut(&mut u32, &mut Place) -> bool) {
    fn inlines(list: &mut Vec<Inline>, f: &mut impl FnMut(&mut u32, &mut Place) -> bool) {
        let mut unlinked = false;
        let mut i = 0;
        while i < list.len() {
            match &mut list[i] {
                Inline::Link {
                    article,
                    place,
                    content,
                } => {
                    inlines(content, f);
                    if !f(article, place) {
                        let content = std::mem::take(content);
                        let n = content.len();
                        list.splice(i..=i, content);
                        unlinked = true;
                        i += n;
                        continue;
                    }
                }
                Inline::Styled(_, c) => inlines(c, f),
                Inline::Text(_)
                | Inline::Math(_)
                | Inline::Icon { .. }
                | Inline::Break
                | Inline::NoteRef(_) => {}
            }
            i += 1;
        }
        if unlinked {
            // As extraction leaves it: neighbouring text joined.
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
    }
    fn blocks(list: &mut [Block], f: &mut impl FnMut(&mut u32, &mut Place) -> bool) {
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
        inlines(&mut t.caption, f);
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

    fn link(to: u32, text: &str) -> Inline {
        Inline::Link {
            article: to,
            place: Place::Section(0),
            content: vec![Inline::Text(text.into())],
        }
    }

    #[test]
    fn articles_come_back_in_order() {
        let a = Article {
            title: "Ångström".into(),
            body: vec![Block::Para(vec![
                Inline::Text("x".repeat(300)),
                link(7, "y"),
            ])],
            ..Default::default()
        };
        let dir = tempfile::tempdir().unwrap();
        let mut spool = ArticleSpool::new(dir.path()).unwrap();
        let stream = xwa_format::content::encode_article(&a);
        spool.push(3, Some((5, &stream))).unwrap();
        spool.push(4, None).unwrap();
        spool.push(9, Some((0, &stream))).unwrap();
        let (mut back, size) = spool.read_back().unwrap();
        assert!(size > 0);
        let (e, x) = back.next_article().unwrap().unwrap();
        assert_eq!((e, x.map(|(f, b)| (f, b == a))), (3, Some((5, true))));
        assert_eq!(back.next_article().unwrap().unwrap(), (4, None));
        assert_eq!(back.next_article().unwrap().unwrap().0, 9);
        assert!(back.next_article().unwrap().is_none());
    }

    #[test]
    fn unlinking_leaves_the_text_joined() {
        let mut a = Article {
            body: vec![Block::Para(vec![
                Inline::Text("a ".into()),
                link(1, "gone"),
                Inline::Text(" b ".into()),
                link(2, "kept"),
            ])],
            ..Default::default()
        };
        relink(&mut a, &mut |to, _| {
            *to += 10;
            *to == 12
        });
        assert_eq!(
            a.body,
            vec![Block::Para(vec![
                Inline::Text("a gone b ".into()),
                link(12, "kept")
            ])]
        );
    }
}
