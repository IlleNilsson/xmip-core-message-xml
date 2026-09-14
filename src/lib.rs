#![forbid(unsafe_code)]

//! XML: one document, one part. The shape walks the document far enough to
//! know it is one — every element closed by its own end tag, nothing but
//! comments and processing instructions around the root — and names the
//! type the document announces: the root element's expanded name,
//! `{namespace}localName` when the root declares its namespace and the bare
//! name when it does not (ADR-0047).
//!
//! That walk is not a parse: no tree, no entity expansion, no attribute read
//! beyond the root's namespace declarations, because a shape sections and
//! names and a contract validates. What cannot be sectioned is refused with
//! the byte where the walk stopped. A vocabulary over XML — `ubl` is one —
//! reads its document through [`root::document`] and adds only what the
//! vocabulary says.

pub mod root;

use message::{Part, Shape, ShapeError, Shaped};
use stream::Stream;

/// The XML shape.
#[derive(Clone, Copy, Debug, Default)]
pub struct Xml;

/// The media types an XML document claims. Any `+xml` suffix is XML too,
/// but a shape claims by list and `choose` believes the list, so the
/// suffixed types the estate meets are named.
const MEDIA_TYPES: &[&str] = &[
    "application/xml",
    "text/xml",
    "application/soap+xml",
    "application/xhtml+xml",
    "application/atom+xml",
    "application/rss+xml",
    "application/xslt+xml",
    "application/xop+xml",
    "image/svg+xml",
];

/// Whether the bytes open, after a BOM and whitespace, with a declaration,
/// a comment, a document type or an element.
fn opens_with_markup(bytes: &[u8]) -> bool {
    let bytes = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes);
    let mut rest = bytes;
    while let [b' ' | b'\t' | b'\r' | b'\n', tail @ ..] = rest {
        rest = tail;
    }
    matches!(rest, [b'<', next, ..] if *next == b'?' || *next == b'!'
        || next.is_ascii_alphabetic() || *next == b'_')
}

impl Shape for Xml {
    fn technology(&self) -> &'static str {
        "xml"
    }

    fn media_types(&self) -> &'static [&'static str] {
        MEDIA_TYPES
    }

    fn recognises(&self, bytes: &[u8]) -> bool {
        opens_with_markup(bytes)
    }

    fn shape(&self, stream: &Stream) -> Result<Shaped, ShapeError> {
        let root =
            root::document(stream.bytes()).map_err(|stop| ShapeError::refused("xml", stop))?;
        let media = stream
            .media_type()
            .map_or_else(|| "application/xml".to_string(), str::to_string);
        Ok(Shaped {
            parts: vec![Part::new(None, stream.bytes(), Some(media))],
            message_type: Some(root.expanded()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use xcore::StreamId;

    fn stream(bytes: &[u8], media: Option<&str>) -> Stream {
        Stream::new(StreamId::new(1), bytes.to_vec(), media.map(str::to_string))
    }

    #[test]
    fn a_document_is_one_part_and_the_root_expanded_name_is_the_announced_type() {
        let text = b"<?xml version=\"1.0\"?>\n<o:Order xmlns:o=\"urn:x:order\"><Line/></o:Order>";
        let shaped = Xml.shape(&stream(text, None)).expect("well-formed");
        assert_eq!(shaped.parts.len(), 1);
        assert_eq!(shaped.parts[0].bytes, text);
        assert_eq!(
            shaped.parts[0].media_type.as_deref(),
            Some("application/xml")
        );
        assert_eq!(shaped.message_type.as_deref(), Some("{urn:x:order}Order"));

        let bare = Xml
            .shape(&stream(b"<Order/>", Some("text/xml; charset=utf-8")))
            .expect("well-formed");
        assert_eq!(bare.message_type.as_deref(), Some("Order"));
        assert_eq!(
            bare.parts[0].media_type.as_deref(),
            Some("text/xml; charset=utf-8")
        );
    }

    #[test]
    fn a_document_that_does_not_close_or_that_trails_content_is_refused_where_it_fails() {
        let crossed = Xml
            .shape(&stream(b"<a><b></a>", None))
            .expect_err("crossed tags");
        assert_eq!(crossed.offset, Some(6));
        assert_eq!(
            crossed.to_string(),
            "xml: the end tag does not close the open element at byte 6"
        );

        let open = Xml.shape(&stream(b"<a>", None)).expect_err("never closed");
        assert_eq!(open.offset, Some(3));

        let two = Xml
            .shape(&stream(b"<a/>\n<b/>", None))
            .expect_err("two roots");
        assert_eq!(two.offset, Some(5));
        assert_eq!(two.reason, "content after the document");

        let none = Xml.shape(&stream(b"", None)).expect_err("no root");
        assert_eq!(none.offset, Some(0));
    }

    #[test]
    fn the_shape_claims_xml_and_its_suffixes_and_recognises_a_leading_tag() {
        assert_eq!(Xml.technology(), "xml");
        assert!(Xml.media_types().contains(&"application/xml"));
        assert!(Xml.media_types().contains(&"text/xml"));
        assert!(Xml.media_types().contains(&"application/soap+xml"));
        assert!(Xml.recognises(b"<?xml version=\"1.0\"?><a/>"));
        assert!(Xml.recognises(b"\xef\xbb\xbf\n  <a/>"));
        assert!(Xml.recognises(b"<!-- c --><a/>"));
        assert!(!Xml.recognises(b"{\"a\": 1}"));
        assert!(!Xml.recognises(b"< a/>"));
        assert!(!Xml.recognises(b""));
    }

    #[test]
    fn choose_picks_xml_by_media_type_and_by_look() {
        let shapes: [&dyn Shape; 1] = [&Xml];
        let by_media = message::choose(&shapes, &stream(b"x", Some("Application/XML")));
        assert_eq!(by_media.map(Shape::technology), Some("xml"));
        let by_suffix = message::choose(&shapes, &stream(b"x", Some("image/svg+xml")));
        assert_eq!(by_suffix.map(Shape::technology), Some("xml"));
        let by_look = message::choose(&shapes, &stream(b"<a/>", None));
        assert_eq!(by_look.map(Shape::technology), Some("xml"));
        assert!(message::choose(&shapes, &stream(b"plain", None)).is_none());
    }
}
