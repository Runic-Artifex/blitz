//! `text-transform` case mapping as Chromium lays text out.
//!
//! `capitalize` follows Blink's `Capitalize` (platform/text/capitalize.cc, Chromium 153):
//! the text is segmented into words with ICU's word break iterator after the previous
//! character, and the first UTF-16 code unit of each word is mapped to its simple
//! (one-to-one) titlecase. Other characters are unchanged. The previous character is
//! the last code unit of the text laid out before (see `construct.rs`).
//!
//! Word boundaries are Unicode's default ones (UAX #29, through `unicode-segmentation`)
//! with the classes Chromium's ICU rules give differently, as measured in Chromium 153
//! for every code point up to U+1FFFF and the tags block (`a{c}b`, `{c}b`, `1{c}b`).
//!
//! Shaping uses the transformed text; the DOM keeps the source text. [`CaseRevert`]s
//! record the changed characters so that copied text is the source text, as in Chromium.

use std::{borrow::Cow, ops::Range};

use blitz_traits::node_id::NodeId;
use markup5ever::local_name;
use style::values::{
    computed::TextTransform,
    specified::box_::{DisplayInside, DisplayOutside},
};
use unicode_segmentation::UnicodeSegmentation;

use crate::{NodeData, node::TextLayout};

/// Chromium keeps the previous character as one UTF-16 code unit: a supplementary
/// character leaves a lone low surrogate, which breaks words like any other symbol.
pub(crate) fn last_code_unit(text: &str) -> Option<char> {
    text.chars()
        .next_back()
        .map(|c| if (c as u32) > 0xFFFF { '\u{FFFD}' } else { c })
}

/// The character standing for `c` in word segmentation.
fn word_break_char(c: char) -> char {
    match c {
        // Blink replaces no-break spaces with spaces, as ICU does not treat them as
        // word separators.
        '\u{A0}' => ' ',
        // Chromium's ICU breaks words at full stops and colons (UAX #29 keeps
        // "e.g" and "a:b" together; CLDR's root locale already drops the colons).
        '.' | ':' | '\u{FE55}' | '\u{FF0E}' | '\u{FF1A}' => '!',
        // Hangul syllables are segmented by dictionary in ICU, and these are not
        // letters there.
        '\u{3005}'
        | '\u{303B}'
        | '\u{AC00}'..='\u{D7A3}'
        | '\u{16FE3}'
        | '\u{16FF0}'..='\u{16FF1}' => '!',
        // ICU segments complex-context scripts (Thai, Lao, Myanmar, Khmer, Tai scripts,
        // Ahom) by dictionary; next to other letters they join them as letters do.
        '\u{0E01}'..='\u{0E30}'
        | '\u{0E32}'..='\u{0E33}'
        | '\u{0E40}'..='\u{0E46}'
        | '\u{0E81}'..='\u{0E82}'
        | '\u{0E84}'
        | '\u{0E86}'..='\u{0E8A}'
        | '\u{0E8C}'..='\u{0EA3}'
        | '\u{0EA5}'
        | '\u{0EA7}'..='\u{0EB0}'
        | '\u{0EB2}'..='\u{0EB3}'
        | '\u{0EBD}'
        | '\u{0EC0}'..='\u{0EC4}'
        | '\u{0EC6}'
        | '\u{0EDC}'..='\u{0EDF}'
        | '\u{1000}'..='\u{102A}'
        | '\u{103F}'
        | '\u{1050}'..='\u{1055}'
        | '\u{105A}'..='\u{105D}'
        | '\u{1061}'
        | '\u{1065}'..='\u{1066}'
        | '\u{106E}'..='\u{1070}'
        | '\u{1075}'..='\u{1081}'
        | '\u{108E}'
        | '\u{109E}'..='\u{109F}'
        | '\u{1780}'..='\u{17B3}'
        | '\u{17D7}'
        | '\u{17DC}'
        | '\u{1950}'..='\u{196D}'
        | '\u{1970}'..='\u{1974}'
        | '\u{1980}'..='\u{19AB}'
        | '\u{19B0}'..='\u{19C9}'
        | '\u{19DE}'..='\u{19DF}'
        | '\u{1A20}'..='\u{1A54}'
        | '\u{1AA0}'..='\u{1AAD}'
        | '\u{A9E0}'..='\u{A9E4}'
        | '\u{A9E6}'..='\u{A9EF}'
        | '\u{A9FA}'..='\u{A9FE}'
        | '\u{AA60}'..='\u{AA7A}'
        | '\u{AA7E}'..='\u{AAAF}'
        | '\u{AAB1}'
        | '\u{AAB5}'..='\u{AAB6}'
        | '\u{AAB9}'..='\u{AABD}'
        | '\u{AAC0}'
        | '\u{AAC2}'
        | '\u{AADB}'..='\u{AADF}'
        | '\u{11700}'..='\u{1171A}'
        | '\u{1173A}'..='\u{1173B}'
        | '\u{1173F}'..='\u{11746}' => 'a',
        _ => c,
    }
}

