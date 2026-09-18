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

/// How one collection command traverses pages.
///
/// The three knobs map one-to-one onto the documented pipeline ergonomics
/// flags: `--all` ([`FollowPolicy::follow`]), `--limit` ([`FollowPolicy::max_items`],
/// the total result cap, which is *not* the server page size) and
/// `--cursor` / `--since-cursor` ([`FollowPolicy::start_cursor`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FollowPolicy {
    /// Follow `nextCursor` until the server reports no more pages. When false,
    /// exactly one page is fetched.
    pub follow: bool,
    /// Hard cap on the total number of items returned by the command, counted
    /// across pages. `None` means the server decides how much a traversal
    /// yields.
    pub max_items: Option<usize>,
    /// Opaque cursor the traversal starts after. It is handed verbatim to the
    /// first request; the client never inspects or constructs cursors.
    pub start_cursor: Option<String>,
}

impl FollowPolicy {
    /// Fetches a single page (the default first-page behavior).
    #[must_use]
    pub fn single_page() -> Self {
        Self::default()
    }

    /// Follows every page (`--all`).
    #[must_use]
    pub fn all() -> Self {
        Self {
            follow: true,
            ..Self::default()
        }
    }
}

/// Follows every page using `fetch`, concatenating items and raw items.
///
/// Equivalent to [`follow_with`] with [`FollowPolicy::all`] — the traversal
/// starts at the first page and has no client-side result cap.
pub async fn follow_all<T, F, Fut>(fetch: F) -> Result<PageItems<T>, ClientError>
where
    F: FnMut(Option<String>) -> Fut,
    Fut: Future<Output = Result<PageItems<T>, ClientError>>,
{
    follow_with(FollowPolicy::all(), fetch).await
}

