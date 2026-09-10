//! The walk that says a byte sequence is one XML document — every element
//! closed by its own end tag, nothing but comments and processing
//! instructions around the root — and what it reads off the root element on
//! the way in: its name, its prefix and the namespace it is declared in.
//!
//! Not a parser. No tree, no entity expansion, no attribute beyond the
//! root's namespace declarations. A shape sections and names; a contract
//! validates.

/// Where the walk stopped and why.
pub type Stop = (&'static str, usize);

/// What the root element says about itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Root {
    /// The byte at which the root's start tag opens.
    pub offset: usize,
    /// The prefix before the colon, when the name has one.
    pub prefix: Option<String>,
    /// The name after the prefix, or the whole name.
    pub local_name: String,
    /// The namespace the root's own `xmlns` or `xmlns:prefix` declares.
    pub namespace: Option<String>,
}

impl Root {
    /// `{namespace}localName`, or the bare local name without a namespace.
    #[must_use]
    pub fn expanded(&self) -> String {
        match &self.namespace {
            Some(namespace) => format!("{{{namespace}}}{}", self.local_name),
            None => self.local_name.clone(),
        }
    }
}

/// Walk `bytes` as one document; the root when it is one.
///
/// # Errors
/// The reason and the byte at which the bytes stopped being a document.
pub fn document(bytes: &[u8]) -> Result<Root, Stop> {
    let mut walk = Walk { bytes, at: 0 };
    if bytes.starts_with(b"\xef\xbb\xbf") {
        walk.at = 3;
    }
    let root = walk.prolog()?;
    let mut open: Vec<&[u8]> = Vec::new();
    let (name, attributes) = walk.start_tag(&mut open)?;
    let root = Root::from_tag(root, name, &attributes);
    while !open.is_empty() {
        walk.content(&mut open)?;
    }
    walk.trailer()?;
    Ok(root)
}

impl Root {
    fn from_tag(offset: usize, name: &[u8], attributes: &[(&[u8], &[u8])]) -> Self {
        let name = String::from_utf8_lossy(name);
        let (prefix, local_name) = match name.split_once(':') {
            Some((prefix, local)) => (Some(prefix.to_string()), local.to_string()),
            None => (None, name.to_string()),
        };
        let declaration = prefix
            .as_ref()
            .map_or_else(|| b"xmlns".to_vec(), |p| format!("xmlns:{p}").into_bytes());
        let namespace = attributes
            .iter()
            .find(|(key, _)| *key == declaration.as_slice())
            .map(|(_, value)| String::from_utf8_lossy(value).into_owned())
            .filter(|value| !value.is_empty());
        Self {
            offset,
            prefix,
            local_name,
            namespace,
        }
    }
}

struct Walk<'a> {
    bytes: &'a [u8],
    at: usize,
}

fn is_name_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || byte == b'_' || byte == b':' || byte >= 0x80
}

fn is_name(byte: u8) -> bool {
    is_name_start(byte) || byte.is_ascii_digit() || byte == b'-' || byte == b'.'
}