/// The simple (one-to-one) titlecase mapping of a BMP character, as ICU's `u_totitle`.
pub(crate) fn simple_titlecase(c: char) -> char {
    match c {
        // Digraphs have titlecase forms of their own.
        '\u{1C4}'..='\u{1C6}' => '\u{1C5}',
        '\u{1C7}'..='\u{1C9}' => '\u{1C8}',
        '\u{1CA}'..='\u{1CC}' => '\u{1CB}',
        '\u{1F1}'..='\u{1F3}' => '\u{1F2}',
        // Georgian Mkhedruli letters are their own titlecase (Mtavruli is uppercase only).
        '\u{10D0}'..='\u{10FA}' | '\u{10FD}'..='\u{10FF}' => c,
        // Greek with ypogegrammeni: the titlecase is the prosgegrammeni letter, whose
        // full uppercase mapping expands.
        '\u{1F80}'..='\u{1F87}' | '\u{1F90}'..='\u{1F97}' | '\u{1FA0}'..='\u{1FA7}' => {
            char::from_u32(c as u32 + 8).unwrap_or(c)
        }
        '\u{1FB3}' => '\u{1FBC}',
        '\u{1FC3}' => '\u{1FCC}',
        '\u{1FF3}' => '\u{1FFC}',
        '\u{1F88}'..='\u{1F8F}'
        | '\u{1F98}'..='\u{1F9F}'
        | '\u{1FA8}'..='\u{1FAF}'
        | '\u{1FBC}'
        | '\u{1FCC}'
        | '\u{1FFC}' => c,
        // Otherwise the uppercase mapping where it is one character (ß, ŉ, ǰ, ﬁ, ...
        // expand and have no simple mapping).
        _ => {
            let mut upper = c.to_uppercase();
            match (upper.next(), upper.next()) {
                (Some(u), None) => u,
                _ => c,
            }
        }
    }
}

/// `text-transform: capitalize` of `text` laid out after `previous`.
pub(crate) fn capitalize(text: &str, previous: char) -> String {
    let mut segmented = String::with_capacity(text.len() + 4);
    segmented.push(word_break_char(previous));
    segmented.extend(text.chars().map(word_break_char));

    // `segmented` holds one character for each of `text`, after the previous one.
    let mut starts = segmented.split_word_bound_indices().map(|(byte, _)| byte);
    let mut next_start = starts.next();
    let mut out = String::with_capacity(text.len());
    for ((byte, _), c) in segmented.char_indices().skip(1).zip(text.chars()) {
        while next_start.is_some_and(|start| start < byte) {
            next_start = starts.next();
        }
        // The first code unit of a word: a supplementary character is a surrogate
        // pair, which Blink leaves as it is, as it does a no-break space.
        if next_start == Some(byte) && (c as u32) <= 0xFFFF && c != '\u{A0}' {
            out.push(simple_titlecase(c));
        } else {
            out.push(c);
        }
    }
    out
}

// --- Chromium's previous character ---------------------------------------------------
//
// Blink's `LayoutText::PreviousCharacter` walks the layout tree backwards in pre-order
// from a text, past inline boxes (`<span>`), empty texts and out-of-flow positioned
// boxes, to the previous text (its last code unit) or any other box (a space). So the
// text after an inline-block, float or text input continues the last text inside it.

/// HTML's ASCII whitespace, as Blink's `ContainsOnlyWhitespaceOrEmpty` tests it.
fn is_html_space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r' | '\u{C}')
}

