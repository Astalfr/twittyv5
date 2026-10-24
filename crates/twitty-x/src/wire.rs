//! Lenient readers over Twitter's JSON. Private to this crate: nothing outside
//! `twitty-x` may depend on Twitter's shapes (see `docs/domain-model.md`).
//!
//! Every accessor returns `Option` and tolerates missing fields, nulls and
//! numbers sent as strings, so shape drift degrades output instead of failing.

use serde_json::Value;

/// The value at `path` (object keys only).
pub(crate) fn at<'a>(v: &'a Value, path: &[&str]) -> Option<&'a Value> {
    path.iter().try_fold(v, |v, k| v.get(*k)).filter(|v| !v.is_null())
}

pub(crate) fn str_at<'a>(v: &'a Value, path: &[&str]) -> Option<&'a str> {
    at(v, path)?.as_str()
}

/// A non-negative integer sent as a number or a numeric string.
pub(crate) fn u64_at(v: &Value, path: &[&str]) -> Option<u64> {
    let v = at(v, path)?;
    v.as_u64().or_else(|| v.as_str()?.trim().parse().ok())
}

pub(crate) fn bool_at(v: &Value, path: &[&str]) -> Option<bool> {
    at(v, path)?.as_bool()
}

pub(crate) fn array_at<'a>(v: &'a Value, path: &[&str]) -> &'a [Value] {
    at(v, path).and_then(Value::as_array).map_or(&[], Vec::as_slice)
}

/// The first `instructions` array found depth-first under `root`.
pub(crate) fn find_instructions(root: &Value) -> Option<&[Value]> {
    match root {
        Value::Object(map) => {
            if let Some(Value::Array(i)) = map.get("instructions") {
                return Some(i);
            }
            map.values().find_map(find_instructions)
        }
        Value::Array(items) => items.iter().find_map(find_instructions),
        _ => None,
    }
}

/// Paging direction of a cursor entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CursorKind {
    Top,
    Bottom,
}

/// One timeline entry, classified but not interpreted.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Entry<'a> {
    /// A single item; `content` is its `itemContent`.
    Item {
        id: &'a str,
        content: &'a Value,
    },
    /// A group of items (conversations, carousels, who-to-follow…).
    Module {
        id: &'a str,
        display_type: &'a str,
        items: &'a [Value],
    },
    Cursor {
        kind: CursorKind,
        value: &'a str,
    },
    /// Anything else (unknown entry types).
    Other {
        id: &'a str,
    },
}

#[cfg(test)]
impl<'a> Entry<'a> {
    pub(crate) fn id(&self) -> &'a str {
        match self {
            Entry::Item { id, .. } | Entry::Module { id, .. } | Entry::Other { id } => id,
            Entry::Cursor { .. } => "cursor",
        }
    }
}

/// The `(entryId, itemContent)` pairs of a module's items.
pub(crate) fn module_items(items: &[Value]) -> impl Iterator<Item = (&str, &Value)> {
    items.iter().filter_map(|i| {
        let id = str_at(i, &["entryId"]).unwrap_or_default();
        Some((id, at(i, &["item", "itemContent"])?))
    })
}

fn entry(e: &Value) -> Option<Entry<'_>> {
    let id = str_at(e, &["entryId"]).unwrap_or_default();
    let content = at(e, &["content"])?;
    let kind = str_at(content, &["entryType"]).or_else(|| str_at(content, &["__typename"]))?;
    Some(match kind {
        "TimelineTimelineItem" => match at(content, &["itemContent"]) {
            Some(item) => Entry::Item { id, content: item },
            None => Entry::Other { id },
        },
        "TimelineTimelineModule" => Entry::Module {
            id,
            display_type: str_at(content, &["displayType"]).unwrap_or_default(),
            items: array_at(content, &["items"]),
        },
        "TimelineTimelineCursor" => {
            let value = str_at(content, &["value"])?;
            match str_at(content, &["cursorType"])? {
                "Top" => Entry::Cursor { kind: CursorKind::Top, value },
                "Bottom" => Entry::Cursor { kind: CursorKind::Bottom, value },
                _ => Entry::Other { id },
            }
        }
        _ => Entry::Other { id },
    })
}

