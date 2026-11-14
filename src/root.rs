//! What the root element says about itself, read on the way through one
//! XML document: its name, its prefix and the namespace it is declared in.
//!
//! The walk is the estate's one XML reader, `codec::xml::Reader`: it holds
//! the bytes to a document's order — every element closed by its own end
//! tag, nothing but comments, processing instructions and a document type
//! declaration around the root — and says the byte where they leave it.
//! Every start tag's attributes are read, so a malformed one is refused
//! too. No tree, no entity read: a shape sections and names, a contract
//! validates. Until 2026-11-13 this file walked the bytes itself.

use codec::xml::{Event, Reader, Tag};
use message::Stop;

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
/// The bytes are read in the encoding their byte order mark or their XML
/// declaration says, UTF-8 where neither does (`codec::xml::decoded`); in
/// another encoding than UTF-8 a byte is counted in the text decoded.
pub fn document(bytes: &[u8]) -> Result<Root, Stop> {
    let text = codec::xml::decoded(bytes).map_err(|stop| (stop.reason, stop.at))?;
    let mut reader = Reader::document(&text);
    let mut root = None;
    while let Some(piece) = reader.read_tag().map_err(|stop| (stop.reason, stop.at))? {
        let Event::Start(tag) = piece.event else {
            continue;
        };
        let mut declared = None;
        let wanted = tag.name().split_once(':').map(|(prefix, _)| prefix);
        for written in tag.attributes() {
            let written = written.map_err(|stop| (stop.reason, stop.at))?;
            if root.is_none() && declares(written.name, wanted) {
                declared = Some(written.value);
            }
        }
        if root.is_none() {
            root = Some(Root::from_tag(&tag, declared));
        }
    }
    root.ok_or(("no root element", bytes.len()))
}

/// Whether the attribute `name` declares the namespace of `prefix`, the
/// default namespace's where there is none.
fn declares(name: &str, prefix: Option<&str>) -> bool {
    match prefix {
        Some(prefix) => name.strip_prefix("xmlns:") == Some(prefix),
        None => name == "xmlns",
    }
}

impl Root {
    fn from_tag(tag: &Tag<'_>, declared: Option<&str>) -> Self {
        let name = tag.name();
        let (prefix, local_name) = match name.split_once(':') {
            Some((prefix, local)) => (Some(prefix.to_string()), local.to_string()),
            None => (None, name.to_string()),
        };
        Self {
            offset: tag.at(),
            prefix,
            local_name,
            namespace: declared
                .filter(|value| !value.is_empty())
                .map(str::to_string),
        }
    }
}

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
        assert_eq!(document(b"<a><!-- x</a>"), Err(("unterminated comment", 3)));
        assert_eq!(
            document(b"<a/><![CDATA[x]]>"),
            Err(("content after the document", 4))
        );
        assert_eq!(document(b"<a>\xff</a>"), Err(("not UTF-8", 3)));
    }

    #[test]
    fn a_document_in_the_encoding_it_declares_or_marks_is_walked_as_any_other() {
        let latin = b"<?xml version=\"1.0\" encoding=\"ISO-8859-1\"?>\
<f:Faktura xmlns:f=\"urn:f\"><Kund>Caf\xe9 \xc5land</Kund></f:Faktura>";
        assert_eq!(
            document(latin).expect("ISO-8859-1").expanded(),
            "{urn:f}Faktura"
        );
        let text = "<?xml version=\"1.0\" encoding=\"UTF-16\"?><o:Order xmlns:o=\"urn:o\">\
<Note>\u{e9}</Note></o:Order>";
        let mut utf16 = vec![0xFF, 0xFE];
        utf16.extend(text.encode_utf16().flat_map(u16::to_le_bytes));
        assert_eq!(document(&utf16).expect("UTF-16").expanded(), "{urn:o}Order");
        let wrong = b"<?xml version=\"1.0\" encoding=\"UTF-8\"?><a>\xe9</a>";
        assert_eq!(document(wrong), Err(("not UTF-8", 41)));
    }
}