/// The in-flow layout object before a node among its layout siblings, as Blink's
/// `AttachContext::previous_in_flow` (floats and out-of-flow boxes are skipped).
#[derive(Clone, Copy)]
enum PreviousInFlow {
    Text { ends_with_space: bool },
    Br,
    InlineLevel,
    BlockLevel,
}

/// A node's children in layout order: `::before`, the DOM children, `::after`.
fn layout_children(nodes: &crate::NodeTree, id: NodeId) -> Vec<NodeId> {
    let node = &nodes[id];
    let mut children = Vec::with_capacity(node.children.len() + 2);
    children.extend(node.before());
    children.extend(node.children.iter().copied());
    children.extend(node.after());
    children
}

fn display_of(nodes: &crate::NodeTree, id: NodeId) -> Option<style::values::computed::Display> {
    nodes[id].display_style()
}

/// The last in-flow layout object `id` generates, if any.
fn last_in_flow(nodes: &crate::NodeTree, id: NodeId) -> Option<PreviousInFlow> {
    let node = &nodes[id];
    match &node.data {
        NodeData::Text(data) => {
            if data.content.is_empty()
                || (data.content.chars().all(is_html_space) && !text_has_layout_object(nodes, id))
            {
                return None;
            }
            Some(PreviousInFlow::Text {
                ends_with_space: data.content.ends_with(is_html_space),
            })
        }
        NodeData::Element(element) | NodeData::AnonymousBlock(element) => {
            let display = display_of(nodes, id)?;
            if display.is_none() {
                return None;
            }
            if display.is_contents() {
                return layout_children(nodes, id)
                    .into_iter()
                    .rev()
                    .find_map(|child| last_in_flow(nodes, child));
            }
            let floats = node
                .primary_styles()
                .is_some_and(|s| s.clone_float().is_floating());
            if node.is_out_of_flow() || floats {
                return None;
            }
            if element.name.local == local_name!("br") {
                return Some(PreviousInFlow::Br);
            }
            Some(if display.outside() == DisplayOutside::Inline {
                PreviousInFlow::InlineLevel
            } else {
                PreviousInFlow::BlockLevel
            })
        }
        _ => None,
    }
}

fn previous_in_flow(nodes: &crate::NodeTree, id: NodeId) -> Option<PreviousInFlow> {
    let mut current = id;
    loop {
        let parent = nodes[current].parent?;
        let siblings = layout_children(nodes, parent);
        let index = siblings.iter().position(|&s| s == current)?;
        if let Some(previous) = siblings[..index]
            .iter()
            .rev()
            .find_map(|&sibling| last_in_flow(nodes, sibling))
        {
            return Some(previous);
        }
        if !display_of(nodes, parent).is_some_and(|d| d.is_contents()) {
            return None;
        }
        current = parent;
    }
}

/// Whether Blink makes a layout object for a text node (`Text::TextLayoutObjectIsNeeded`):
/// a whitespace-only text has none at the start of a block, after a block-level box or
/// a `<br>`, after a text ending in whitespace, or among flex, grid and table children.
pub(crate) fn text_has_layout_object(nodes: &crate::NodeTree, id: NodeId) -> bool {
    let NodeData::Text(data) = &nodes[id].data else {
        return false;
    };
    if data.content.is_empty() {
        return false;
    }
    if !data.content.chars().all(is_html_space) {
        return true;
    }
    let Some(mut parent) = nodes[id].parent else {
        return false;
    };
    let preserves_breaks = nodes[parent].primary_styles().is_some_and(|s| {
        use style::computed_values::white_space_collapse::T as Collapse;
        !matches!(s.clone_white_space_collapse(), Collapse::Collapse)
    });
    while display_of(nodes, parent).is_some_and(|d| d.is_contents()) {
        let Some(grandparent) = nodes[parent].parent else {
            break;
        };
        parent = grandparent;
    }
    let previous = previous_in_flow(nodes, id);
    let parent_display = display_of(nodes, parent);
    let without_whitespace_children = parent_display.is_some_and(|d| {
        !matches!(
            d.inside(),
            DisplayInside::Flow | DisplayInside::FlowRoot | DisplayInside::TableCell
        )
    });
    if without_whitespace_children {
        return match previous {
            Some(PreviousInFlow::Text { ends_with_space }) => preserves_breaks || !ends_with_space,
            _ => false,
        };
    }
    if preserves_breaks {
        return true;
    }
    match previous {
        None => parent_display.is_some_and(|d| {
            d.outside() == DisplayOutside::Inline
                && d.inside() == DisplayInside::Flow
                && nodes[parent]
                    .element_data()
                    .is_some_and(|el| !super::replaced::is_inline_box_element(&el.name.local))
        }),
        Some(PreviousInFlow::Text { ends_with_space }) => !ends_with_space,
        Some(PreviousInFlow::Br | PreviousInFlow::BlockLevel) => false,
        Some(PreviousInFlow::InlineLevel) => true,
    }
}

