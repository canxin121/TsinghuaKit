//! Exact selector policies extracted from the pinned THUInfo news.ts.
//! Only DOM reading; no script evaluation, URL fetching or invented content.
use super::*;
use std::sync::OnceLock;
type Policy = (String, [String; 2]);
static POLICIES: OnceLock<Vec<Policy>> = OnceLock::new();
fn policies() -> &'static [Policy] {
    POLICIES.get_or_init(|| {
        serde_json::from_str(include_str!("reference_news_policies.json"))
            .expect("checked source-owned news policies")
    })
}
fn selector(value: &str) -> Result<(scraper::Selector, usize), NewsParseError> {
    let (value, index) = if let Some(value) = value.strip_suffix(":nth(1)") {
        (value, 1)
    } else if let Some(value) = value.strip_suffix(":nth(3)") {
        (value, 3)
    } else {
        (value.strip_suffix(":first").unwrap_or(value), 0)
    };
    let value = value
        .replace("[colspan=4]", "[colspan='4']")
        .replace("[width=95%]", "[width='95%']")
        .replace("[height=40]", "[height='40']");
    scraper::Selector::parse(&value)
        .map(|selector| (selector, index))
        .map_err(|_| detail_malformed("legacy.policy", "unsupported source-owned selector"))
}

pub(crate) fn parse_news_legacy_detail_for_url(
    body: &str,
    url: &str,
) -> Result<NewsDetail, NewsParseError> {
    let mapped_ghxt = Url::parse(url).ok().is_some_and(|target| {
        target.path().split('/').nth(2) == Some(crate::info_news::GHXT_MAPPING_ID)
    });
    let Some((_, selectors)) = policies()
        .iter()
        .find(|(key, _)| (key == "ghxt" && mapped_ghxt) || url.contains(key))
    else {
        return parse_news_legacy_detail(body);
    };
    match classify_html(body) {
        Some(NewsHtmlClassification::LoginPage) => return Err(NewsParseError::HtmlLoginPage),
        Some(NewsHtmlClassification::OtherHtml) => {}
        None => return Err(NewsParseError::HtmlPage),
    }
    if body.len() > 4 * 1024 * 1024 {
        return Err(detail_malformed(
            "legacy.html",
            "article document exceeds limit",
        ));
    }
    let document = scraper::Html::parse_document(body);
    let title = if selectors[0].is_empty() {
        String::new()
    } else {
        let (query, index) = selector(&selectors[0])?;
        let element = document.select(&query).nth(index).ok_or_else(|| {
            detail_malformed("legacy.title", "reference title selector has no match")
        })?;
        collapse_whitespace(&element.text().collect::<String>())
    };
    let (query, index) = selector(&selectors[1])?;
    let element = document.select(&query).nth(index).ok_or_else(|| {
        detail_malformed("legacy.content", "reference body selector has no match")
    })?;
    // The reference selector is evaluated by an HTML5 parser. Render that
    // selected DOM subtree directly instead of scanning its serialized HTML
    // for matching quote/tag delimiters: real legacy pages can contain raw
    // text elements whose serialized children are not a strict tag stream.
    let (content_html, summary) = render_reference_article(element)?;
    validate_detail_html(&content_html)
        .map_err(|reason| detail_malformed("legacy.validation", &reason))?;
    if summary.is_empty() {
        return Err(detail_malformed(
            "legacy.content",
            "reference-selected article body is empty",
        ));
    }
    Ok(NewsDetail {
        id: String::new(),
        title,
        content_html,
        summary,
        attachments: Vec::new(),
    })
}

const ARTICLE_LIMIT: usize = 4 * 1024 * 1024;
const SAFE_ARTICLE_TAGS: &[&str] = &[
    "a",
    "b",
    "blockquote",
    "br",
    "caption",
    "code",
    "em",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "i",
    "li",
    "ol",
    "p",
    "pre",
    "section",
    "strong",
    "sub",
    "sup",
    "table",
    "tbody",
    "td",
    "tfoot",
    "th",
    "thead",
    "tr",
    "u",
    "ul",
];
const SKIP_ARTICLE_TAGS: &[&str] = &[
    "script", "style", "noscript", "iframe", "object", "embed", "form", "template", "textarea",
];

fn render_reference_article(
    element: scraper::ElementRef<'_>,
) -> Result<(String, String), NewsParseError> {
    let mut html = String::new();
    let mut text = String::new();
    render_reference_children(element, &mut html, &mut text, 0)?;
    Ok((html, collapse_whitespace(&text)))
}

fn render_reference_children(
    element: scraper::ElementRef<'_>,
    html: &mut String,
    text: &mut String,
    depth: usize,
) -> Result<(), NewsParseError> {
    if depth > 128 {
        return Err(detail_malformed(
            "legacy.fragment",
            "article nesting exceeds limit",
        ));
    }
    for child in element.children() {
        if let Some(child_element) = scraper::ElementRef::wrap(child) {
            let tag = child_element.value().name();
            if SKIP_ARTICLE_TAGS.contains(&tag) {
                continue;
            }
            let retain = SAFE_ARTICLE_TAGS.contains(&tag);
            if retain {
                html.push('<');
                html.push_str(tag);
                html.push('>');
            }
            render_reference_children(child_element, html, text, depth + 1)?;
            if retain && !is_void_html_tag(tag) {
                html.push_str("</");
                html.push_str(tag);
                html.push('>');
            }
            text.push(' ');
        } else if let scraper::Node::Text(value) = child.value() {
            text.push_str(value);
            for character in value.chars() {
                match character {
                    '&' => html.push_str("&amp;"),
                    '<' => html.push_str("&lt;"),
                    '>' => html.push_str("&gt;"),
                    _ => html.push(character),
                }
            }
        }
        if html.len() > ARTICLE_LIMIT || text.len() > ARTICLE_LIMIT {
            return Err(detail_malformed(
                "legacy.fragment",
                "article fragment exceeds limit",
            ));
        }
    }
    Ok(())
}
