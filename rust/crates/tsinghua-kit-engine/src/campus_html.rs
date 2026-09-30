//! Bounded, dependency-free HTML scanning shared by the campus read adapters.
//!
//! The legacy campus services are pre-framework ASP.NET / JSP pages.  They are
//! not well-formed XML, so a strict parser cannot be used.  This module offers
//! the smallest primitive the domain adapters need: locate elements by tag and
//! attribute, and read their text.
//!
//! Three rules keep the result trustworthy:
//!
//! * Every scan is bounded.  Pathological input fails with an error instead of
//!   growing an allocation, because the response is untrusted.
//! * An element that starts but never closes is an error, never a truncated
//!   result.  A silently shortened table would look like a smaller data set.
//! * Nothing here decides business meaning.  A domain adapter must still
//!   validate that a value came from the expected selector and that the page
//!   was not a login or expiry page.
//!
//! The reference implementations use `cheerio` plus raw DOM child indices.
//! This module deliberately does not reproduce positional indexing: it exposes
//! attributes and text so an adapter can key on semantic labels instead.

use std::fmt;

/// Upper bound on elements collected from one response.
pub(crate) const MAX_ELEMENTS: usize = 16_384;
/// Upper bound on one element's raw source slice.
pub(crate) const MAX_ELEMENT_BYTES: usize = 1024 * 1024;

/// A scanning failure.  It never retains response bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ScanError {
    /// The response exceeded a bounded scanning limit.
    TooLarge { context: &'static str },
    /// An element started but never closed before the response ended.
    Unbalanced { context: &'static str },
}

impl fmt::Display for ScanError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooLarge { context } => write!(formatter, "html scan limit reached: {context}"),
            Self::Unbalanced { context } => {
                write!(formatter, "html element is not closed: {context}")
            }
        }
    }
}

/// How a page should be classified before any business selector is applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PageClass {
    /// The WebVPN or identity login page, so the session is not established.
    Login,
    /// The service's own session-expiry page.
    Expired,
    /// Neither marker was found; the caller still has to validate structure.
    Unknown,
}

/// The reference clients' shared expiry message.  The leading English fragment
/// is dropped so a reworded prefix does not change the classification.
pub(crate) const EXPIRED_MARKER: &str = "用户登陆超时或访问内容不存在";
/// A second observed expiry wording used by the ASP.NET applications.
pub(crate) const EXPIRED_MARKER_LEGACY: &str = "用户登录超时或访问内容不存在";
/// WebVPN's own page title, present on every unauthenticated mapping hop.
pub(crate) const WEBVPN_TITLE_MARKER: &str = "清华大学WebVPN";
/// The identity handoff interstitial shown before the WebVPN portal.
pub(crate) const HANDOFF_MARKER: &str = "您即将登录";
/// The ASP.NET login control shared by the dormitory hosts.
pub(crate) const ASPNET_LOGIN_CONTROL: &str = "net_Default_LoginCtrl1_txtUserName";

/// Detects the session-level markers before any domain parsing runs.
///
/// This runs first so an HTTP 200 login page cannot become an empty or
/// partially parsed business result.
pub(crate) fn classify_page(body: &str) -> PageClass {
    if body.contains(WEBVPN_TITLE_MARKER)
        || body.contains(HANDOFF_MARKER)
        || body.contains(ASPNET_LOGIN_CONTROL)
    {
        return PageClass::Login;
    }
    if body.contains(EXPIRED_MARKER) || body.contains(EXPIRED_MARKER_LEGACY) {
        return PageClass::Expired;
    }
    PageClass::Unknown
}

/// One scanned element: its name, attributes, raw source, and inner source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RawElement {
    name: String,
    attrs: Vec<(String, String)>,
    outer: String,
    inner: String,
}

impl RawElement {
    /// Returns the lowercased attribute value for `name`.
    pub(crate) fn attr(&self, name: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }

    /// Returns the raw source between the start and end tag.
    pub(crate) fn inner(&self) -> &str {
        &self.inner
    }

    /// Returns whether one of the space-separated class tokens matches.
    pub(crate) fn has_class(&self, class: &str) -> bool {
        self.attr("class")
            .is_some_and(|value| value.split_whitespace().any(|token| token == class))
    }

