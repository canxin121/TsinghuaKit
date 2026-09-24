//! Strict, read-only INFO news filter catalogs from the pinned THU Info routes.
//! IDs and labels are data, never navigation URLs or authentication material.
use super::*;
use std::collections::HashSet;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewsSourceOption {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewsChannelOption {
    pub id: String,
    pub title: String,
}

const MAX_OPTIONS: usize = 1000;

fn malformed(kind: &'static str) -> NewsParseError {
    NewsParseError::CatalogMalformedPayload { kind }
}

fn catalog_object(body: &str, kind: &'static str) -> Result<Value, NewsParseError> {
    match classify_html(body) {
        Some(NewsHtmlClassification::LoginPage) => return Err(NewsParseError::HtmlLoginPage),
        Some(NewsHtmlClassification::OtherHtml) => return Err(NewsParseError::HtmlPage),
        None => {}
    }
    if body.len() > 2 * 1024 * 1024 {
        return Err(malformed(kind));
    }
    let root: Value =
        serde_json::from_str(body.trim_start_matches('\u{feff}')).map_err(|_| malformed(kind))?;
    let object = root.as_object().ok_or_else(|| malformed(kind))?;
    match inspect_news_envelope(object) {
        Ok(()) => {}
        Err(NewsEnvelopeError::LoginRequired) => return Err(NewsParseError::LoginRequired),
        Err(NewsEnvelopeError::BusinessFailure { message }) => {
            return Err(NewsParseError::BusinessFailure { message });
        }
        Err(NewsEnvelopeError::Malformed(_)) => return Err(malformed(kind)),
    }
    object.get("object").cloned().ok_or_else(|| malformed(kind))
}

fn option_id(value: &Value, kind: &'static str) -> Result<String, NewsParseError> {
    let id = value.as_str().ok_or_else(|| malformed(kind))?.trim();
    if !safe_option_id(id) {
        return Err(malformed(kind));
    }
    Ok(id.to_owned())
}

fn safe_option_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && !id.chars().any(|character| {
            character.is_control()
                || character.is_whitespace()
                || matches!(character, '/' | '\\' | '?' | '#' | '&' | '=')
        })
}

fn option_label(value: &Value, kind: &'static str) -> Result<String, NewsParseError> {
    let raw = value.as_str().ok_or_else(|| malformed(kind))?.trim();
    if raw.len() > 512 || raw.contains(['<', '>']) {
        return Err(malformed(kind));
    }
    let label = decode_html_entities_only(raw)
        .map(|value| collapse_whitespace(&value))
        .map_err(|_| malformed(kind))?;
    if label.is_empty()
        || label.len() > 256
        || label.chars().any(|character| character.is_control())
    {
        return Err(malformed(kind));
    }
    Ok(label)
}

/// Reference shape: `object: [{id, text}]` from the INFO subscription-unit API.
pub fn parse_news_sources(body: &str) -> Result<Vec<NewsSourceOption>, NewsParseError> {
    const KIND: &str = "sources";
    let object = catalog_object(body, KIND)?;
    let rows = object.as_array().ok_or_else(|| malformed(KIND))?;
    if rows.len() > MAX_OPTIONS {
        return Err(malformed(KIND));
    }
    let mut seen = HashSet::new();
    let mut options = Vec::with_capacity(rows.len());
    for row in rows {
        let row = row.as_object().ok_or_else(|| malformed(KIND))?;
        let id = option_id(row.get("id").ok_or_else(|| malformed(KIND))?, KIND)?;
        let name = option_label(row.get("text").ok_or_else(|| malformed(KIND))?, KIND)?;
        if !seen.insert(id.clone()) {
            return Err(malformed(KIND));
        }
        options.push(NewsSourceOption { id, name });
    }
    Ok(options)
}

