//! The "Add column" dialog: which kinds of column exist, and how what the user
//! typed (a query, `@handle`, list or tweet link) or picked (one of their
//! lists) becomes a `FeedSource`.

use twitty_model::{
    FeedSource, HomeMode, ListId, ListSummary, NotificationFilter, PostId, SearchSort, UserTab,
};

/// A kind of column the user can add.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColumnKind {
    Home,
    Notifications,
    Search,
    User,
    List,
    Post,
    Bookmarks,
}

impl ColumnKind {
    /// Every kind, in the dialog's order.
    pub const ALL: [ColumnKind; 7] = [
        ColumnKind::Home,
        ColumnKind::Notifications,
        ColumnKind::Search,
        ColumnKind::User,
        ColumnKind::List,
        ColumnKind::Post,
        ColumnKind::Bookmarks,
    ];

    /// The kind of column showing `source` (a thread is a Tweet column).
    pub fn of(source: &FeedSource) -> ColumnKind {
        match source {
            FeedSource::Home { .. } => ColumnKind::Home,
            FeedSource::Notifications { .. } => ColumnKind::Notifications,
            FeedSource::Search { .. } => ColumnKind::Search,
            FeedSource::User { .. } => ColumnKind::User,
            FeedSource::List { .. } => ColumnKind::List,
            FeedSource::Thread { .. } => ColumnKind::Post,
            FeedSource::Bookmarks => ColumnKind::Bookmarks,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ColumnKind::Home => "Home",
            ColumnKind::Notifications => "Notifications",
            ColumnKind::Search => "Search",
            ColumnKind::User => "User",
            ColumnKind::List => "List",
            ColumnKind::Post => "Tweet",
            ColumnKind::Bookmarks => "Bookmarks",
        }
    }

    /// Placeholder of the input, or `None` when the kind needs no input.
    pub fn prompt(self) -> Option<&'static str> {
        match self {
            ColumnKind::Search => Some("Search query, e.g. rust lang:en"),
            ColumnKind::User => Some("@handle or profile link"),
            ColumnKind::List => Some("List link or id"),
            ColumnKind::Post => Some("Tweet link or id"),
            ColumnKind::Home | ColumnKind::Notifications | ColumnKind::Bookmarks => None,
        }
    }
}

/// The result of reading the dialog's input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Parsed {
    /// Ready to add.
    Source(FeedSource),
    /// A handle that must be looked up (`Backend::profile`) to get the user id.
    LookUpUser(String),
}

/// Why the input can't be used.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InputError {
    #[error("Type something first.")]
    Empty,
    #[error("That isn't a valid @handle.")]
    BadHandle,
    #[error("Paste a list link like x.com/i/lists/123…")]
    BadList,
    #[error("Pick one of your lists or paste a list link.")]
    NoList,
    #[error("Paste a tweet link like x.com/user/status/123…")]
    BadPost,
}

/// Turns the dialog's input into a source for `kind`.
pub fn parse(kind: ColumnKind, input: &str) -> Result<Parsed, InputError> {
    let input = input.trim();
    let needs_input = kind.prompt().is_some();
    if needs_input && input.is_empty() {
        return Err(InputError::Empty);
    }
    Ok(Parsed::Source(match kind {
        ColumnKind::Home => FeedSource::Home { mode: HomeMode::ForYou },
        ColumnKind::Notifications => FeedSource::Notifications { filter: NotificationFilter::All },
        ColumnKind::Bookmarks => FeedSource::Bookmarks,
        ColumnKind::Search => {
            FeedSource::Search { query: input.to_owned(), sort: SearchSort::Latest }
        }
        ColumnKind::User => return handle(input).map(Parsed::LookUpUser),
        ColumnKind::List => FeedSource::List {
            id: ListId::new(id_after(input, &["lists"]).ok_or(InputError::BadList)?),
            name: None,
        },
        ColumnKind::Post => FeedSource::Thread {
            root: PostId::new(id_after(input, &["status", "statuses"]).ok_or(InputError::BadPost)?),
        },
    }))
}