    /// Returns the visible text of this element with entities decoded.
    pub(crate) fn text(&self) -> String {
        element_text(&self.inner)
    }
}

/// Scans `body` for every element named `name` at any nesting depth.
pub(crate) fn scan(body: &str, name: &str) -> Result<Vec<RawElement>, ScanError> {
    let mut found = Vec::new();
    let mut index = 0usize;
    while let Some((start, end)) = next_element(body, index, name)? {
        found.push(parse_element(name, &body[start..end])?);
        if found.len() > MAX_ELEMENTS {
            return Err(ScanError::TooLarge {
                context: "element count",
            });
        }
        index = end;
    }
    Ok(found)
}

/// Scans and keeps only elements whose `class` attribute contains `class`.
pub(crate) fn scan_with_class(
    body: &str,
    name: &str,
    class: &str,
) -> Result<Vec<RawElement>, ScanError> {
    Ok(scan(body, name)?
        .into_iter()
        .filter(|element| element.has_class(class))
        .collect())
}

/// Scans and keeps only elements whose `id` attribute equals `id`.
pub(crate) fn scan_with_id(body: &str, name: &str, id: &str) -> Result<Vec<RawElement>, ScanError> {
    Ok(scan(body, name)?
        .into_iter()
        .filter(|element| element.attr("id") == Some(id))
        .collect())
}

/// Returns the direct child elements of `container` named `name`.
///
/// A nested element with the same name is skipped, so a table inside a table
/// cannot contribute rows to its parent.
pub(crate) fn direct_children(container: &str, name: &str) -> Result<Vec<RawElement>, ScanError> {
    let mut found = Vec::new();
    let mut index = 0usize;
    while let Some((start, end)) = next_element(container, index, name)? {
        found.push(parse_element(name, &container[start..end])?);
        if found.len() > MAX_ELEMENTS {
            return Err(ScanError::TooLarge {
                context: "child element count",
            });
        }
        index = end;
    }
    Ok(found)
}

/// Extracts visible text from a fragment: tags removed, entities decoded,
/// whitespace collapsed, and `script`/`style`/`template` contents dropped.
///
/// Line-breaking tags contribute a separating space so two adjacent cells do
/// not run together; purely inline markup (such as `<strong>`) does not, which
/// matches how the reference clients read `"总学分：<strong>112</strong>"` as
/// one unbroken token.
pub(crate) fn element_text(fragment: &str) -> String {
    let mut out = String::with_capacity(fragment.len().min(4096));
    let mut index = 0usize;
    while index < fragment.len() {
        if !fragment.is_char_boundary(index) {
            index += 1;
            continue;
        }
        if fragment.as_bytes()[index] != b'<' {
            let next = fragment[index..]
                .find('<')
                .map(|offset| index + offset)
                .unwrap_or(fragment.len());
            out.push_str(&fragment[index..next]);
            index = next;
            continue;
        }
        if fragment[index..].starts_with("<!--") {
            index = skip_comment(fragment, index);
            continue;
        }
        let Some(end) = find_tag_end(fragment, index) else {
            break;
        };
        let source = &fragment[index + 1..end];
        let name = tag_name(source);
        let closing = fragment[index..].starts_with("</");
        let self_closing = is_void(&name) || source.trim_end().ends_with('/');
        let dropped = matches!(name.as_str(), "script" | "style" | "template" | "noscript");
        if !closing && !self_closing && dropped {
            index = match find_matching_close(fragment, end + 1, &name) {
                Ok(Some((_, close_end))) => close_end,
                // A malformed script/style block must not silently truncate
                // the page; keep the remainder readable instead of dropping
                // the rest of the document.
                Ok(None) | Err(_) => fragment.len(),
            };
            out.push(' ');
            continue;
        }
        if self_closing && !closing && breaks_line(&name) {
            out.push(' ');
        }
        if closing && breaks_line(&name) {
            out.push(' ');
        }
        index = end + 1;
    }
    normalize_text(&out)
}

/// Collapses whitespace and decodes entities in an already-tag-free fragment.
pub(crate) fn normalize_text(value: &str) -> String {
    decode_entities(&collapse_whitespace(value))
}

