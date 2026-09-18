// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! Deterministic ordering for Work Item collections (`--sort KEY[:DIR]`).
//!
//! The Public API `sort` parameter selects a primary key but carries no
//! direction and does not promise a tie-break, so two identical queries can
//! return tied items in different orders. This module turns the server order
//! into a total order on the *bounded result the CLI actually fetched*:
//!
//! * no direction (`--sort updated`) keeps the server ordering and only
//!   stabilizes ties, so nothing a working pipeline depends on changes;
//! * `:asc` / `:desc` orders by the requested key, with items missing the
//!   sort value kept last in both directions, matching how the documented keys
//!   are described (for example "earliest due date first, undated last");
//! * ties always break on `key`, then `id`, which is what makes the result
//!   byte-stable across runs.
//!
//! `rank` is a server-computed composite with no comparable field, so
//! `--sort rank:desc` reverses the fetched pages instead of re-deriving rank
//! client-side. Reordering is never attempted against items the CLI did not
//! fetch: it is applied to the bounded result only.

use std::cmp::Ordering;

use serde_json::Value;

use crate::args::{SortArg, SortDirArg, SortKeyArg};

/// Reorders the `items` array of a collection payload in place.
///
/// Payloads without an `items` array (or with fewer than two items) are
/// returned untouched.
pub(crate) fn apply_sort(payload: &mut Value, sort: SortArg) {
    let Some(items) = payload
        .get_mut("items")
        .and_then(|items| items.as_array_mut())
    else {
        return;
    };
    if items.len() < 2 {
        return;
    }

    match (sort.key(), sort.dir()) {
        // Rank has no client-side representation: reverse the server order.
        (SortKeyArg::Rank, Some(_)) => {
            items.reverse();
        }
        (SortKeyArg::Rank, None) => {}
        (key, None) => {
            items.sort_by(|left, right| {
                if primary(key, left) == primary(key, right) {
                    tie_break(left, right)
                } else {
                    Ordering::Equal
                }
            });
        }
        (key, Some(dir)) => {
            items.sort_by(
                |left, right| match (primary(key, left), primary(key, right)) {
                    (None, None) => tie_break(left, right),
                    // Missing sort values stay last whichever way the key runs.
                    (None, Some(_)) => Ordering::Greater,
                    (Some(_), None) => Ordering::Less,
                    (Some(left), Some(right)) => match dir {
                        SortDirArg::Asc => left.cmp(&right),
                        SortDirArg::Desc => right.cmp(&left),
                    },
                },
            );
        }
    }
}

/// The comparable representation of an item's value for `key`, or `None` when
/// the item has no value for it.
fn primary(key: SortKeyArg, item: &Value) -> Option<String> {
    match key {
        SortKeyArg::Updated => text(item, "updatedAt"),
        SortKeyArg::DueDate => text(item, "dueDate"),
        SortKeyArg::Priority => item
            .get("priority")
            .and_then(Value::as_str)
            .and_then(priority_rank),
        SortKeyArg::Rank => None,
    }
}

/// Priorities ordered urgent → low so a plain string compare sorts correctly.
fn priority_rank(priority: &str) -> Option<String> {
    let rank = match priority {
        "urgent" => "0",
        "high" => "1",
        "medium" => "2",
        "low" => "3",
        _ => return None,
    };
    Some(rank.to_string())
}

/// Total-order tie-break: Work Item key, then id, then the empty string.
fn tie_break(left: &Value, right: &Value) -> Ordering {
    text(left, "key")
        .unwrap_or_default()
        .cmp(&text(right, "key").unwrap_or_default())
        .then_with(|| {
            text(left, "id")
                .unwrap_or_default()
                .cmp(&text(right, "id").unwrap_or_default())
        })
}