/// The viewer's lists, which the dialog offers for list columns. Loaded the
/// first time the List kind is chosen.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ViewerLists {
    #[default]
    NotLoaded,
    Loading,
    Loaded(Vec<ListSummary>),
    /// Why loading failed; choosing List again retries.
    Failed(String),
}

impl ViewerLists {
    /// Whether choosing List should (re)load them.
    pub fn needs_load(&self) -> bool {
        matches!(self, ViewerLists::NotLoaded | ViewerLists::Failed(_))
    }

    /// The loaded list with `id`.
    pub fn find(&self, id: &ListId) -> Option<&ListSummary> {
        match self {
            ViewerLists::Loaded(lists) => lists.iter().find(|l| &l.id == id),
            _ => None,
        }
    }
}

/// A list column, named after the list.
pub fn list_source(list: &ListSummary) -> FeedSource {
    FeedSource::List { id: list.id.clone(), name: Some(list.name.clone()) }
}

/// A list column from the picked list, unless a link or id was typed: what the
/// user typed wins, so any list (not only their own) can be added.
pub fn parse_list(picked: Option<&ListSummary>, input: &str) -> Result<Parsed, InputError> {
    match picked {
        _ if !input.trim().is_empty() => parse(ColumnKind::List, input),
        Some(list) => Ok(Parsed::Source(list_source(list))),
        None => Err(InputError::NoList),
    }
}

/// A user column once the handle was resolved.
pub fn user_source(id: twitty_model::UserId, handle: &str) -> FeedSource {
    FeedSource::User { id, handle: handle.to_owned(), tab: UserTab::Posts }
}

/// Path segments of an x.com / twitter.com link, or `None` when `input` isn't one.
fn link_segments(input: &str) -> Option<Vec<&str>> {
    let rest =
        input.strip_prefix("https://").or_else(|| input.strip_prefix("http://")).unwrap_or(input);
    let (host, path) = rest.split_once('/')?;
    let host = host.trim_start_matches("www.").trim_start_matches("mobile.");
    if !matches!(host, "x.com" | "twitter.com") {
        return None;
    }
    let path = path.split(['?', '#']).next().unwrap_or_default();
    Some(path.split('/').filter(|s| !s.is_empty()).collect())
}

/// A numeric id: the input itself, or the segment after one of `markers` in a link.
fn id_after(input: &str, markers: &[&str]) -> Option<String> {
    let numeric = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit());
    if numeric(input) {
        return Some(input.to_owned());
    }
    let segments = link_segments(input)?;
    let at = segments.iter().position(|s| markers.contains(s))?;
    segments.get(at + 1).filter(|s| numeric(s)).map(|s| (*s).to_owned())
}

