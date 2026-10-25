//! Twitter user results → `Profile`.
//!
//! As of 2026-10 user objects carry `core`, `avatar`, `profile_bio`,
//! `relationship_counts`, `verification`… and no `legacy` block. The older
//! `legacy` layout is still read as a fallback.

use serde_json::Value;
use twitty_model::{
    Badges, ImageRef, Profile, ProfileCounts, RichText, UserId, VerifiedKind, ViewerRelation,
};
use url::Url;

use super::text;
use crate::wire::{at, bool_at, str_at, u64_at};

/// Translates a `user_results.result`. `None` when it isn't a usable user.
pub(crate) fn profile(user: &Value) -> Option<Profile> {
    if str_at(user, &["__typename"]).is_some_and(|t| t != "User") {
        return None;
    }
    let id = str_at(user, &["rest_id"]).or_else(|| str_at(user, &["legacy", "id_str"]))?;
    let either = |new: &[&str], old: &[&str]| str_at(user, new).or_else(|| str_at(user, old));
    let handle = either(&["core", "screen_name"], &["legacy", "screen_name"])?;
    let name = either(&["core", "name"], &["legacy", "name"]).unwrap_or(handle);

    let mut p = Profile::new(UserId::new(id), handle, name);
    p.avatar = either(&["avatar", "image_url"], &["legacy", "profile_image_url_https"])
        .and_then(|u| Url::parse(&avatar_size(u)).ok())
        .map(|url| ImageRef::sized(url, Some((73, 73))));
    p.banner = either(&["banner", "image_url"], &["legacy", "profile_banner_url"])
        .and_then(|u| Url::parse(u).ok())
        .map(ImageRef::new);
    p.bio = bio(user);
    p.location = either(&["location", "location"], &["legacy", "location"])
        .filter(|l| !l.is_empty())
        .map(str::to_owned);
    p.website = website(user);
    p.badges = Badges {
        verified: verified(user),
        protected: bool_at(user, &["privacy", "protected"])
            .or_else(|| bool_at(user, &["legacy", "protected"]))
            .unwrap_or(false),
    };
    let n =
        |new: &[&str], old: &[&str]| u64_at(user, new).or_else(|| u64_at(user, old)).unwrap_or(0);
    p.counts = ProfileCounts {
        followers: n(&["relationship_counts", "followers"], &["legacy", "followers_count"]),
        following: n(&["relationship_counts", "following"], &["legacy", "friends_count"]),
        posts: n(&["tweet_counts", "tweets"], &["legacy", "statuses_count"]),
    };
    let rel = |k: &str| {
        bool_at(user, &["relationship_perspectives", k])
            .or_else(|| bool_at(user, &["legacy", k]))
            .unwrap_or(false)
    };
    p.viewer = ViewerRelation {
        following: rel("following"),
        followed_by: rel("followed_by"),
        blocking: rel("blocking"),
        muting: rel("muting"),
    };
    Some(p)
}

/// The 48 px `_normal` avatar is too small for 40 px at 2x; `_bigger` is 73 px.
fn avatar_size(url: &str) -> String {
    url.replacen("_normal.", "_bigger.", 1)
}

/// The bio, with its links from Twitter and its mentions, hashtags and
/// cashtags found here: Twitter only marks up the links.
fn bio(user: &Value) -> RichText {
    let read = |base: &str| {
        let desc = str_at(user, &[base, "description"])?;
        let ents = at(user, &[base, "entities", "description"]).map(text::entities);
        let ents = text::with_typed_entities(desc, ents.unwrap_or_default());
        Some(text::rich_text(desc, ents, None))
    };
    read("profile_bio").or_else(|| read("legacy")).unwrap_or_default()
}

fn website(user: &Value) -> Option<Url> {
    let expanded = |base: &[&str]| {
        let urls = at(user, base)?.get("urls")?.as_array()?;
        str_at(urls.first()?, &["expanded_url"]).map(str::to_owned)
    };
    expanded(&["profile_bio", "entities", "url"])
        .or_else(|| expanded(&["legacy", "entities", "url"]))
        .or_else(|| str_at(user, &["website", "url"]).filter(|u| !u.is_empty()).map(str::to_owned))
        .and_then(|u| Url::parse(&u).ok())
}