fn text(item: &Value, field: &str) -> Option<String> {
    item.get(field)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use serde_json::json;

    fn keys(payload: &Value) -> Vec<String> {
        payload["items"]
            .as_array()
            .expect("items array")
            .iter()
            .map(|item| item["key"].as_str().unwrap_or_default().to_string())
            .collect()
    }

    #[test]
    fn ties_are_broken_deterministically() {
        // Same updatedAt: the order must be the item key, whatever the server
        // returned first.
        for input in [["HAM-3", "HAM-1", "HAM-2"], ["HAM-2", "HAM-3", "HAM-1"]] {
            let mut payload = json!({
                "items": input.iter().map(|k| json!({ "key": k, "updatedAt": "2026-01-01T00:00:00Z" })).collect::<Vec<_>>()
            });
            apply_sort(&mut payload, SortArg::parse("updated").unwrap());
            assert_eq!(keys(&payload), vec!["HAM-1", "HAM-2", "HAM-3"]);
        }
    }

    #[test]
    fn distinct_values_keep_the_server_order_without_a_direction() {
        let mut payload = json!({
            "items": [
                { "key": "HAM-9", "updatedAt": "2026-03-01T00:00:00Z" },
                { "key": "HAM-1", "updatedAt": "2026-02-01T00:00:00Z" },
                { "key": "HAM-5", "updatedAt": "2026-01-01T00:00:00Z" }
            ]
        });
        apply_sort(&mut payload, SortArg::parse("updated").unwrap());
        assert_eq!(keys(&payload), vec!["HAM-9", "HAM-1", "HAM-5"]);
    }

    #[test]
    fn direction_orders_the_key_and_stabilizes_ties() {
        let build = |due: [Option<&str>; 3]| {
            let mut payload = json!({
                "items": [
                    { "key": "HAM-2", "dueDate": due[0] },
                    { "key": "HAM-1", "dueDate": due[1] },
                    { "key": "HAM-3", "dueDate": due[2] }
                ]
            });
            apply_sort(&mut payload, SortArg::parse("dueDate:desc").unwrap());
            keys(&payload)
        };
        assert_eq!(
            build([Some("2026-01-01"), Some("2026-03-01"), Some("2026-02-01")]),
            vec!["HAM-1", "HAM-3", "HAM-2"]
        );
        // Undated sorts last even when descending.
        assert_eq!(
            build([Some("2026-01-01"), None, Some("2026-02-01")]),
            vec!["HAM-3", "HAM-2", "HAM-1"]
        );
    }

    #[test]
    fn priority_orders_urgent_first_and_desc_reverses_it() {
        let build = |dir: &str| {
            let mut payload = json!({
                "items": [
                    { "key": "HAM-1", "priority": "low" },
                    { "key": "HAM-2", "priority": "urgent" },
                    { "key": "HAM-3", "priority": "medium" }
                ]
            });
            apply_sort(
                &mut payload,
                SortArg::parse(&format!("priority:{dir}")).unwrap(),
            );
            keys(&payload)
        };
        assert_eq!(build("asc"), vec!["HAM-2", "HAM-3", "HAM-1"]);
        assert_eq!(build("desc"), vec!["HAM-1", "HAM-3", "HAM-2"]);
    }

    #[test]
    fn rank_reverses_the_server_order() {
        let mut payload = json!({ "items": [{ "key": "A" }, { "key": "B" }, { "key": "C" }] });
        apply_sort(&mut payload, SortArg::parse("rank:desc").unwrap());
        assert_eq!(keys(&payload), vec!["C", "B", "A"]);
    }

    #[test]
    fn non_collection_payloads_are_untouched() {
        let mut payload = json!({ "key": "HAM-1" });
        apply_sort(&mut payload, SortArg::parse("updated:desc").unwrap());
        assert_eq!(payload, json!({ "key": "HAM-1" }));
    }

    #[test]
    fn sort_specs_are_validated() {
        assert!(SortArg::parse("updated").is_ok());
        assert!(SortArg::parse("dueDate:asc").is_ok());
        assert!(SortArg::parse("rank:desc").is_ok());
        assert!(SortArg::parse("updated:sideways").is_err());
        assert!(SortArg::parse("nope").is_err());
        assert!(SortArg::parse("nope:desc").is_err());
        assert_eq!(
            SortArg::parse("dueDate:desc").expect("parses").as_str(),
            "dueDate"
        );
    }
}