/// The previous character for the text after the inline-level box `id` (an atomic
/// inline, a float or an out-of-flow box): the last text in it, a space for any other
/// box, or `None` where the walk passes through it (an out-of-flow box with no text).
pub(crate) fn previous_after_box(nodes: &crate::NodeTree, id: NodeId) -> Option<char> {
    let node = &nodes[id];
    match &node.data {
        NodeData::Text(data) => text_has_layout_object(nodes, id)
            .then(|| last_code_unit(&data.content))
            .flatten(),
        NodeData::Element(element) | NodeData::AnonymousBlock(element) => {
            let display = display_of(nodes, id)?;
            if display.is_none() {
                return None;
            }
            let tag = &element.name.local;
            if *tag == local_name!("br") {
                return Some('\n');
            }
            if *tag == local_name!("wbr") {
                return None;
            }
            // Text controls end with their inner editor's text; a password's is masked.
            if *tag == local_name!("input") || *tag == local_name!("textarea") {
                if *tag == local_name!("input") && element.effective_input_type() == "password" {
                    return Some('\u{2022}');
                }
                return Some(
                    element
                        .text_input_data()
                        .and_then(|input| last_code_unit(input.editor.raw_text()))
                        .unwrap_or(' '),
                );
            }
            if super::replaced::is_replaced_element(tag) {
                return Some(' ');
            }
            if let Some(c) = layout_children(nodes, id)
                .into_iter()
                .rev()
                .find_map(|child| previous_after_box(nodes, child))
            {
                return Some(c);
            }
            let inline_flow = display.outside() == DisplayOutside::Inline
                && display.inside() == DisplayInside::Flow
                && !super::replaced::is_inline_box_element(tag);
            if node.is_out_of_flow() || display.is_contents() || inline_flow {
                None
            } else {
                Some(' ')
            }
        }
        _ => None,
    }
}

// --- Copied text ------------------------------------------------------------------------

/// A character that `text-transform` changed, at its bytes in the laid-out text.
#[derive(Clone, Debug, PartialEq)]
pub struct CaseRevert {
    pub range: Range<usize>,
    pub original: char,
}

/// The characters one text node's transform changed: their first transformed
/// character's index among the non-whitespace characters, how many characters they
/// became and the original. White space collapsing never touches these.
#[derive(Default)]
pub(crate) struct PendingReverts {
    source: u64,
    changed: Vec<(usize, usize, char)>,
}

impl PendingReverts {
    /// `transformed` is `text` mapped character by character (`str::to_uppercase`,
    /// `str::to_lowercase`, whose final sigma is one character as well, or `capitalize`).
    pub(crate) fn new(
        source: u64,
        text: &str,
        transformed: &str,
        piece_len: impl Fn(char) -> usize,
    ) -> Self {
        let mut changed = Vec::new();
        let mut out = transformed.chars();
        let mut ordinal = 0;
        for c in text.chars() {
            let len = piece_len(c);
            let mut same = true;
            for i in 0..len {
                match out.next() {
                    Some(t) => same &= i == 0 && t == c,
                    None => same = false,
                }
            }
            if c.is_whitespace() {
                continue;
            }
            if !same || len != 1 {
                changed.push((ordinal, len, c));
            }
            ordinal += len;
        }
        Self { source, changed }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.changed.is_empty()
    }
}