fn collapse_whitespace(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut pending_space = false;
    for character in value.chars() {
        let is_space = character.is_whitespace() || character == '\u{a0}';
        if is_space {
            pending_space = !out.is_empty();
            continue;
        }
        if pending_space {
            out.push(' ');
            pending_space = false;
        }
        out.push(character);
    }
    out
}

/// Decodes the small entity set that the campus pages actually emit.  A
/// numeric reference outside the Unicode range is left untouched rather than
/// replaced, because a wrong character is worse than a literal one.
fn decode_entities(value: &str) -> String {
    if !value.contains('&') {
        return value.to_owned();
    }
    let mut out = String::with_capacity(value.len());
    let mut remainder = value;
    while let Some(position) = remainder.find('&') {
        out.push_str(&remainder[..position]);
        remainder = &remainder[position..];
        let window = &remainder[..remainder.len().min(32)];
        let Some(semicolon) = window.find(';') else {
            out.push('&');
            remainder = &remainder[1..];
            continue;
        };
        let entity = &remainder[1..semicolon];
        let decoded = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "nbsp" => Some(' '),
            "hellip" => Some('…'),
            "middot" => Some('·'),
            _ => decode_numeric_entity(entity),
        };
        match decoded {
            Some(character) => {
                out.push(character);
                remainder = &remainder[semicolon + 1..];
            }
            None => {
                out.push('&');
                remainder = &remainder[1..];
            }
        }
    }
    out.push_str(remainder);
    out
}

fn decode_numeric_entity(entity: &str) -> Option<char> {
    let digits = entity.strip_prefix('#')?;
    let code = match digits.strip_prefix(['x', 'X']) {
        Some(hex) => u32::from_str_radix(hex, 16).ok()?,
        None => digits.parse::<u32>().ok()?,
    };
    char::from_u32(code)
}

fn skip_comment(body: &str, index: usize) -> usize {
    match body[index..].find("-->") {
        Some(offset) => index + offset + 3,
        None => body.len(),
    }
}

/// HTML void elements, which never have a close tag.
fn is_void(name: &str) -> bool {
    matches!(
        name,
        "area"
            | "base"
            | "br"
            | "col"
            | "embed"
            | "hr"
            | "img"
            | "input"
            | "link"
            | "meta"
            | "param"
            | "source"
            | "track"
            | "wbr"
    )
}

/// Tags whose boundaries separate readable tokens.
fn breaks_line(name: &str) -> bool {
    matches!(
        name,
        "br" | "p"
            | "div"
            | "tr"
            | "td"
            | "th"
            | "li"
            | "ul"
            | "ol"
            | "table"
            | "h1"
            | "h2"
            | "h3"
            | "h4"
            | "h5"
            | "h6"
    )
}

/// Builds a [`RawElement`] from a source slice that starts at `<name`.
fn parse_element(name: &str, source: &str) -> Result<RawElement, ScanError> {
    if source.len() > MAX_ELEMENT_BYTES {
        return Err(ScanError::TooLarge {
            context: "element size",
        });
    }
    let Some(open_end) = find_tag_end(source, 0) else {
        return Err(ScanError::Unbalanced {
            context: "element start tag",
        });
    };
    let attrs = parse_attributes(&source[1 + name.len()..open_end])
        .into_iter()
        .map(|(key, value)| (key.to_ascii_lowercase(), normalize_text(&value)))
        .collect();
    let self_closing = is_void(name) || source[..=open_end].trim_end().ends_with("/>");
    let inner = if self_closing {
        String::new()
    } else {
        match find_matching_close(source, open_end + 1, name)? {
            Some((close_start, close_end)) => {
                if close_end != source.len() {
                    // Trailing bytes mean the slice was not a single element.
                    return Err(ScanError::Unbalanced {
                        context: "element boundary",
                    });
                }
                source[open_end + 1..close_start].to_owned()
            }
            None => {
                return Err(ScanError::Unbalanced {
                    context: "element close tag",
                });
            }
        }
    };
    Ok(RawElement {
        name: name.to_owned(),
        attrs,
        outer: source.to_owned(),
        inner,
    })
}

