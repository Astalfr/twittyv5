//! Lists from `ListsManagementPageTimeline`.

use serde_json::Value;
use twitty_model::{ListId, ListSummary, ListVisibility};

use super::Ctx;
use crate::wire::{self, Entry, at, str_at, u64_at};

/// The "Your Lists" module (owned and followed lists). The page also carries a
/// "Discover new Lists" module (`list-to-follow-module-…`), which is skipped.
const OWN_MODULE: &str = "owned-subscribed-list-module";

/// Result of translating the viewer's lists.
#[derive(Debug)]
pub struct TranslatedLists {
    pub lists: Vec<ListSummary>,
    /// Items in the "Your Lists" module that could not be read, with reasons.
    pub warnings: Vec<String>,
    /// Whether the page had a "Your Lists" module at all. When it doesn't,
    /// Twitter likely renamed it and the empty result is wrong.
    pub found_module: bool,
}

/// The lists the viewer owns or follows, in Twitter's order. Only the first
/// page is read; its bottom cursor is ignored.
pub fn lists(root: &Value) -> TranslatedLists {
    let mut ctx = Ctx::default();
    let mut lists = Vec::new();
    let mut found_module = false;
    for entry in wire::timeline(root) {
        let Entry::Module { id, items, .. } = entry else { continue };
        if !id.starts_with(OWN_MODULE) {
            continue;
        }
        found_module = true;
        for (item_id, content) in wire::module_items(items) {
            match str_at(content, &["itemType"]) {
                Some("TimelineTwitterList") => {}
                // The "you have no lists" prompt.
                Some("TimelineMessagePrompt") => continue,
                t => {
                    ctx.warn(format!("{item_id}: not a list ({t:?})"));
                    continue;
                }
            }
            match at(content, &["list"]).and_then(list) {
                Some(l) => lists.push(l),
                None => ctx.warn(format!("{item_id}: unreadable list")),
            }
        }
    }
    TranslatedLists { lists, warnings: ctx.warnings, found_module }
}

fn list(l: &Value) -> Option<ListSummary> {
    let visibility = match str_at(l, &["mode"])? {
        "Public" => ListVisibility::Public,
        "Private" => ListVisibility::Private,
        other => {
            tracing::warn!("unknown list mode {other:?}");
            return None;
        }
    };
    Some(ListSummary {
        id: ListId::new(str_at(l, &["id_str"])?),
        name: str_at(l, &["name"])?.to_owned(),
        members: u64_at(l, &["member_count"]).unwrap_or(0),
        visibility,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn page(modules: &[(&str, Value)]) -> Value {
        let entries: Vec<Value> = modules
            .iter()
            .map(|(module, items)| {
                json!({
                    "entryId": format!("{module}-1"),
                    "content": {"entryType": "TimelineTimelineModule", "displayType": "Vertical", "items": items},
                })
            })
            .collect();
        json!({"data": {"viewer": {"list_management_timeline": {"timeline": {"instructions": [
            {"type": "TimelineAddEntries", "entries": entries},
        ]}}}}})
    }

    fn item(content: Value) -> Value {
        json!({"entryId": "x-list-1", "item": {"itemContent": content}})
    }

    fn twitter_list(mode: &str) -> Value {
        json!({"itemType": "TimelineTwitterList", "list": {
            "id_str": "77", "name": "Mine", "member_count": 3, "mode": mode,
        }})
    }

    #[test]
    fn reads_only_the_viewers_module() {
        let t = lists(&page(&[
            ("list-to-follow-module", json!([item(twitter_list("Public"))])),
            (OWN_MODULE, json!([item(twitter_list("Private"))])),
        ]));
        assert!(t.found_module && t.warnings.is_empty(), "{:?}", t.warnings);
        assert_eq!(
            t.lists,
            [ListSummary {
                id: ListId::new("77"),
                name: "Mine".into(),
                members: 3,
                visibility: ListVisibility::Private,
            }]
        );
    }

    #[test]
    fn a_page_without_the_viewers_module_is_flagged() {
        let t = lists(&page(&[("list-to-follow-module", json!([item(twitter_list("Public"))]))]));
        assert!(t.lists.is_empty());
        assert!(!t.found_module, "a renamed module must not pass for an empty account");
    }

    #[test]
    fn unreadable_items_are_skipped_with_a_warning() {
        let t = lists(&page(&[(
            OWN_MODULE,
            json!([
                item(twitter_list("Secret")),
                item(json!({"itemType": "TimelineTweet"})),
                item(json!({"itemType": "TimelineMessagePrompt"})),
                item(twitter_list("Public")),
            ]),
        )]));
        assert_eq!(t.lists.len(), 1);
        assert_eq!(t.warnings.len(), 2, "{:?}", t.warnings);
    }
}