/// Locates the changed characters in the laid-out text (`sources` are its ranges per
/// text node, in order).
pub(crate) fn resolve_reverts(
    text: &str,
    sources: &[parley::TextSourceRange],
    pending: &[PendingReverts],
) -> Vec<CaseRevert> {
    let mut reverts = Vec::new();
    for node in pending {
        let mut changed = node.changed.iter().peekable();
        let mut ordinal = 0;
        // (start byte, characters left, original) of the piece being matched.
        let mut open: Option<(usize, usize, char)> = None;
        'ranges: for source in sources.iter().filter(|s| s.source == node.source) {
            for (offset, c) in text[source.range.clone()].char_indices() {
                if c.is_whitespace() {
                    continue;
                }
                let byte = source.range.start + offset;
                if open.is_none() {
                    match changed.peek() {
                        Some(&&(at, len, original)) if at == ordinal => {
                            changed.next();
                            open = Some((byte, len, original));
                        }
                        Some(_) => {}
                        None => break 'ranges,
                    }
                }
                ordinal += 1;
                if let Some((start, left, original)) = open {
                    if left == 1 {
                        reverts.push(CaseRevert {
                            range: start..byte + c.len_utf8(),
                            original,
                        });
                        open = None;
                    } else {
                        open = Some((start, left - 1, original));
                    }
                }
            }
        }
    }
    reverts.sort_by_key(|r| r.range.start);
    reverts
}

impl TextLayout {
    /// The source text of a laid-out range, as copied: the characters `text-transform`
    /// changed are the DOM's (white space stays collapsed).
    pub(crate) fn source_text(&self, range: Range<usize>) -> String {
        let mut out = String::with_capacity(range.len());
        let mut cursor = range.start;
        let first = self
            .case_reverts
            .partition_point(|r| r.range.end <= range.start);
        for revert in &self.case_reverts[first..] {
            if revert.range.start >= range.end {
                break;
            }
            if revert.range.start >= cursor {
                out.push_str(&self.text[cursor..revert.range.start]);
                out.push(revert.original);
            }
            // A piece selected from its middle contributes nothing.
            cursor = revert.range.end.min(range.end);
        }
        if cursor < range.end {
            out.push_str(&self.text[cursor..range.end]);
        }
        out
    }
}

/// The case transform state of one inline formatting context being built.
pub(crate) struct CaseContext {
    /// Blink's previous character for the next text (a space at the start of a block).
    pub(crate) previous: char,
    pub(crate) pending: Vec<PendingReverts>,
}

impl CaseContext {
    pub(crate) fn new() -> Self {
        Self {
            previous: ' ',
            pending: Vec::new(),
        }
    }

    /// The laid-out text of the text node `id` under `transform` (its parent's).
    pub(crate) fn text<'a>(
        &mut self,
        nodes: &crate::NodeTree,
        id: NodeId,
        text: &'a str,
        transform: TextTransform,
    ) -> Cow<'a, str> {
        let transformed: Cow<'a, str> = match transform {
            TextTransform::UPPERCASE => {
                let out = text.to_uppercase();
                self.record(id, text, &out, |c| c.to_uppercase().count());
                out.into()
            }
            TextTransform::LOWERCASE => {
                let out = text.to_lowercase();
                self.record(id, text, &out, |c| c.to_lowercase().count());
                out.into()
            }
            TextTransform::CAPITALIZE => {
                let out = capitalize(text, self.previous);
                self.record(id, text, &out, |_| 1);
                out.into()
            }
            _ => text.into(),
        };
        // White space after white space breaks words alike, laid out or not (Blink
        // drops some white space texts); finding out walks the siblings.
        let spaces_after_space =
            is_html_space(self.previous) && transformed.chars().all(is_html_space);
        if !spaces_after_space && text_has_layout_object(nodes, id) {
            if let Some(c) = last_code_unit(&transformed) {
                self.previous = c;
            }
        }
        transformed
    }

    fn record(&mut self, id: NodeId, text: &str, out: &str, piece_len: impl Fn(char) -> usize) {
        if text == out {
            return;
        }
        let pending = PendingReverts::new(id.as_u64(), text, out, piece_len);
        if !pending.is_empty() {
            self.pending.push(pending);
        }
    }

    /// After the inline-level box `id`.
    pub(crate) fn after_box(&mut self, nodes: &crate::NodeTree, id: NodeId) {
        if let Some(c) = previous_after_box(nodes, id) {
            self.previous = c;
        }
    }
}