/// Finds the byte range of the next `<name ...>...</name>` starting at `from`.
///
/// Returns `Ok(None)` when no further element begins.  An element that begins
/// but cannot be closed is an error, not the end of the scan.
fn next_element(body: &str, from: usize, name: &str) -> Result<Option<(usize, usize)>, ScanError> {
    let mut index = from;
    while index < body.len() {
        if !body.is_char_boundary(index) {
            index += 1;
            continue;
        }
        if body.as_bytes()[index] != b'<' {
            index += 1;
            continue;
        }
        if body[index..].starts_with("<!--") {
            index = skip_comment(body, index);
            continue;
        }
        if body[index..].starts_with("</") {
            index += 2;
            continue;
        }
        if starts_with_tag_name(&body[index + 1..], name) {
            let Some(open_end) = find_tag_end(body, index) else {
                return Err(ScanError::Unbalanced {
                    context: "element start tag",
                });
            };
            if is_void(name) || body[index..=open_end].trim_end().ends_with("/>") {
                return Ok(Some((index, open_end + 1)));
            }
            return match find_matching_close(body, open_end + 1, name)? {
                Some((_, close_end)) => Ok(Some((index, close_end))),
                None => Err(ScanError::Unbalanced {
                    context: "element close tag",
                }),
            };
        }
        index += 1;
    }
    Ok(None)
}

/// Returns the lowercase tag name that begins a start-tag source.
fn tag_name(source: &str) -> String {
    source
        .trim_start_matches('/')
        .split_whitespace()
        .next()
        .unwrap_or("")
        .trim_end_matches('/')
        .to_ascii_lowercase()
}

/// Returns whether `rest` begins with `name` followed by a tag delimiter.
fn starts_with_tag_name(rest: &str, name: &str) -> bool {
    let Some(head) = rest.get(..name.len()) else {
        return false;
    };
    if !head.eq_ignore_ascii_case(name) {
        return false;
    }
    rest[name.len()..]
        .chars()
        .next()
        .is_some_and(|character| character.is_whitespace() || character == '>' || character == '/')
}

/// Returns the index of the `>` that closes a tag starting at `start`.
fn find_tag_end(body: &str, start: usize) -> Option<usize> {
    let bytes = body.as_bytes();
    let mut index = start;
    let mut quote: Option<u8> = None;
    let mut after_equals = false;
    while index < bytes.len() {
        let byte = bytes[index];
        match quote {
            Some(delimiter) => {
                if byte == delimiter {
                    quote = None;
                }
            }
            None => match byte {
                // A quote opens an attribute value only directly after `=`.  The
                // deployed identity page carries `style="color:#8b0000;""`; that
                // second quote is an HTML5 attribute name, and treating it as an
                // unterminated value would hide every tag after it.
                b'"' | b'\'' if after_equals => quote = Some(byte),
                b'>' => return Some(index),
                _ => {}
            },
        }
        if !byte.is_ascii_whitespace() {
            after_equals = byte == b'=';
        }
        index += 1;
    }
    None
}

/// Finds the close tag that matches an already-open `name` element.
///
/// Returns `(index_of_less_than, index_after_greater_than)`.  Nested elements
/// with the same name are counted so an inner table cannot terminate an outer
/// one.
fn find_matching_close(
    body: &str,
    from: usize,
    name: &str,
) -> Result<Option<(usize, usize)>, ScanError> {
    let mut index = from;
    let mut depth = 1usize;
    while index < body.len() {
        if !body.is_char_boundary(index) {
            index += 1;
            continue;
        }
        if body.as_bytes()[index] != b'<' {
            index += 1;
            continue;
        }
        if body[index..].starts_with("<!--") {
            index = skip_comment(body, index);
            continue;
        }
        if body[index..].starts_with("</") {
            if starts_with_tag_name(&body[index + 2..], name) {
                let Some(close_end) = find_tag_end(body, index) else {
                    return Err(ScanError::Unbalanced {
                        context: "element close tag",
                    });
                };
                depth -= 1;
                if depth == 0 {
                    return Ok(Some((index, close_end + 1)));
                }
                index = close_end + 1;
                continue;
            }
            index += 2;
            continue;
        }
        if starts_with_tag_name(&body[index + 1..], name) && !is_void(name) {
            let Some(open_end) = find_tag_end(body, index) else {
                return Err(ScanError::Unbalanced {
                    context: "element start tag",
                });
            };
            if !body[index..=open_end].trim_end().ends_with("/>") {
                depth += 1;
            }
            index = open_end + 1;
            continue;
        }
        index += 1;
    }
    Ok(None)
}