/// All entries of a timeline response, in display order (pinned entries first).
///
/// Instructions handled: `TimelinePinEntry`, `TimelineAddEntries`,
/// `TimelineReplaceEntry` (cursor refreshes) and `TimelineAddToModule` (later
/// pages of a module, e.g. a profile's media grid). Others (cache clearing,
/// covers, unread markers…) carry nothing to display.
pub(crate) fn timeline(root: &Value) -> Vec<Entry<'_>> {
    let Some(instructions) = find_instructions(root) else { return Vec::new() };
    let mut pinned = Vec::new();
    let mut rest = Vec::new();
    for ins in instructions {
        match str_at(ins, &["type"]) {
            Some("TimelinePinEntry") => pinned.extend(at(ins, &["entry"]).and_then(entry)),
            Some("TimelineAddEntries") => {
                rest.extend(array_at(ins, &["entries"]).iter().filter_map(entry))
            }
            Some("TimelineReplaceEntry") => rest.extend(at(ins, &["entry"]).and_then(entry)),
            Some("TimelineAddToModule") => rest.push(Entry::Module {
                id: str_at(ins, &["moduleEntryId"]).unwrap_or_default(),
                display_type: "",
                items: array_at(ins, &["moduleItems"]),
            }),
            _ => {}
        }
    }
    pinned.extend(rest);
    pinned
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn accessors_are_lenient() {
        let v = json!({"a": {"n": "42", "m": 7, "z": null, "b": true}});
        assert_eq!(u64_at(&v, &["a", "n"]), Some(42));
        assert_eq!(u64_at(&v, &["a", "m"]), Some(7));
        assert_eq!(at(&v, &["a", "z"]), None, "null counts as missing");
        assert_eq!(bool_at(&v, &["a", "b"]), Some(true));
        assert_eq!(str_at(&v, &["a", "missing", "deep"]), None);
        assert!(array_at(&v, &["a"]).is_empty());
    }

    #[test]
    fn timeline_orders_pins_first_and_classifies() {
        let v = json!({"data": {"x": {"timeline": {"instructions": [
            {"type": "TimelineClearCache"},
            {"type": "TimelineAddEntries", "entries": [
                {"entryId": "tweet-1", "content": {"entryType": "TimelineTimelineItem", "itemContent": {"__typename": "TimelineTweet"}}},
                {"entryId": "cursor-bottom-1", "content": {"entryType": "TimelineTimelineCursor", "cursorType": "Bottom", "value": "B"}},
                {"entryId": "conv-1", "content": {"entryType": "TimelineTimelineModule", "displayType": "VerticalConversation", "items": [
                    {"entryId": "conv-1-tweet-2", "item": {"itemContent": {"__typename": "TimelineTweet"}}}
                ]}},
                {"entryId": "weird", "content": {"entryType": "SomethingNew"}}
            ]},
            {"type": "TimelinePinEntry", "entry": {"entryId": "tweet-9", "content": {"entryType": "TimelineTimelineItem", "itemContent": {"__typename": "TimelineTweet"}}}}
        ]}}}});
        let entries = timeline(&v);
        let ids: Vec<&str> = entries.iter().map(Entry::id).collect();
        assert_eq!(ids, ["tweet-9", "tweet-1", "cursor", "conv-1", "weird"]);
        assert!(matches!(entries[2], Entry::Cursor { kind: CursorKind::Bottom, value: "B" }));
        let Entry::Module { items, display_type, .. } = entries[3] else { panic!("module") };
        assert_eq!(display_type, "VerticalConversation");
        assert_eq!(module_items(items).count(), 1);
        assert!(matches!(entries[4], Entry::Other { id: "weird" }));
    }

    #[test]
    fn add_to_module_pages_are_module_entries() {
        let v = json!({"instructions": [
            {"type": "TimelineAddToModule", "moduleEntryId": "profile-grid-0", "moduleItems": [
                {"entryId": "profile-grid-0-tweet-1", "item": {"itemContent": {"__typename": "TimelineTweet"}}},
                {"entryId": "profile-grid-0-tweet-2", "item": {"itemContent": {"__typename": "TimelineTweet"}}}
            ]},
            {"type": "TimelineAddEntries", "entries": [
                {"entryId": "cursor-bottom-2", "content": {"entryType": "TimelineTimelineCursor", "cursorType": "Bottom", "value": "B2"}}
            ]}
        ]});
        let entries = timeline(&v);
        let Entry::Module { id, items, .. } = entries[0] else { panic!("module") };
        assert_eq!(id, "profile-grid-0");
        assert_eq!(module_items(items).count(), 2);
        assert!(matches!(entries[1], Entry::Cursor { kind: CursorKind::Bottom, value: "B2" }));
    }

    #[test]
    fn missing_instructions_is_empty() {
        assert!(timeline(&json!({"data": {}})).is_empty());
    }
}