#[cfg(test)]
mod tests {
    //! Expected text as Chromium 153 lays it out (`innerText`, which applies
    //! text-transform) and copies it (the clipboard after Ctrl+C).

    use blitz_traits::shell::{ColorScheme, Viewport};

    use super::*;
    use crate::{Attribute, BaseDocument, DocumentConfig, QualName, qual_name};

    #[test]
    fn capitalizes_words_as_chromium() {
        for (text, expected) in [
            ("hello world", "Hello World"),
            ("foo-bar baz_qux", "Foo-Bar Baz_qux"),
            ("don't it's rock'n'roll", "Don't It's Rock'n'roll"),
            ("don\u{2019}t", "Don\u{2019}t"),
            ("3rd 1st x2y 12abc 1.5em", "3rd 1st X2y 12abc 1.5em"),
            (
                "e.g. a.b a,b a;b a:b a/b a@b",
                "E.G. A.B A,B A;B A:B A/B A@B",
            ),
            (
                "(paren) \u{ab}guill\u{bb} 'single' \"double\"",
                "(Paren) \u{ab}Guill\u{bb} 'Single' \"Double\"",
            ),
            ("HELLO wORLD mIxEd McDONALD", "HELLO WORLD MIxEd McDONALD"),
            (
                "\u{e9}lan \u{df}tra\u{df}e stra\u{df}e",
                "\u{c9}lan \u{df}tra\u{df}e Stra\u{df}e",
            ),
            (
                "\u{1c6}emal \u{1c9}ubav \u{1c4}X",
                "\u{1c5}emal \u{1c8}ubav \u{1c5}X",
            ),
            (
                "\u{fb01}sh \u{149}a \u{1f0}a",
                "\u{fb01}sh \u{149}a \u{1f0}a",
            ),
            (
                "\u{3b1}\u{3b2}\u{3b3} \u{1f00}\u{3c1}\u{3c7}\u{3ae} \u{390}a",
                "\u{391}\u{3b2}\u{3b3} \u{1f08}\u{3c1}\u{3c7}\u{3ae} \u{390}a",
            ),
            (
                "\u{131}i istanbul \u{10d0}\u{10d1} \u{1f80}",
                "Ii Istanbul \u{10d0}\u{10d1} \u{1f88}",
            ),
            ("a\u{a0}b", "A\u{a0}B"),
            ("e\u{301}clair \u{301}x", "E\u{301}clair \u{301}X"),
            ("\u{10428}\u{10428} z", "\u{10428}\u{10428} Z"),
            (
                "a\u{ad}b a\u{200b}b a\u{200d}b",
                "A\u{ad}b A\u{200b}B A\u{200d}b",
            ),
            ("x_1 _y __z", "X_1 _y __z"),
            ("1a 1-a a1 a-1 1.a", "1a 1-A A1 A-1 1.A"),
            (
                "a\u{e01}b \u{e01}b a\u{ac00}b",
                "A\u{e01}b \u{e01}b A\u{ac00}B",
            ),
        ] {
            assert_eq!(capitalize(text, ' '), expected, "{text:?}");
        }
        // The previous text's last code unit continues or ends the word.
        assert_eq!(capitalize("bc", 'a'), "bc");
        assert_eq!(capitalize("t", '\''), "T");
        assert_eq!(capitalize("'t", 'n'), "'t");
        assert_eq!(capitalize("b", '\u{a0}'), "B");
        assert_eq!(capitalize("b", last_code_unit("\u{10428}").unwrap()), "B");
    }

    enum Html {
        El(&'static str, &'static str, Vec<Html>),
        Text(&'static str),
    }
    use Html::{El, Text};