/// Parses `key="value"` pairs from the inside of a start tag.
fn parse_attributes(source: &str) -> Vec<(String, String)> {
    let mut attrs = Vec::new();
    let bytes = source.as_bytes();
    let mut index = 0usize;
    while index < bytes.len() {
        if !source.is_char_boundary(index) {
            index += 1;
            continue;
        }
        let byte = bytes[index];
        if byte.is_ascii_whitespace() || byte == b'/' {
            index += 1;
            continue;
        }
        let name_start = index;
        while index < bytes.len()
            && !bytes[index].is_ascii_whitespace()
            && !matches!(bytes[index], b'=' | b'/' | b'>')
        {
            index += 1;
        }
        if index == name_start {
            index += 1;
            continue;
        }
        let key = source[name_start..index].to_owned();
        let mut cursor = index;
        while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        if cursor >= bytes.len() || bytes[cursor] != b'=' {
            attrs.push((key, String::new()));
            continue;
        }
        cursor += 1;
        while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        if cursor >= bytes.len() {
            attrs.push((key, String::new()));
            break;
        }
        let value = match bytes[cursor] {
            quote @ (b'"' | b'\'') => {
                let value_start = cursor + 1;
                let mut end = value_start;
                while end < bytes.len() && bytes[end] != quote {
                    end += 1;
                }
                let value = source[value_start..end].to_owned();
                cursor = (end + 1).min(bytes.len());
                value
            }
            _ => {
                let value_start = cursor;
                while cursor < bytes.len()
                    && !bytes[cursor].is_ascii_whitespace()
                    && !matches!(bytes[cursor], b'/' | b'>')
                {
                    cursor += 1;
                }
                source[value_start..cursor].to_owned()
            }
        };
        attrs.push((key, value));
        index = cursor;
    }
    attrs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn class_and_id_lookup_are_scoped_to_the_named_tag() {
        let body = r#"<div class="p-fbox"><strong>Book</strong></div>
            <table class="myTable"><tr><td>1</td></tr></table>"#;

        let divs = scan_with_class(body, "div", "p-fbox").unwrap();
        assert_eq!(divs.len(), 1);
        assert_eq!(divs[0].text(), "Book");

        // The same class on another tag must not match.
        assert!(scan_with_class(body, "span", "p-fbox").unwrap().is_empty());

        let tables = scan_with_class(body, "table", "myTable").unwrap();
        assert_eq!(tables.len(), 1);
        assert_eq!(direct_children(tables[0].inner(), "tr").unwrap().len(), 1);
    }

    #[test]
    fn nested_same_tag_does_not_terminate_the_outer_element() {
        let body = "<table class=\"outer\"><tr><td><table class=\"inner\"><tr><td>x</td></tr></table></td></tr></table>";
        let outer = scan_with_class(body, "table", "outer").unwrap();
        assert_eq!(outer.len(), 1);
        let inner = scan_with_class(outer[0].inner(), "table", "inner").unwrap();
        assert_eq!(inner.len(), 1);
        // The outer row still has exactly one direct cell.
        assert_eq!(direct_children(&outer[0].inner(), "tr").unwrap().len(), 1);
    }

    #[test]
    fn scan_survives_a_stray_attribute_quote_before_the_wanted_element() {
        // The deployed identity page ends its device-trust banner with
        // `style="color:#8b0000;""`.  Under HTML5 that second quotation mark
        // starts another attribute name, and everything after it is ordinary
        // markup.  A scanner that treats any quote as opening a value never
        // reaches the next `>` again and silently loses every later element.
        // The element that actually disappears in production is the one whose
        // start tag comes *after* the stray quote: the scan for the surrounding
        // container still calls `find_tag_end` before the quote, and the search
        // then runs past the container's own `>` to the next quotation mark it
        // can find, which belongs to a tag further down the page.
        let body = r#"<div class="p-fbox">
            <span style="color:#8b0000;"">为了您的账号安全</span>
            <form id="theform" method="post" action="/do/off/ui/auth/login/check">
                <strong class="wanted">Book</strong>
            </form>
        </div>"#;
        let containers = scan(body, "div").unwrap();
        assert_eq!(containers.len(), 1);
        assert_eq!(containers[0].attr("class"), Some("p-fbox"));
        let text = containers[0].text();
        assert!(
            text.contains("为了您的账号安全"),
            "banner text lost: {text:?}"
        );
        assert!(text.contains("Book"), "form content lost: {text:?}");
        let forms = scan_with_id(containers[0].inner(), "form", "theform").unwrap();
        assert_eq!(forms.len(), 1, "the login form must survive the scan");
        assert_eq!(forms[0].attr("action"), Some("/do/off/ui/auth/login/check"));
    }

    #[test]
    fn scan_still_honours_a_quoted_attribute_containing_a_greater_than() {
        // The relaxed delimiter rule must not shorten a real attribute value:
        // a `>` inside quotes is not the end of the tag.
        let body = r#"<div title="a > b"><strong class="wanted">Book</strong></div>"#;
        let found = scan_with_class(body, "strong", "wanted").unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].text(), "Book");
        assert_eq!(scan(body, "div").unwrap()[0].attr("title"), Some("a > b"));
    }

    #[test]
    fn text_extraction_drops_script_style_and_decodes_entities() {
        let body = "<td>方案内实际完成 <script>var x = 1;</script>总学分：<strong>168.5</strong>&nbsp;分</td>";
        let cells = scan(body, "td").unwrap();
        assert_eq!(cells.len(), 1);
        let text = cells[0].text();
        assert!(text.contains("总学分：168.5"), "unexpected text: {text}");
        assert!(!text.contains("var x"));
    }

    #[test]
    fn block_tags_separate_tokens_but_inline_tags_do_not() {
        assert_eq!(element_text("<b>a</b><b>b</b>"), "ab");
        assert_eq!(element_text("<td>a</td><td>b</td>"), "a b");
        assert_eq!(element_text("a<br>b"), "a b");
    }

    #[test]
    fn classify_page_separates_login_and_expiry_from_business_pages() {
        assert_eq!(
            classify_page("<title>清华大学WebVPN</title>"),
            PageClass::Login
        );
        assert_eq!(classify_page("<div>您即将登录</div>"), PageClass::Login);
        assert_eq!(
            classify_page("time out用户登陆超时或访问内容不存在。请重试"),
            PageClass::Expired
        );
        assert_eq!(
            classify_page("<table class=\"myTable\"></table>"),
            PageClass::Unknown
        );
    }

    #[test]
    fn unclosed_element_is_reported_instead_of_silently_truncated() {
        assert_eq!(
            direct_children("<tr><td>x</td>", "tr").unwrap_err(),
            ScanError::Unbalanced {
                context: "element close tag"
            }
        );
        assert_eq!(
            scan("<div>half", "div").unwrap_err(),
            ScanError::Unbalanced {
                context: "element close tag"
            }
        );
    }

    #[test]
    fn attributes_without_values_and_single_quotes_are_both_read() {
        let body = r#"<input type=hidden name='x' value="1" checked>"#;
        let inputs = scan(body, "input").unwrap();
        assert_eq!(inputs.len(), 1);
        assert_eq!(inputs[0].attr("type"), Some("hidden"));
        assert_eq!(inputs[0].attr("name"), Some("x"));
        assert_eq!(inputs[0].attr("value"), Some("1"));
        assert_eq!(inputs[0].attr("checked"), Some(""));
    }

    #[test]
    fn comments_are_not_scanned_as_elements() {
        let body = "<!-- <td>fake</td> --><td>real</td>";
        let cells = scan(body, "td").unwrap();
        assert_eq!(cells.len(), 1);
        assert_eq!(cells[0].text(), "real");
    }
}
