//! Strict read-only INFO subscription rules and favorite pages. The reference
//! uses one fixed subscription route and a paged favorite route; neither
//! parser treats a failed or unfamiliar response as an empty collection.
use super::*;
use std::collections::HashSet;

const MAX_PERSONAL_BODY_BYTES: usize = 2 * 1024 * 1024;
const MAX_SUBSCRIPTIONS: usize = 1000;
pub const MAX_FAVORITE_PAGES: u32 = 100;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewsSubscriptionRule {
    /// Server identity remains in Rust; the Flutter bridge exposes only text.
    pub id: String,
    pub title: String,
    pub order: i64,
    pub sources: Vec<String>,
    pub channels: Vec<String>,
    pub keyword: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewsFavoritePage {
    pub items: Vec<NewsItem>,
    pub total_pages: u32,
}

fn invalid(kind: &'static str) -> NewsParseError {
    NewsParseError::CatalogMalformedPayload { kind }
}

fn personal_root(body: &str, kind: &'static str) -> Result<Value, NewsParseError> {
    match classify_html(body) {
        Some(NewsHtmlClassification::LoginPage) => return Err(NewsParseError::HtmlLoginPage),
        Some(NewsHtmlClassification::OtherHtml) => return Err(NewsParseError::HtmlPage),
        None => {}
    }
    if body.len() > MAX_PERSONAL_BODY_BYTES {
        return Err(invalid(kind));
    }
    let value: Value =
        serde_json::from_str(body.trim_start_matches('\u{feff}')).map_err(|_| invalid(kind))?;
    let root = value.as_object().ok_or_else(|| invalid(kind))?;
    match inspect_news_envelope(root) {
        Ok(()) => {}
        Err(NewsEnvelopeError::LoginRequired) => return Err(NewsParseError::LoginRequired),
        Err(NewsEnvelopeError::BusinessFailure { message }) => {
            return Err(NewsParseError::BusinessFailure { message });
        }
        Err(NewsEnvelopeError::Malformed(_)) => return Err(invalid(kind)),
    }
    root.get("object").cloned().ok_or_else(|| invalid(kind))
}

fn text(value: &Value, kind: &'static str) -> Result<String, NewsParseError> {
    let raw = value.as_str().ok_or_else(|| invalid(kind))?;
    let clean = normalize_plain_text(raw, kind).map_err(|_| invalid(kind))?;
    if clean.len() > 512 || clean.contains(['<', '>']) {
        return Err(invalid(kind));
    }
    Ok(clean)
}

fn optional_labels(row: &Map<String, Value>, field: &str) -> Result<Vec<String>, NewsParseError> {
    const KIND: &str = "subscriptions";
    let Some(value) = row.get(field) else {
        return Ok(Vec::new());
    };
    let labels = value.as_array().ok_or_else(|| invalid(KIND))?;
    if labels.len() > 100 {
        return Err(invalid(KIND));
    }
    labels.iter().map(|value| text(value, KIND)).collect()
}

/// Reference shape: `object: [{id, titile, pxz, fbdwmcList, lmmcList, bt}]`.
/// The upstream spelling `titile` is preserved at the parsing boundary.
pub fn parse_news_subscriptions(body: &str) -> Result<Vec<NewsSubscriptionRule>, NewsParseError> {
    const KIND: &str = "subscriptions";
    let object = personal_root(body, KIND)?;
    let rows = object.as_array().ok_or_else(|| invalid(KIND))?;
    if rows.len() > MAX_SUBSCRIPTIONS {
        return Err(invalid(KIND));
    }
    let mut seen = HashSet::new();
    let mut rules = Vec::with_capacity(rows.len());
    for value in rows {
        let row = value.as_object().ok_or_else(|| invalid(KIND))?;
        let id = row
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid(KIND))?;
        let id = normalize_identifier(id, "id").map_err(|_| invalid(KIND))?;
        if id.len() > 128
            || id.chars().any(|character| {
                character.is_whitespace() || matches!(character, '/' | '\\' | '?' | '#' | '&' | '=')
            })
        {
            return Err(invalid(KIND));
        }
        if !seen.insert(id.clone()) {
            return Err(invalid(KIND));
        }
        let title = text(row.get("titile").ok_or_else(|| invalid(KIND))?, KIND)?;
        let order = row
            .get("pxz")
            .and_then(Value::as_i64)
            .filter(|value| (0..=1_000_000).contains(value))
            .ok_or_else(|| invalid(KIND))?;
        let sources = optional_labels(row, "fbdwmcList")?;
        let channels = optional_labels(row, "lmmcList")?;
        let keyword = match row.get("bt") {
            None | Some(Value::Null) => String::new(),
            Some(Value::String(value)) if value.trim().is_empty() => String::new(),
            Some(value) => text(value, KIND)?,
        };
        rules.push(NewsSubscriptionRule {
            id,
            title,
            order,
            sources,
            channels,
            keyword,
        });
    }
    rules.sort_by(|left, right| left.order.cmp(&right.order).then(left.id.cmp(&right.id)));
    Ok(rules)
}

