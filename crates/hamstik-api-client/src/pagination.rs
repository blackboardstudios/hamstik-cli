// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! Cursor pagination primitives.
//!
//! Cursors are treated as fully opaque strings: the client never inspects or
//! constructs them. [`follow_all`] drives a caller-supplied page fetcher until
//! the server reports no more pages, subject to a client-side budget
//! ([`MAX_FOLLOW_PAGES`] / [`MAX_FOLLOW_ITEMS`]) so a hostile server cannot
//! spin forever or exhaust memory.

use std::future::Future;

use serde::{Deserialize, Deserializer, Serialize};

use crate::error::ClientError;

/// The most pages a single `--all` aggregation will fetch.
///
/// With the smallest useful page size this still covers far more than any
/// realistic collection; the cap exists so `hasMore: true` forever terminates
/// with an error instead of an unbounded request loop.
pub const MAX_FOLLOW_PAGES: usize = 1_000;

/// The most items a single `--all` aggregation will accumulate.
pub const MAX_FOLLOW_ITEMS: usize = 50_000;

/// A `page` object returned alongside every Public API collection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Page {
    /// The page size the server used for this response.
    pub limit: i64,
    /// Whether more pages exist after this one.
    #[serde(rename = "hasMore")]
    pub has_more: bool,
    /// The opaque cursor for the next page, when `hasMore` is true.
    #[serde(
        rename = "nextCursor",
        deserialize_with = "deserialize_required_nullable"
    )]
    pub next_cursor: Option<String>,
}

fn deserialize_required_nullable<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    Option::<String>::deserialize(deserializer)
}

/// A single page of decoded items plus their raw JSON, used while aggregating.
#[derive(Debug, Clone)]
pub struct PageItems<T> {
    /// The decoded items of this page.
    pub items: Vec<T>,
    /// The same items as raw JSON, for `--json` fidelity.
    pub raw_items: Vec<serde_json::Value>,
    /// The page metadata that came with the response.
    pub page: Page,
}

impl<T> PageItems<T> {
    /// Builds a page result from a decoded item vector and the raw collection
    /// `serde_json::Value` it came from.
    pub fn new(items: Vec<T>, raw: &serde_json::Value, page: Page) -> Self {
        let raw_items = raw
            .get("items")
            .and_then(serde_json::Value::as_array)
            .cloned()
            .unwrap_or_default();
        Self {
            items,
            raw_items,
            page,
        }
    }
}