/// A handle from `@name`, `name` or a profile link.
fn handle(input: &str) -> Result<String, InputError> {
    let name = match link_segments(input) {
        Some(segments) => segments.first().copied().unwrap_or_default(),
        None => input.trim_start_matches('@'),
    };
    let valid = (1..=15).contains(&name.len())
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    if valid { Ok(name.to_owned()) } else { Err(InputError::BadHandle) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn every_column_has_the_kind_that_adds_it() {
        let inputs = [
            (ColumnKind::Home, ""),
            (ColumnKind::Notifications, ""),
            (ColumnKind::Search, "rust"),
            (ColumnKind::List, "123"),
            (ColumnKind::Post, "456"),
            (ColumnKind::Bookmarks, ""),
        ];
        for (kind, input) in inputs {
            let Ok(Parsed::Source(source)) = parse(kind, input) else {
                panic!("{kind:?} parses {input:?}");
            };
            assert_eq!(ColumnKind::of(&source), kind);
        }
        let user = user_source(twitty_model::UserId::new("1"), "someone");
        assert_eq!(ColumnKind::of(&user), ColumnKind::User);
    }

    #[test]
    fn kinds_without_input_need_none() {
        assert_eq!(parse(ColumnKind::Bookmarks, ""), Ok(Parsed::Source(FeedSource::Bookmarks)));
        assert!(matches!(
            parse(ColumnKind::Home, "ignored"),
            Ok(Parsed::Source(FeedSource::Home { .. }))
        ));
        assert_eq!(parse(ColumnKind::Search, "  "), Err(InputError::Empty));
    }

    #[test]
    fn search_keeps_the_query() {
        assert_eq!(
            parse(ColumnKind::Search, " rust lang:en "),
            Ok(Parsed::Source(FeedSource::Search {
                query: "rust lang:en".into(),
                sort: SearchSort::Latest
            }))
        );
    }

    #[test]
    fn handles_from_names_and_links() {
        for input in [
            "NASA",
            "@NASA",
            "https://x.com/NASA",
            "x.com/NASA/media",
            "https://twitter.com/NASA?s=20",
        ] {
            assert_eq!(
                parse(ColumnKind::User, input),
                Ok(Parsed::LookUpUser("NASA".into())),
                "{input}"
            );
        }
        for bad in ["not a handle", "@", "https://example.com/NASA", "@waytoolonghandle_123"] {
            assert_eq!(parse(ColumnKind::User, bad), Err(InputError::BadHandle), "{bad}");
        }
    }

    #[test]
    fn lists_from_ids_and_links() {
        let expected = Ok(Parsed::Source(FeedSource::List {
            id: ListId::new("1613023332353318913"),
            name: None,
        }));
        for input in [
            "1613023332353318913",
            "https://x.com/i/lists/1613023332353318913",
            "x.com/i/lists/1613023332353318913/members",
        ] {
            assert_eq!(parse(ColumnKind::List, input), expected, "{input}");
        }
        assert_eq!(parse(ColumnKind::List, "https://x.com/i/lists/abc"), Err(InputError::BadList));
        assert_eq!(parse(ColumnKind::List, "rust people"), Err(InputError::BadList));
    }

    fn list(id: &str, name: &str) -> ListSummary {
        ListSummary {
            id: ListId::new(id),
            name: name.into(),
            members: 3,
            visibility: twitty_model::ListVisibility::Private,
        }
    }

    #[test]
    fn a_picked_list_is_added_with_its_name_unless_a_link_is_typed() {
        let mine = list("7", "Friends");
        assert_eq!(
            parse_list(Some(&mine), "  "),
            Ok(Parsed::Source(FeedSource::List {
                id: ListId::new("7"),
                name: Some("Friends".into())
            }))
        );
        assert_eq!(
            parse_list(Some(&mine), "x.com/i/lists/42"),
            Ok(Parsed::Source(FeedSource::List { id: ListId::new("42"), name: None })),
            "the typed link wins"
        );
        assert_eq!(parse_list(Some(&mine), "nope"), Err(InputError::BadList));
        assert_eq!(parse_list(None, ""), Err(InputError::NoList));
    }

    #[test]
    fn viewer_lists_load_once_and_retry_after_a_failure() {
        assert!(ViewerLists::NotLoaded.needs_load());
        assert!(!ViewerLists::Loading.needs_load());
        assert!(ViewerLists::Failed("down".into()).needs_load());
        let loaded = ViewerLists::Loaded(vec![list("7", "Friends")]);
        assert!(!loaded.needs_load());
        assert_eq!(loaded.find(&ListId::new("7")).map(|l| l.name.as_str()), Some("Friends"));
        assert_eq!(loaded.find(&ListId::new("8")), None);
        assert_eq!(ViewerLists::Loading.find(&ListId::new("7")), None);
    }

    #[test]
    fn posts_from_ids_and_links() {
        let expected =
            Ok(Parsed::Source(FeedSource::Thread { root: PostId::new("2112560723603632217") }));
        for input in [
            "2112560723603632217",
            "https://x.com/NASA/status/2112560723603632217",
            "https://twitter.com/NASA/status/2112560723603632217/photo/1",
            "https://mobile.twitter.com/NASA/statuses/2112560723603632217?s=20",
        ] {
            assert_eq!(parse(ColumnKind::Post, input), expected, "{input}");
        }
        assert_eq!(parse(ColumnKind::Post, "https://x.com/NASA"), Err(InputError::BadPost));
    }
}