    /// Resolve `<html><body>{content}</body></html>`; the ids of the elements in order.
    fn resolve(content: Html) -> (BaseDocument, Vec<NodeId>) {
        let mut doc = BaseDocument::new(DocumentConfig {
            viewport: Some(Viewport::new(800, 600, 1.0, ColorScheme::Light)),
            ..Default::default()
        });
        let root = doc.root_node().id;
        let mut ids = Vec::new();
        let mut mutator = doc.mutate();
        fn build(
            mutator: &mut crate::DocumentMutator<'_>,
            html: Html,
            ids: &mut Vec<NodeId>,
        ) -> NodeId {
            match html {
                Text(text) => mutator.create_text_node(text),
                El(tag, style, children) => {
                    let name = QualName::new(None, markup5ever::ns!(html), tag.into());
                    let attrs = vec![Attribute {
                        name: qual_name!("style"),
                        value: style.to_string(),
                    }];
                    let id = mutator.create_element(name, attrs);
                    ids.push(id);
                    let children: Vec<NodeId> = children
                        .into_iter()
                        .map(|child| build(mutator, child, ids))
                        .collect();
                    mutator.append_children(id, &children);
                    id
                }
            }
        }
        let html = mutator.create_element(qual_name!("html", html), vec![]);
        let body = mutator.create_element(qual_name!("body", html), vec![]);
        let content = build(&mut mutator, content, &mut ids);
        mutator.append_children(body, &[content]);
        mutator.append_children(html, &[body]);
        mutator.append_children(root, &[html]);
        drop(mutator);
        doc.resolve(0.0);
        (doc, ids)
    }

    fn laid_out(doc: &BaseDocument, id: NodeId) -> &TextLayout {
        doc.nodes[id]
            .element_data()
            .unwrap()
            .inline_layout_data
            .as_deref()
            .unwrap()
    }

    const C: &str = "text-transform:capitalize";

    fn p(children: Vec<Html>) -> Html {
        El("p", C, children)
    }

    fn capitalized(children: Vec<Html>) -> String {
        let (doc, ids) = resolve(p(children));
        laid_out(&doc, ids[0]).text.clone()
    }

    #[test]
    fn words_continue_across_inline_elements() {
        assert_eq!(
            capitalized(vec![El("b", "", vec![Text("a")]), Text("bc")]),
            "Abc"
        );
        assert_eq!(
            capitalized(vec![Text("a"), El("b", "", vec![Text("bc")])]),
            "Abc"
        );
        assert_eq!(
            capitalized(vec![
                Text("ab"),
                El("span", "", vec![Text("c d")]),
                Text("e")
            ]),
            "Abc De"
        );
        assert_eq!(
            capitalized(vec![Text("foo "), El("b", "", vec![Text("bar")])]),
            "Foo Bar"
        );
        // One previous code unit: "'" then "t" is a word of its own.
        assert_eq!(
            capitalized(vec![Text("don'"), El("b", "", vec![Text("t")])]),
            "Don'T"
        );
        assert_eq!(
            capitalized(vec![Text("don"), El("b", "", vec![Text("'t")])]),
            "Don't"
        );
        assert_eq!(
            capitalized(vec![Text("abc"), El("br", "", vec![]), Text("def")]),
            "Abc\nDef"
        );
        assert_eq!(
            capitalized(vec![
                Text("ab"),
                El("span", "display:none", vec![Text("x")]),
                Text("cd")
            ]),
            "Abcd"
        );
        assert_eq!(
            capitalized(vec![
                Text("ab"),
                El("span", "display:contents", vec![Text("cd ef")]),
                Text("gh"),
            ]),
            "Abcd Efgh"
        );
        // Text without the transform still precedes.
        assert_eq!(
            capitalized(vec![
                El("span", "text-transform:none", vec![Text("ab")]),
                El("span", C, vec![Text("cd")]),
            ]),
            "abcd"
        );
        assert_eq!(
            capitalized(vec![
                El("span", "text-transform:uppercase", vec![Text("ab")]),
                Text("cd"),
            ]),
            "ABcd"
        );
    }

