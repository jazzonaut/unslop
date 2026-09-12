//! Markdown as the language the model speaks.
//!
//! An 8B asked to rewrite raw HTML returns broken markup. Markdown it handles
//! well, so rich text makes a round trip through it. The trade is that inline
//! styles, fonts and merged cells are flattened: tables, lists, links, headings
//! and emphasis survive, and the original is still on the clipboard until
//! Copy is pressed.

use pulldown_cmark::{Options, Parser};

/// Tables and strikethrough are not in the base specification but are exactly
/// what clipboard content is full of.
fn options() -> Options {
    Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH
}

pub fn from_html(html: &str) -> Option<String> {
    htmd::convert(&drop_invisible_links(&promote_table_headers(html))).ok()
}

/// An anchor with nothing to click on: no text and no image.
///
/// GitHub puts one of these beside every heading as a permalink, and Word
/// leaves them behind as bookmarks. They render as nothing at all, but they
/// carry a URL, which `facts` protects and then requires the model to copy
/// through untouched. The model drops the invisible token, as anyone would,
/// and a perfectly good rewrite is rejected for it.
fn is_invisible_link(link: &kuchikiki::NodeDataRef<kuchikiki::ElementData>) -> bool {
    link.text_contents().trim().is_empty() && link.as_node().select_first("img").is_err()
}

/// Take those anchors out before anything measures or converts the markup, so
/// the shape check and the model see the same document.
fn drop_invisible_links(html: &str) -> String {
    use kuchikiki::traits::*;

    let document = kuchikiki::parse_html().one(html);
    let Ok(links) = document.select("a[href]") else {
        return html.to_owned();
    };
    for link in links.collect::<Vec<_>>() {
        if is_invisible_link(&link) {
            link.as_node().detach();
        }
    }
    serialise(&document, html)
}

/// Put an edited document back together as a fragment, keeping whatever the
/// parser put in either section and falling back to the input when that comes
/// to nothing.
fn serialise(document: &kuchikiki::NodeRef, fallback: &str) -> String {
    let section = |name| {
        document
            .select_first(name)
            .ok()
            .map(|found| {
                found
                    .as_node()
                    .children()
                    .map(|child| child.to_string())
                    .collect::<String>()
            })
            .unwrap_or_default()
    };
    let out = section("head") + &section("body");
    if out.is_empty() {
        fallback.to_owned()
    } else {
        out
    }
}

pub fn to_html(markdown: &str) -> String {
    let mut html = String::with_capacity(markdown.len());
    pulldown_cmark::html::push_html(&mut html, Parser::new_ext(markdown, options()));
    html
}

/// The shape of a document, for noticing when a rewrite has destroyed it.
///
/// Three counts rather than a dozen: the round trip already constrains the
/// structure heavily, and these are the ones a user notices immediately.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Shape {
    pub tables: usize,
    pub links: usize,
    pub list_items: usize,
}

impl Shape {
    /// Measured on the markup itself, which is the only way to notice damage
    /// done by the conversion rather than by the model.
    pub fn of_html(html: &str) -> Self {
        use kuchikiki::traits::*;
        let document = kuchikiki::parse_html().one(html);
        let count = |selector| {
            document
                .select(selector)
                .map(Iterator::count)
                .unwrap_or_default()
        };
        Shape {
            tables: count("table"),
            // An invisible anchor is not a link the user would miss, and
            // `from_html` has already taken it out: see `is_invisible_link`.
            links: document
                .select("a[href]")
                .map(|links| links.filter(|link| !is_invisible_link(link)).count())
                .unwrap_or_default(),
            list_items: count("li"),
        }
    }

    /// What was lost entirely, if anything. Only a count falling to zero is
    /// treated as damage: a model merging two list items is editing, not
    /// vandalism.
    pub fn lost(self, after: Self) -> Option<&'static str> {
        match () {
            () if self.tables > 0 && after.tables == 0 => Some("the table was lost"),
            () if self.links > 0 && after.links == 0 => Some("the links were lost"),
            () if self.list_items > 0 && after.list_items == 0 => Some("the list was lost"),
            () => None,
        }
    }
}

/// Give a headerless table a header row, so it survives the conversion.
///
/// Markdown has no way to express a table without one, and htmd flattens such
/// a table into loose paragraphs. Word and Google Docs both emit tables as
/// plain `<tr><td>` with the first row merely styled, so this is the common
/// case rather than the exotic one.
///
/// The cost is that the first row renders as a header afterwards. That is a
/// far smaller loss than the table disappearing.
pub fn promote_table_headers(html: &str) -> String {
    use html5ever::{QualName, local_name, namespace_url, ns};
    use kuchikiki::{NodeRef, traits::*};

    let document = kuchikiki::parse_html().one(html);
    let Ok(tables) = document.select("table") else {
        return html.to_owned();
    };

    for table in tables {
        let node = table.as_node();
        // A table that already says which row is the header needs no help.
        if node
            .select("th")
            .is_ok_and(|mut found| found.next().is_some())
        {
            continue;
        }
        let Some(first_row) = node.select("tr").ok().and_then(|mut rows| rows.next()) else {
            continue;
        };
        for cell in first_row.as_node().children().collect::<Vec<_>>() {
            if cell
                .as_element()
                .is_none_or(|el| el.name.local.as_ref() != "td")
            {
                continue;
            }
            let header = NodeRef::new_element(
                QualName::new(None, ns!(html), local_name!("th")),
                cell.as_element()
                    .map(|el| el.attributes.borrow().map.clone().into_iter())
                    .into_iter()
                    .flatten(),
            );
            for child in cell.children().collect::<Vec<_>>() {
                header.append(child);
            }
            cell.insert_before(header);
            cell.detach();
        }
    }

    serialise(&document, html)
}
