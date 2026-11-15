//! A small element tree over quick-xml, enough for OPDS feeds and
//! Metalink files (a few hundred kilobytes at most).

use quick_xml::events::{BytesStart, Event};

use crate::Error;

#[derive(Debug, Default, Clone)]
pub struct Node {
    /// Local name, without any namespace prefix.
    pub name: String,
    pub attrs: Vec<(String, String)>,
    pub text: String,
    pub children: Vec<Node>,
}

impl Node {
    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }

    pub fn child(&self, name: &str) -> Option<&Node> {
        self.children.iter().find(|c| c.name == name)
    }

    pub fn children_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Node> {
        self.children.iter().filter(move |c| c.name == name)
    }

    /// Trimmed text of the named child, or "" if there is none.
    pub fn child_text(&self, name: &str) -> &str {
        self.child(name).map_or("", |c| c.text.trim())
    }
}

fn local(name: &str) -> String {
    name.rsplit_once(':').map_or(name, |(_, l)| l).to_string()
}

fn start(e: &BytesStart) -> Result<Node, Error> {
    let mut node = Node {
        name: local(e.name().as_ref()),
        ..Default::default()
    };
    for a in e.attributes() {
        let a = a.map_err(|err| Error::Format(format!("XML attribute: {err}")))?;
        let value = a
            .normalized_value(quick_xml::XmlVersion::Implicit1_0)
            .map_err(|err| Error::Format(format!("XML attribute: {err}")))?;
        node.attrs.push((local(a.key.as_ref()), value.into_owned()));
    }
    Ok(node)
}

/// Parses a document and returns its root element.
pub fn parse(xml: &str) -> Result<Node, Error> {
    let mut reader = quick_xml::Reader::from_str(xml);
    let bad = |err: quick_xml::Error| Error::Format(format!("XML: {err}"));
    let mut stack: Vec<Node> = vec![Node::default()];
    loop {
        match reader.read_event().map_err(bad)? {
            Event::Start(e) => stack.push(start(&e)?),
            Event::Empty(e) => {
                let node = start(&e)?;
                stack.last_mut().unwrap().children.push(node);
            }
            Event::End(_) => {
                let node = stack.pop().unwrap();
                match stack.last_mut() {
                    Some(parent) => parent.children.push(node),
                    None => return Err(Error::Format("XML: unbalanced end tag".into())),
                }
            }
            Event::Text(t) => stack.last_mut().unwrap().text.push_str(&t.into_inner()),
            Event::CData(t) => stack.last_mut().unwrap().text.push_str(&t.into_inner()),
            Event::GeneralRef(r) => {
                let resolved = match r.resolve_char_ref().map_err(bad)? {
                    Some(c) => c.to_string(),
                    None => match r.into_inner().as_ref() {
                        "amp" => "&".into(),
                        "lt" => "<".into(),
                        "gt" => ">".into(),
                        "quot" => "\"".into(),
                        "apos" => "'".into(),
                        other => format!("&{other};"),
                    },
                };
                stack.last_mut().unwrap().text.push_str(&resolved);
            }
            Event::Eof => break,
            _ => {}
        }
    }
    let root = stack.pop().unwrap();
    if !stack.is_empty() {
        return Err(Error::Format("XML: unclosed element".into()));
    }
    root.children
        .into_iter()
        .next()
        .ok_or_else(|| Error::Format("XML: no root element".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_names_attributes_text_and_entities() {
        let root = parse(r#"<?xml version="1.0"?><feed xmlns:dc="x"><dc:title a="1 &amp; 2">Fish &amp; chips &#233;</dc:title><e/></feed>"#)
            .unwrap();
        assert_eq!(root.name, "feed");
        let t = root.child("title").unwrap();
        assert_eq!(t.attr("a"), Some("1 & 2"));
        assert_eq!(t.text, "Fish & chips é");
        assert!(root.child("e").is_some());
        assert!(parse("<a><b></a>").is_err());
    }
}