/// Reference shape: `object: {totalPages, resultList}`. Each page must carry
/// a total so the caller can prove that all pages were read.
pub fn parse_news_favorite_page(body: &str) -> Result<NewsFavoritePage, NewsParseError> {
    const KIND: &str = "favorites";
    let object = personal_root(body, KIND)?;
    let total_pages = object
        .as_object()
        .and_then(|object| object.get("totalPages"))
        .and_then(Value::as_u64)
        .filter(|pages| *pages <= u64::from(MAX_FAVORITE_PAGES))
        .ok_or_else(|| invalid(KIND))? as u32;
    let rows = object
        .as_object()
        .and_then(|object| object.get("resultList"))
        .and_then(Value::as_array)
        .ok_or_else(|| invalid(KIND))?;
    if rows.len() > 5_000 {
        return Err(invalid(KIND));
    }
    let items = rows
        .iter()
        .enumerate()
        .map(|(index, row)| parse_news_item(row, index, NewsFeedKind::Favorite))
        .collect::<Result<Vec<_>, _>>()
        .map_err(malformed)?;
    if total_pages == 0 && !items.is_empty() {
        return Err(invalid(KIND));
    }
    if items.iter().any(|item| !item.favorited) {
        return Err(invalid(KIND));
    }
    Ok(NewsFavoritePage { items, total_pages })
}

/// Reference shape: `object.resultList` for one selected rule and page.
/// This endpoint has no documented total page count, so an empty response is
/// only the end of this individual page request, not proof of a full archive.
pub fn parse_news_subscription_page(body: &str) -> Result<NewsPage, NewsParseError> {
    const KIND: &str = "subscription_page";
    let object = personal_root(body, KIND)?;
    let rows = object
        .as_object()
        .and_then(|object| object.get("resultList"))
        .and_then(Value::as_array)
        .ok_or_else(|| invalid(KIND))?;
    if rows.len() > 500 {
        return Err(invalid(KIND));
    }
    let items = rows
        .iter()
        .enumerate()
        .map(|(index, row)| parse_news_item(row, index, NewsFeedKind::Subscription))
        .collect::<Result<Vec<_>, _>>()
        .map_err(malformed)?;
    if items
        .iter()
        .map(|item| &item.id)
        .collect::<HashSet<_>>()
        .len()
        != items.len()
    {
        return Err(invalid(KIND));
    }
    Ok(NewsPage {
        feed: NewsFeedKind::Subscription,
        items,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_repair_info_personal_parsers_accept_real_shapes_and_reject_false_empty() {
        let rules = parse_news_subscriptions(
            r#"{"result":"success","object":[{"id":"rule_1","titile":"我的通知","pxz":2,"fbdwmcList":["教务处"],"lmmcList":["教务通知"],"bt":null}]}"#,
        )
        .unwrap();
        assert_eq!(rules[0].sources, ["教务处"]);
        assert_eq!(rules[0].keyword, "");
        let page = parse_news_favorite_page(
            r#"{"object":{"totalPages":1,"resultList":[{"bt":"通知","url":"/article/1","xxid":"article_1","time":"2026-09-24","dwmc_show":"教务处","lmid":"LM_JWGG","sfsc":true}]}}"#,
        )
        .unwrap();
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.total_pages, 1);
        for body in [
            r#"{"result":"error","object":[]}"#,
            r#"{"object":{}}"#,
            r#"{"object":[{"id":"same","titile":"A","pxz":1},{"id":"same","titile":"B","pxz":2}]}"#,
            "<html><form><input name='password'></form></html>",
        ] {
            assert!(parse_news_subscriptions(body).is_err());
        }
        for body in [
            r#"{"object":{"resultList":[]}}"#,
            r#"{"object":{"totalPages":0,"resultList":[{"bt":"A"}]}}"#,
            r#"{"result":"error","object":{"totalPages":0,"resultList":[]}}"#,
        ] {
            assert!(parse_news_favorite_page(body).is_err());
        }
    }

    #[test]
    fn backend_repair_info_subscription_feed_requires_result_list_and_real_items() {
        let page = parse_news_subscription_page(
            r#"{"object":{"resultList":[{"bt":"通知","url":"/article/1","xxid":"article_1","time":"2026-09-24","dwmc_show":"教务处","lmid":"LM_JWGG","sfsc":false}]}}"#,
        ).unwrap();
        assert_eq!(page.feed, NewsFeedKind::Subscription);
        assert_eq!(page.items.len(), 1);
        assert!(!page.items[0].topped);
        assert!(
            parse_news_subscription_page(r#"{"object":{"resultList":[]}}"#)
                .unwrap()
                .is_empty()
        );
        for body in [
            r#"{"object":{}}"#,
            r#"{"result":"error","object":{"resultList":[]}}"#,
            "<html><form><input name='password'></form></html>",
        ] {
            assert!(parse_news_subscription_page(body).is_err());
        }
    }
}