/// Follows every page using `fetch`, concatenating items and raw items.
///
/// `fetch` is called with `None` for the first page and then the previous page's
/// `next_cursor` until `has_more` is false or a cursor is absent. Aggregation
/// stops with an error at [`MAX_FOLLOW_PAGES`] pages or [`MAX_FOLLOW_ITEMS`]
/// items, whichever comes first.
pub async fn follow_all<T, F, Fut>(mut fetch: F) -> Result<PageItems<T>, ClientError>
where
    F: FnMut(Option<String>) -> Fut,
    Fut: Future<Output = Result<PageItems<T>, ClientError>>,
{
    let mut items: Vec<T> = Vec::new();
    let mut raw_items: Vec<serde_json::Value> = Vec::new();
    let mut cursor: Option<String> = None;
    let mut last_page: Page;
    let mut pages = 0usize;

    loop {
        if pages >= MAX_FOLLOW_PAGES {
            return Err(ClientError::Protocol(format!(
                "pagination exceeded the {MAX_FOLLOW_PAGES}-page limit; narrow the query instead of using --all"
            )));
        }
        let page = fetch(cursor).await?;
        pages += 1;
        let PageItems {
            items: page_items,
            raw_items: page_raw,
            page: page_meta,
        } = page;
        let has_more = page_meta.has_more;
        let next = page_meta.next_cursor.clone();
        if items.len() + page_items.len() > MAX_FOLLOW_ITEMS {
            return Err(ClientError::Protocol(format!(
                "pagination exceeded the {MAX_FOLLOW_ITEMS}-item limit; narrow the query instead of using --all"
            )));
        }
        items.extend(page_items);
        raw_items.extend(page_raw);
        last_page = page_meta;
        match (has_more, next) {
            (true, Some(next_cursor)) if !next_cursor.is_empty() => {
                cursor = Some(next_cursor);
            }
            _ => break,
        }
    }

    Ok(PageItems {
        items,
        raw_items,
        page: last_page,
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn page(has_more: bool, next: Option<&str>) -> Page {
        Page {
            limit: 2,
            has_more,
            next_cursor: next.map(str::to_string),
        }
    }

    #[tokio::test]
    async fn follows_multiple_pages() {
        let mut calls = 0;
        let result = follow_all(|cursor| {
            calls += 1;
            async move {
                let (items, next) = match cursor.as_deref() {
                    None => (vec![1, 2], Some("c1".to_string())),
                    Some("c1") => (vec![3, 4], Some("c2".to_string())),
                    _ => (vec![5], None),
                };
                let raw = serde_json::json!({ "items": items });
                let has_more = next.is_some();
                Ok(PageItems::new(items, &raw, page(has_more, next.as_deref())))
            }
        })
        .await
        .unwrap();
        assert_eq!(calls, 3);
        assert_eq!(result.items, vec![1, 2, 3, 4, 5]);
        assert!(!result.page.has_more);
    }

    #[tokio::test]
    async fn single_page_stops_immediately() {
        let result = follow_all(|cursor| {
            assert!(cursor.is_none());
            async move {
                let items = vec![7];
                let raw = serde_json::json!({ "items": items });
                Ok(PageItems::new(items, &raw, page(false, None)))
            }
        })
        .await
        .unwrap();
        assert_eq!(result.items, vec![7]);
    }

    #[tokio::test]
    async fn stops_when_has_more_without_cursor() {
        let mut calls = 0;
        let result = follow_all(|_cursor| {
            calls += 1;
            async move {
                let items = vec![1];
                let raw = serde_json::json!({ "items": items });
                // has_more true but null cursor must not loop forever.
                Ok(PageItems::new(items, &raw, page(true, None)))
            }
        })
        .await
        .unwrap();
        assert_eq!(calls, 1);
        assert_eq!(result.items, vec![1]);
    }

    #[test]
    fn page_deserializes_camel_case() {
        let page: Page =
            serde_json::from_str(r#"{"limit":50,"hasMore":true,"nextCursor":"abc"}"#).unwrap();
        assert!(page.has_more);
        assert_eq!(page.next_cursor.as_deref(), Some("abc"));
    }

    #[tokio::test]
    async fn stops_at_the_page_budget_when_the_server_never_stops() {
        let result = follow_all(|_cursor| {
            // Every page claims there is more: the budget must break the loop.
            async move {
                let raw = serde_json::json!({ "items": [1] });
                Ok(PageItems::new(vec![1], &raw, page(true, Some("c"))))
            }
        })
        .await;
        assert!(result.is_err(), "unbounded pagination must fail");
    }

    #[tokio::test]
    async fn stops_at_the_item_budget() {
        // Two oversized pages: the second would push the running total past
        // MAX_FOLLOW_ITEMS, so the aggregation must fail rather than keep all
        // of it in memory.
        let items = vec![0u8; MAX_FOLLOW_ITEMS];
        let big: Vec<Vec<u8>> = vec![items.clone(), items];
        let pages = std::cell::RefCell::new(big.into_iter());
        let result = follow_all(|_cursor| {
            let items = pages.borrow_mut().next().unwrap_or_default();
            async move {
                let raw = serde_json::json!({ "items": [] });
                Ok(PageItems::new(items, &raw, page(true, Some("c"))))
            }
        })
        .await;
        assert!(result.is_err(), "item budget must fail the aggregation");
    }
}