/// Reference shape: `object.lmlist: [{id, title_zh, title_en}]`.
pub fn parse_news_channels(body: &str) -> Result<Vec<NewsChannelOption>, NewsParseError> {
    const KIND: &str = "channels";
    let object = catalog_object(body, KIND)?;
    let rows = object
        .as_object()
        .and_then(|object| object.get("lmlist"))
        .and_then(Value::as_array)
        .ok_or_else(|| malformed(KIND))?;
    if rows.len() > MAX_OPTIONS {
        return Err(malformed(KIND));
    }
    let mut seen = HashSet::new();
    let mut options = Vec::with_capacity(rows.len());
    for row in rows {
        let row = row.as_object().ok_or_else(|| malformed(KIND))?;
        let id = option_id(row.get("id").ok_or_else(|| malformed(KIND))?, KIND)?;
        let title = option_label(row.get("title_zh").ok_or_else(|| malformed(KIND))?, KIND)?;
        if !seen.insert(id.clone()) {
            return Err(malformed(KIND));
        }
        options.push(NewsChannelOption { id, title });
    }
    Ok(options)
}

/// The retired channel-directory route cannot prove a complete catalog. A
/// successful live news page can still prove the channel IDs actually present
/// on that page. Names are reference taxonomy labels; unknown IDs stay as
/// their server-provided IDs. The caller must keep the result `partial`.
pub(crate) fn channels_observed_on_news_page(page: &NewsPage) -> Vec<NewsChannelOption> {
    let mut seen = HashSet::new();
    page.items
        .iter()
        .filter_map(|item| {
            let id = item.channel.as_str();
            (safe_option_id(id) && seen.insert(id)).then(|| NewsChannelOption {
                id: id.to_owned(),
                title: reference_channel_title(id).unwrap_or(id).to_owned(),
            })
        })
        .collect()
}

fn reference_channel_title(id: &str) -> Option<&'static str> {
    // THU Info's news page groups these fixed IDs. This mapping labels only
    // IDs observed in a successful live response; it never creates options.
    Some(match id {
        "LM_BGTG" => "办公通知",
        "LM_ZYGG" => "重要公告",
        "LM_YQFKZT" => "疫情防控专题",
        "LM_JWGG" => "教务通知",
        "LM_KYTZ" => "科研通知",
        "LM_HB" => "海报",
        "LM_XJ_XTWBGTZ" => "校团委通知",
        "LM_XSBGGG" => "学生工作通知",
        "LM_TTGGG" => "图书馆信息",
        "LM_JYGG" => "学生社区通知",
        "LM_XJ_XSSQDT" => "学生社区动态",
        "LM_BYJYXX" => "就业通知",
        "LM_JYZPXX" => "招聘信息",
        "LM_XJ_GJZZSXRZ" => "国际组织实习任职",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_repair_news_filter_catalogs_keep_server_options_and_reject_false_empty() {
        let sources = parse_news_sources(
            r#"{"result":"success","object":[{"id":"unit_1","text":"图书馆&amp;档案馆"}]}"#,
        )
        .unwrap();
        assert_eq!(sources[0].name, "图书馆&档案馆");
        let channels = parse_news_channels(
            r#"{"object":{"lmlist":[{"id":"LM_BGTG","title_zh":"办公通知","title_en":"Office"}]}}"#,
        )
        .unwrap();
        assert_eq!(channels[0].id, "LM_BGTG");
        for body in [
            r#"{"result":"error","object":[]}"#,
            r#"{"object":{"list":[]}}"#,
            r#"{"object":[{"id":"one","text":"A"},{"id":"one","text":"B"}]}"#,
            "<html><form><input name='password'></form></html>",
        ] {
            assert!(parse_news_sources(body).is_err());
        }
    }

    #[test]
    fn backend_repair_news_observed_channels_never_invent_unseen_options() {
        let item = |channel: &str| NewsItem {
            id: "fixture-article".into(),
            title: "Fixture".into(),
            link: NewsLink("/fixture".into()),
            published_at: "2026-09-24 12:00:00".into(),
            source: "Fixture source".into(),
            topped: false,
            channel: NewsChannelId(channel.into()),
            favorited: false,
        };
        let page = NewsPage {
            feed: NewsFeedKind::List,
            items: vec![
                item("LM_JWGG"),
                item("LM_JWGG"),
                item("LM_NEW"),
                item("bad/channel"),
            ],
        };
        let channels = channels_observed_on_news_page(&page);
        assert_eq!(channels.len(), 2);
        assert_eq!(channels[0].id, "LM_JWGG");
        assert_eq!(channels[0].title, "教务通知");
        assert_eq!(channels[1].id, "LM_NEW");
        assert_eq!(channels[1].title, "LM_NEW");
    }
}
