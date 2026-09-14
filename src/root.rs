//! The walk that says a byte sequence is one XML document — every element
//! closed by its own end tag, nothing but comments and processing
//! instructions around the root — and what it reads off the root element on
//! the way in: its name, its prefix and the namespace it is declared in.
//!
//! Not a parser. No tree, no entity expansion, no attribute beyond the
//! root's namespace declarations. A shape sections and names; a contract
//! validates. The cursor — peek, the rest, whitespace — is the Foundation's
//! [`Scan`] (ADR-0044); the grammar of a document is this file's.

use message::Stop;
use message::scan::Scan;

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
    let mut walk = Scan::new(bytes);
    if bytes.starts_with(b"\xef\xbb\xbf") {
        walk.at = 3;
    }
    let root = prolog(&mut walk)?;
    let mut open: Vec<&[u8]> = Vec::new();
    let (name, attributes) = start_tag(&mut walk, &mut open)?;
    let root = Root::from_tag(root, name, &attributes);
    while !open.is_empty() {
        content(&mut walk, &mut open)?;
    }
    trailer(&mut walk)?;
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

fn is_name_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || byte == b'_' || byte == b':' || byte >= 0x80
}

fn is_name(byte: u8) -> bool {
    is_name_start(byte) || byte.is_ascii_digit() || byte == b'-' || byte == b'.'
}

/// Skip to just past `close`, or stop with `reason` at the markup start.
fn past(walk: &mut Scan<'_>, close: &[u8], reason: &'static str) -> Result<(), Stop> {
    let start = walk.at;
    match walk.rest().windows(close.len()).position(|w| w == close) {
        Some(found) => {
            walk.at += found + close.len();
            Ok(())
        }
        None => Err((reason, start)),
    }
}

/// The comment, processing instruction or CDATA section under the cursor,
/// if one begins there.
fn misc(walk: &mut Scan<'_>) -> Result<bool, Stop> {
    let rest = walk.rest();
    if rest.starts_with(b"<!--") {
        past(walk, b"-->", "unterminated comment")?;
    } else if rest.starts_with(b"<?") {
        past(walk, b"?>", "unterminated processing instruction")?;
    } else if rest.starts_with(b"<![CDATA[") {
        past(walk, b"]]>", "unterminated CDATA section")?;
    } else {
        return Ok(false);
    }
    Ok(true)
}

/// Everything before the root; the offset of the root's `<`.
fn prolog(walk: &mut Scan<'_>) -> Result<usize, Stop> {
    loop {
        walk.whitespace();
        if misc(walk)? {
            continue;
        }
        if walk.rest().starts_with(b"<!DOCTYPE") {
            doctype(walk)?;
            continue;
        }
        return match walk.rest() {
            [b'<', next, ..] if is_name_start(*next) => Ok(walk.at),
            [] => Err(("no root element", walk.at)),
            _ => Err(("expected the root element", walk.at)),
        };
    }
}

fn doctype(walk: &mut Scan<'_>) -> Result<(), Stop> {
    let start = walk.at;
    let mut subset = 0usize;
    while let Some(byte) = walk.peek() {
        walk.at += 1;
        match byte {
            b'[' => subset += 1,
            b']' => subset = subset.saturating_sub(1),
            b'>' if subset == 0 => return Ok(()),
            _ => {}
        }
    }
    Err(("unterminated document type declaration", start))
}

/// A start tag under the cursor: its name and attributes, the name pushed
/// on `open` unless the tag closes itself.
fn start_tag<'a>(
    walk: &mut Scan<'a>,
    open: &mut Vec<&'a [u8]>,
) -> Result<(&'a [u8], Attributes<'a>), Stop> {
    walk.at += 1;
    let name = name(walk, "expected an element name")?;
    let mut attributes = Vec::new();
    loop {
        walk.whitespace();
        match walk.peek() {
            Some(b'>') => {
                walk.at += 1;
                open.push(name);
                return Ok((name, attributes));
            }
            Some(b'/') if walk.bytes.get(walk.at + 1) == Some(&b'>') => {
                walk.at += 2;
                return Ok((name, attributes));
            }
            Some(byte) if is_name_start(byte) => {
                attributes.push(attribute(walk)?);
            }
            _ => return Err(("expected an attribute or the end of the tag", walk.at)),
        }
    }
}

fn name<'a>(walk: &mut Scan<'a>, reason: &'static str) -> Result<&'a [u8], Stop> {
    let start = walk.at;
    if !walk.peek().is_some_and(is_name_start) {
        return Err((reason, start));
    }
    while walk.peek().is_some_and(is_name) {
        walk.at += 1;
    }
    Ok(&walk.bytes[start..walk.at])
}

fn attribute<'a>(walk: &mut Scan<'a>) -> Result<(&'a [u8], &'a [u8]), Stop> {
    let key = name(walk, "expected an attribute name")?;
    walk.whitespace();
    if walk.peek() != Some(b'=') {
        return Err(("expected = after the attribute name", walk.at));
    }
    walk.at += 1;
    walk.whitespace();
    let Some(quote @ (b'"' | b'\'')) = walk.peek() else {
        return Err(("expected a quoted attribute value", walk.at));
    };
    let start = walk.at + 1;
    let Some(length) = walk.bytes[start..].iter().position(|b| *b == quote) else {
        return Err(("unterminated attribute value", walk.at));
    };
    walk.at = start + length + 1;
    Ok((key, &walk.bytes[start..start + length]))
}

/// One piece of content inside the root: text, markup, a start tag or
/// the end tag that closes the innermost open element.
fn content<'a>(walk: &mut Scan<'a>, open: &mut Vec<&'a [u8]>) -> Result<(), Stop> {
    let Some(found) = walk.rest().iter().position(|b| *b == b'<') else {
        return Err(("the element is never closed", walk.bytes.len()));
    };
    walk.at += found;
    if misc(walk)? {
        return Ok(());
    }
    match walk.rest() {
        [b'<', b'/', ..] => end_tag(walk, open),
        [b'<', next, ..] if is_name_start(*next) => start_tag(walk, open).map(|_| ()),
        _ => Err(("expected a tag", walk.at)),
    }
}

fn end_tag<'a>(walk: &mut Scan<'a>, open: &mut Vec<&'a [u8]>) -> Result<(), Stop> {
    let start = walk.at;
    walk.at += 2;
    let name = name(walk, "expected an element name")?;
    walk.whitespace();
    if walk.peek() != Some(b'>') {
        return Err(("expected > to end the tag", walk.at));
    }
    walk.at += 1;
    if open.pop() != Some(name) {
        return Err(("the end tag does not close the open element", start));
    }
    Ok(())
}

/// Everything after the root: whitespace, comments and instructions.
fn trailer(walk: &mut Scan<'_>) -> Result<(), Stop> {
    loop {
        walk.whitespace();
        if walk.at >= walk.bytes.len() {
            return Ok(());
        }
        if walk.rest().starts_with(b"<![CDATA[") || !misc(walk)? {
            return Err(("content after the document", walk.at));
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