impl<'a> Walk<'a> {
    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.at).copied()
    }

    fn rest(&self) -> &'a [u8] {
        &self.bytes[self.at.min(self.bytes.len())..]
    }

    fn whitespace(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\r' | b'\n')) {
            self.at += 1;
        }
    }

    /// Skip to just past `close`, or stop with `reason` at the markup start.
    fn past(&mut self, close: &[u8], reason: &'static str) -> Result<(), Stop> {
        let start = self.at;
        match self.rest().windows(close.len()).position(|w| w == close) {
            Some(found) => {
                self.at += found + close.len();
                Ok(())
            }
            None => Err((reason, start)),
        }
    }

    /// The comment, processing instruction or CDATA section at `at`, if
    /// one begins there.
    fn misc(&mut self) -> Result<bool, Stop> {
        let rest = self.rest();
        if rest.starts_with(b"<!--") {
            self.past(b"-->", "unterminated comment")?;
        } else if rest.starts_with(b"<?") {
            self.past(b"?>", "unterminated processing instruction")?;
        } else if rest.starts_with(b"<![CDATA[") {
            self.past(b"]]>", "unterminated CDATA section")?;
        } else {
            return Ok(false);
        }
        Ok(true)
    }

    /// Everything before the root; the offset of the root's `<`.
    fn prolog(&mut self) -> Result<usize, Stop> {
        loop {
            self.whitespace();
            if self.misc()? {
                continue;
            }
            if self.rest().starts_with(b"<!DOCTYPE") {
                self.doctype()?;
                continue;
            }
            return match self.rest() {
                [b'<', next, ..] if is_name_start(*next) => Ok(self.at),
                [] => Err(("no root element", self.at)),
                _ => Err(("expected the root element", self.at)),
            };
        }
    }

    fn doctype(&mut self) -> Result<(), Stop> {
        let start = self.at;
        let mut subset = 0usize;
        while let Some(byte) = self.peek() {
            self.at += 1;
            match byte {
                b'[' => subset += 1,
                b']' => subset = subset.saturating_sub(1),
                b'>' if subset == 0 => return Ok(()),
                _ => {}
            }
        }
        Err(("unterminated document type declaration", start))
    }

    /// A start tag at `at`: its name and attributes, the name pushed on
    /// `open` unless the tag closes itself.
    fn start_tag(&mut self, open: &mut Vec<&'a [u8]>) -> Result<(&'a [u8], Attributes<'a>), Stop> {
        self.at += 1;
        let name = self.name("expected an element name")?;
        let mut attributes = Vec::new();
        loop {
            self.whitespace();
            match self.peek() {
                Some(b'>') => {
                    self.at += 1;
                    open.push(name);
                    return Ok((name, attributes));
                }
                Some(b'/') if self.bytes.get(self.at + 1) == Some(&b'>') => {
                    self.at += 2;
                    return Ok((name, attributes));
                }
                Some(byte) if is_name_start(byte) => {
                    attributes.push(self.attribute()?);
                }
                _ => return Err(("expected an attribute or the end of the tag", self.at)),
            }
        }
    }

    fn name(&mut self, reason: &'static str) -> Result<&'a [u8], Stop> {
        let start = self.at;
        if !self.peek().is_some_and(is_name_start) {
            return Err((reason, start));
        }
        while self.peek().is_some_and(is_name) {
            self.at += 1;
        }
        Ok(&self.bytes[start..self.at])
    }

    fn attribute(&mut self) -> Result<(&'a [u8], &'a [u8]), Stop> {
        let key = self.name("expected an attribute name")?;
        self.whitespace();
        if self.peek() != Some(b'=') {
            return Err(("expected = after the attribute name", self.at));
        }
        self.at += 1;
        self.whitespace();
        let Some(quote @ (b'"' | b'\'')) = self.peek() else {
            return Err(("expected a quoted attribute value", self.at));
        };
        let start = self.at + 1;
        let Some(length) = self.bytes[start..].iter().position(|b| *b == quote) else {
            return Err(("unterminated attribute value", self.at));
        };
        self.at = start + length + 1;
        Ok((key, &self.bytes[start..start + length]))
    }

    /// One piece of content inside the root: text, markup, a start tag or
    /// the end tag that closes the innermost open element.
    fn content(&mut self, open: &mut Vec<&'a [u8]>) -> Result<(), Stop> {
        let Some(found) = self.rest().iter().position(|b| *b == b'<') else {
            return Err(("the element is never closed", self.bytes.len()));
        };
        self.at += found;
        if self.misc()? {
            return Ok(());
        }
        match self.rest() {
            [b'<', b'/', ..] => self.end_tag(open),
            [b'<', next, ..] if is_name_start(*next) => self.start_tag(open).map(|_| ()),
            _ => Err(("expected a tag", self.at)),
        }
    }

    fn end_tag(&mut self, open: &mut Vec<&'a [u8]>) -> Result<(), Stop> {
        let start = self.at;
        self.at += 2;
        let name = self.name("expected an element name")?;
        self.whitespace();
        if self.peek() != Some(b'>') {
            return Err(("expected > to end the tag", self.at));
        }
        self.at += 1;
        if open.pop() != Some(name) {
            return Err(("the end tag does not close the open element", start));
        }
        Ok(())
    }

    /// Everything after the root: whitespace, comments and instructions.
    fn trailer(&mut self) -> Result<(), Stop> {
        loop {
            self.whitespace();
            if self.at >= self.bytes.len() {
                return Ok(());
            }
            if self.rest().starts_with(b"<![CDATA[") || !self.misc()? {
                return Err(("content after the document", self.at));
            }
        }
    }
}

type Attributes<'a> = Vec<(&'a [u8], &'a [u8])>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_document_walks_to_its_root_name_prefix_and_namespace() {
        let text = b"\xef\xbb\xbf<?xml version=\"1.0\"?>\n<!-- c -->\
<!DOCTYPE a [<!ENTITY x \"y\">]>\
<p:a xmlns:p='urn:p' xmlns=\"urn:d\" k=\"v>\"><b><![CDATA[<x>]]></b><c/>\
t &amp; u<?pi x?></p:a>\n<!--z-->";
        let root = document(text).expect("well-formed");
        assert_eq!(root.offset, 65);
        assert_eq!(root.prefix.as_deref(), Some("p"));
        assert_eq!(root.local_name, "a");
        assert_eq!(root.namespace.as_deref(), Some("urn:p"));
        assert_eq!(root.expanded(), "{urn:p}a");

        let plain = document(b"<a xmlns=\"urn:d\"/>").expect("well-formed");
        assert_eq!(plain.expanded(), "{urn:d}a");
        let bare = document(b"<a x='1'></a>").expect("well-formed");
        assert_eq!(bare.expanded(), "a");
        assert_eq!(bare.namespace, None);
    }

    #[test]
    fn the_first_byte_that_cannot_continue_is_the_offset() {
        assert_eq!(
            document(b"<a><b></a>"),
            Err(("the end tag does not close the open element", 6))
        );
        assert_eq!(document(b"<a><b>"), Err(("the element is never closed", 6)));
        assert_eq!(
            document(b"<a/><b/>"),
            Err(("content after the document", 4))
        );
        assert_eq!(document(b"<a/>x"), Err(("content after the document", 4)));
        assert_eq!(document(b"  "), Err(("no root element", 2)));
        assert_eq!(document(b"text"), Err(("expected the root element", 0)));
        assert_eq!(
            document(b"<a x=1/>"),
            Err(("expected a quoted attribute value", 5))
        );
        assert_eq!(
            document(b"<a x=\"1/>"),
            Err(("unterminated attribute value", 5))
        );
        assert_eq!(document(b"<a><!-- x</a>"), Err(("unterminated comment", 3)));
        assert_eq!(document(b"<a>< b/></a>"), Err(("expected a tag", 3)));
        assert_eq!(
            document(b"<a/><![CDATA[x]]>"),
            Err(("content after the document", 4))
        );
    }
}