fn verified(user: &Value) -> Option<VerifiedKind> {
    let kind = str_at(user, &["verification", "verified_type"])
        .or_else(|| str_at(user, &["legacy", "verified_type"]));
    match kind {
        Some("Business") => Some(VerifiedKind::Business),
        Some("Government") => Some(VerifiedKind::Government),
        _ if bool_at(user, &["is_blue_verified"]).unwrap_or(false) => {
            Some(VerifiedKind::Individual)
        }
        _ if bool_at(user, &["verification", "verified"]).unwrap_or(false) => {
            Some(VerifiedKind::Individual)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use twitty_model::SpanKind;

    fn new_layout() -> Value {
        json!({
            "__typename": "User",
            "rest_id": "11348282",
            "core": {"name": "NASA", "screen_name": "NASA", "created_at": "Wed Dec 19 20:20:32 +0000 2007"},
            "avatar": {"image_url": "https://pbs.twimg.com/profile_images/1/abc_normal.jpg"},
            "banner": {"image_url": "https://pbs.twimg.com/profile_banners/11348282/1"},
            "profile_bio": {"description": "Explore https://t.co/x", "entities": {
                "description": {"urls": [{"url": "https://t.co/x", "expanded_url": "https://nasa.gov", "display_url": "nasa.gov", "indices": [8, 22]}]},
                "url": {"urls": [{"url": "https://t.co/y", "expanded_url": "https://www.nasa.gov/", "indices": [0, 23]}]}
            }},
            "location": {"location": "Pale Blue Dot"},
            "is_blue_verified": true,
            "verification": {"verified": false, "verified_type": "Government"},
            "privacy": {"protected": false},
            "relationship_counts": {"followers": 92310469, "following": 115},
            "tweet_counts": {"tweets": 75000},
            "relationship_perspectives": {"following": true, "followed_by": false, "blocking": false, "muting": false}
        })
    }

    #[test]
    fn new_layout_translates() {
        let p = profile(&new_layout()).unwrap();
        assert_eq!(
            (p.id.as_str(), p.handle.as_str(), p.display_name.as_str()),
            ("11348282", "NASA", "NASA")
        );
        assert_eq!(
            p.avatar.unwrap().url.as_str(),
            "https://pbs.twimg.com/profile_images/1/abc_bigger.jpg"
        );
        assert_eq!(p.bio.text(), "Explore nasa.gov");
        assert_eq!(p.bio.spans().len(), 1, "the link");
        assert_eq!(p.location.as_deref(), Some("Pale Blue Dot"));
        assert_eq!(p.website.unwrap().as_str(), "https://www.nasa.gov/");
        assert_eq!(p.badges.verified, Some(VerifiedKind::Government));
        assert_eq!(
            p.counts,
            ProfileCounts { followers: 92_310_469, following: 115, posts: 75_000 }
        );
        assert!(p.viewer.following);
    }

    #[test]
    fn legacy_layout_still_translates() {
        let u = json!({
            "rest_id": "1",
            "legacy": {"screen_name": "old", "name": "Old Layout", "description": "bio", "followers_count": 5,
                       "friends_count": 6, "statuses_count": 7, "protected": true, "location": "",
                       "profile_image_url_https": "https://pbs.twimg.com/p/a_normal.png"}
        });
        let p = profile(&u).unwrap();
        assert_eq!(p.display_name, "Old Layout");
        assert_eq!(p.counts.posts, 7);
        assert!(p.badges.protected);
        assert_eq!(p.location, None, "empty location is none");
        assert_eq!(p.bio.text(), "bio");
    }

    /// Twitter only marks up a bio's links; the rest is found in the text.
    #[test]
    fn bio_mentions_hashtags_and_cashtags_are_found() {
        let u = json!({"rest_id": "1", "core": {"screen_name": "a", "name": "A"},
            "profile_bio": {"description": "Team @rustlang #RustLang $TSLA https://t.co/x mail me@ex.io",
                "entities": {"description": {"urls": [{"url": "https://t.co/x", "expanded_url": "https://rust-lang.org",
                    "display_url": "rust-lang.org", "indices": [31, 45]}]}}}});
        let bio = profile(&u).unwrap().bio;
        let spans: Vec<_> =
            bio.spans().iter().map(|s| (&bio.text()[s.range.clone()], &s.kind)).collect();
        assert_eq!(spans.len(), 4, "{spans:?}");
        assert!(
            matches!(spans[0], ("@rustlang", SpanKind::Mention(u)) if u.handle == "rustlang" && u.id.is_none())
        );
        assert!(
            matches!(spans[1], ("#RustLang", SpanKind::Hashtag { tag }) if tag.as_str() == "RustLang")
        );
        assert!(
            matches!(spans[2], ("$TSLA", SpanKind::Cashtag { symbol }) if symbol.as_str() == "TSLA")
        );
        assert!(matches!(spans[3], ("rust-lang.org", SpanKind::Link { .. })));
    }

    #[test]
    fn legacy_bio_finds_mentions_too() {
        let u = json!({"rest_id": "1",
            "legacy": {"screen_name": "old", "name": "Old", "description": "by @ferris"}});
        let bio = profile(&u).unwrap().bio;
        assert_eq!(&bio.text()[bio.spans()[0].range.clone()], "@ferris");
    }

    #[test]
    fn verification_kinds() {
        let v = |j: Value| verified(&j);
        assert_eq!(
            v(json!({"is_blue_verified": true, "verification": {"verified_type": "Business"}})),
            Some(VerifiedKind::Business)
        );
        assert_eq!(
            v(json!({"is_blue_verified": true, "verification": {"verified": false}})),
            Some(VerifiedKind::Individual)
        );
        assert_eq!(
            v(json!({"is_blue_verified": false, "verification": {"verified": false}})),
            None
        );
    }

    #[test]
    fn non_users_are_rejected() {
        assert!(profile(&json!({"__typename": "UserUnavailable", "rest_id": "1"})).is_none());
        assert!(profile(&json!({"rest_id": "1"})).is_none(), "no handle");
    }
}