/// Traverses pages according to `policy`, concatenating items and raw items.
///
/// The first `fetch` receives [`FollowPolicy::start_cursor`] (or `None`), then
/// each page's `next_cursor`, until `has_more` is false, a cursor is absent,
/// [`FollowPolicy::max_items`] items have been produced, or — when
/// [`FollowPolicy::follow`] is false — one page has been fetched. Aggregation
/// stops with an error at [`MAX_FOLLOW_PAGES`] pages or [`MAX_FOLLOW_ITEMS`]
/// items, whichever comes first, so a hostile or looping server cannot spin
/// forever.
///
/// Resume semantics: a cursor is only ever a server-issued boundary, so a
/// traversal started from one never re-reads or skips items ahead of that
/// boundary. If [`FollowPolicy::max_items`] cuts the traversal through the
/// middle of a server page, the returned `page.next_cursor` is `None`: the
/// Public API has no cursor for a position partway through a page, and
/// reporting the page-end cursor would silently skip the items that were
/// dropped from this result. `page.has_more` still reports `true` in that
/// case, so a consumer can tell a capped result from an exhausted one.
pub async fn follow_with<T, F, Fut>(
    policy: FollowPolicy,
    mut fetch: F,
) -> Result<PageItems<T>, ClientError>
where
    F: FnMut(Option<String>) -> Fut,
    Fut: Future<Output = Result<PageItems<T>, ClientError>>,
{
    let FollowPolicy {
        follow,
        max_items,
        start_cursor,
    } = policy;
    let mut items: Vec<T> = Vec::new();
    let mut raw_items: Vec<serde_json::Value> = Vec::new();
    let mut cursor: Option<String> = start_cursor.filter(|c| !c.is_empty());
    let mut last_page: Option<Page>;
    let mut pages = 0usize;
    let mut trimmed = false;

    loop {
        if pages >= MAX_FOLLOW_PAGES {
            return Err(ClientError::Protocol(format!(
                "pagination exceeded the {MAX_FOLLOW_PAGES}-page limit; narrow the query instead of using --all"
            )));
        }
        let page = fetch(cursor).await?;
        pages += 1;
        let PageItems {
            items: mut page_items,
            raw_items: mut page_raw,
            page: page_meta,
        } = page;
        let has_more = page_meta.has_more;
        let next = page_meta.next_cursor.clone();
        if let Some(max) = max_items {
            let room = max.saturating_sub(items.len());
            if page_items.len() > room {
                page_items.truncate(room);
                trimmed = true;
            }
            if page_raw.len() > room {
                page_raw.truncate(room);
            }
        }
        if items.len() + page_items.len() > MAX_FOLLOW_ITEMS {
            return Err(ClientError::Protocol(format!(
                "pagination exceeded the {MAX_FOLLOW_ITEMS}-item limit; narrow the query instead of using --all"
            )));
        }
        items.extend(page_items);
        raw_items.extend(page_raw);
        let reached_cap = max_items.is_some_and(|max| items.len() >= max);
        last_page = Some(page_meta);
        if reached_cap {
            if trimmed {
                break;
            }
            // The cap landed exactly on a page boundary: `next` remains a valid
            // resume point for whatever comes after this result.
            break;
        }
        if !follow {
            break;
        }
        match (has_more, next) {
            (true, Some(next_cursor)) if !next_cursor.is_empty() => {
                cursor = Some(next_cursor);
            }
            _ => break,
        }
    }

    let mut page = last_page
        .ok_or_else(|| ClientError::Protocol("pagination produced no page".to_string()))?;
    if trimmed {
        page.next_cursor = None;
    }

    Ok(PageItems {
        items,
        raw_items,
        page,
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

    #[tokio::test]
    async fn starts_from_the_policy_start_cursor() {
        let seen = std::cell::RefCell::new(Vec::new());
        let result = follow_with(
            FollowPolicy {
                follow: true,
                start_cursor: Some("resume-1".to_string()),
                ..FollowPolicy::default()
            },
            |cursor| {
                seen.borrow_mut().push(cursor.clone());
                let next = match cursor.as_deref() {
                    Some("resume-1") => Some("resume-2".to_string()),
                    _ => None,
                };
                let has_more = next.is_some();
                async move {
                    let items = vec![cursor.unwrap_or_default()];
                    let raw = serde_json::json!({ "items": items });
                    Ok(PageItems::new(items, &raw, page(has_more, next.as_deref())))
                }
            },
        )
        .await
        .unwrap();
        assert_eq!(
            seen.borrow()[0].as_deref(),
            Some("resume-1"),
            "a resume cursor must be sent on the first request"
        );
        assert_eq!(
            result.items,
            vec!["resume-1".to_string(), "resume-2".to_string()]
        );
    }

    #[tokio::test]
    async fn caps_total_items_and_drops_an_unrepresentable_cursor() {
        let result = follow_with(
            FollowPolicy {
                follow: true,
                max_items: Some(3),
                ..FollowPolicy::default()
            },
            |_cursor| async move {
                let raw = serde_json::json!({ "items": [1, 2] });
                Ok(PageItems::new(vec![1, 2], &raw, page(true, Some("next"))))
            },
        )
        .await
        .unwrap();
        assert_eq!(result.items, vec![1, 2, 1]);
        assert!(
            result.page.next_cursor.is_none(),
            "a mid-page cap must not advertise a resume cursor that skips items"
        );
        assert!(
            result.page.has_more,
            "a capped result must still report that more items exist"
        );
    }

    #[tokio::test]
    async fn keeps_the_resume_cursor_when_the_cap_lands_on_a_page_boundary() {
        let mut calls = 0;
        let result = follow_with(
            FollowPolicy {
                follow: true,
                max_items: Some(4),
                ..FollowPolicy::default()
            },
            |_cursor| {
                calls += 1;
                async move {
                    let raw = serde_json::json!({ "items": [1, 2] });
                    Ok(PageItems::new(vec![1, 2], &raw, page(true, Some("third"))))
                }
            },
        )
        .await
        .unwrap();
        assert_eq!(calls, 2, "the cap must stop the traversal at 4 items");
        assert_eq!(result.items.len(), 4);
        assert_eq!(
            result.page.next_cursor.as_deref(),
            Some("third"),
            "a boundary-aligned cap keeps the server resume cursor"
        );
    }

    #[tokio::test]
    async fn single_page_policy_fetches_one_page() {
        let mut calls = 0;
        let result = follow_with(FollowPolicy::single_page(), |_cursor| {
            calls += 1;
            async move {
                let items = vec![1];
                let raw = serde_json::json!({ "items": items });
                Ok(PageItems::new(items, &raw, page(true, Some("more"))))
            }
        })
        .await
        .unwrap();
        assert_eq!(calls, 1);
        assert_eq!(result.items, vec![1]);
        assert_eq!(result.page.next_cursor.as_deref(), Some("more"));
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