    #[test]
    fn boxes_end_with_their_last_text() {
        let ib = "display:inline-block";
        let abs = "position:absolute";
        // The inline box's text is its own layout, not the paragraph's.
        let (doc, ids) = resolve(p(vec![
            Text("abc"),
            El("span", ib, vec![Text("x")]),
            Text("def"),
        ]));
        assert_eq!(laid_out(&doc, ids[0]).text, "Abcdef");
        assert_eq!(laid_out(&doc, ids[1]).text, "X");
        assert_eq!(
            capitalized(vec![Text("abc"), El("span", ib, vec![]), Text("def")]),
            "AbcDef"
        );
        assert_eq!(
            capitalized(vec![
                Text("abc"),
                El("span", ib, vec![Text("x ")]),
                Text("def")
            ]),
            "AbcDef"
        );
        assert_eq!(
            capitalized(vec![
                El(
                    "span",
                    ib,
                    vec![El("span", "display:block", vec![Text("x")])]
                ),
                Text("cd"),
            ]),
            "cd"
        );
        // Out-of-flow boxes without text are passed through; floats are boxes.
        assert_eq!(
            capitalized(vec![Text("ab"), El("span", abs, vec![]), Text("cd")]),
            "Abcd"
        );
        assert_eq!(
            capitalized(vec![
                Text("ab"),
                El("span", abs, vec![Text(" ")]),
                Text("cd")
            ]),
            "Abcd"
        );
        assert_eq!(
            capitalized(vec![
                Text("ab"),
                El("span", abs, vec![Text("x")]),
                Text("cd")
            ]),
            "Abcd"
        );
        assert_eq!(
            capitalized(vec![
                Text("ab"),
                El("span", "float:left", vec![]),
                Text("cd")
            ]),
            "AbCd"
        );
        // A white space text after an out-of-flow box or a float at the start of a block
        // has no layout object in Blink: the text before it is the box's (the space
        // itself collapses at the start of the line).
        assert_eq!(
            capitalized(vec![
                El("span", abs, vec![Text("x")]),
                Text(" "),
                El("b", "", vec![Text("cd")]),
            ]),
            "cd"
        );
        assert_eq!(
            capitalized(vec![
                El("span", "float:left", vec![Text("x")]),
                Text(" "),
                El("b", "", vec![Text("cd")]),
            ]),
            "cd"
        );
        assert_eq!(
            capitalized(vec![
                El("span", ib, vec![Text("x")]),
                Text(" "),
                El("b", "", vec![Text("cd")])
            ]),
            " Cd"
        );
        assert_eq!(
            capitalized(vec![Text("ab"), El("img", "", vec![]), Text("cd")]),
            "AbCd"
        );
        assert_eq!(
            capitalized(vec![Text("ab"), El("wbr", "", vec![]), Text("cd")]),
            "Abcd"
        );
    }

    #[test]
    fn copies_the_source_text() {
        let (mut doc, ids) = resolve(p(vec![
            Text("hello "),
            El("b", "", vec![Text("big")]),
            Text(" world"),
        ]));
        let root = ids[0];
        assert_eq!(laid_out(&doc, root).text, "Hello Big World");
        doc.set_text_selection(root, 0, root, 15);
        assert_eq!(doc.get_selected_text().as_deref(), Some("hello big world"));
        doc.set_text_selection(root, 3, root, 12);
        assert_eq!(doc.get_selected_text().as_deref(), Some("lo big wo"));

        let (mut doc, ids) = resolve(El(
            "p",
            "text-transform:uppercase",
            vec![Text("stra\u{df}e  \u{131}x")],
        ));
        let root = ids[0];
        let text = laid_out(&doc, root).text.clone();
        assert_eq!(text, "STRASSE IX");
        doc.set_text_selection(root, 0, root, text.len());
        assert_eq!(
            doc.get_selected_text().as_deref(),
            Some("stra\u{df}e \u{131}x")
        );
        // From the middle of a character's expansion.
        doc.set_text_selection(root, 5, root, text.len());
        assert_eq!(doc.get_selected_text().as_deref(), Some("e \u{131}x"));

        let (mut doc, ids) = resolve(El(
            "p",
            "text-transform:lowercase",
            vec![Text("ABC D\u{3a3}")],
        ));
        let root = ids[0];
        let text = laid_out(&doc, root).text.clone();
        assert_eq!(text, "abc d\u{3c2}");
        doc.set_text_selection(root, 0, root, text.len());
        assert_eq!(doc.get_selected_text().as_deref(), Some("ABC D\u{3a3}"));
    }

    #[test]
    fn follows_text_changes() {
        let (mut doc, ids) = resolve(p(vec![Text("abc")]));
        let text = doc.nodes[ids[0]].children[0];
        doc.mutate().set_node_text(text, "xyz uvw");
        doc.resolve(0.0);
        assert_eq!(laid_out(&doc, ids[0]).text, "Xyz Uvw");
    }
}
