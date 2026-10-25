//! The profile sheet: its tabs, the media gallery and how its list rows map
//! to the header, tweets and gallery rows.

use std::{ops::Range, sync::Arc};

use twitty_model::{FeedItem, FeedSource, Post, Profile, UserTab};

/// A tab of the profile sheet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileTab {
    Tweets,
    Media,
}

impl ProfileTab {
    pub const ALL: [ProfileTab; 2] = [ProfileTab::Tweets, ProfileTab::Media];

    pub fn label(self) -> &'static str {
        match self {
            ProfileTab::Tweets => "Tweets",
            ProfileTab::Media => "Media",
        }
    }

    /// The feed behind this tab for `user`.
    pub fn source(self, user: &Profile) -> FeedSource {
        let tab = match self {
            ProfileTab::Tweets => UserTab::Posts,
            ProfileTab::Media => UserTab::Media,
        };
        FeedSource::User { id: user.id.clone(), handle: user.handle.to_string(), tab }
    }
}

/// Rows above the content: the profile header and the tab bar.
pub const HEAD_ROWS: usize = 2;
/// Tiles per gallery row.
pub const GALLERY_COLUMNS: usize = 3;

/// One picture, GIF or video of the gallery: `post.media[index]`.
#[derive(Debug, Clone, PartialEq)]
pub struct Tile {
    pub post: Arc<Post>,
    pub index: usize,
}

/// Every media of the tweets in `items`, newest tweet first, in each tweet's
/// order. Retweets are left out: their media is someone else's. So are pins:
/// a pinned tweet also has its own row in its place in time.
pub fn tiles(items: &[FeedItem]) -> Vec<Tile> {
    let posts = items.iter().flat_map(|item| match item {
        FeedItem::Post(p) => vec![p],
        FeedItem::Thread(ps) => ps.iter().collect(),
        _ => Vec::new(),
    });
    posts
        .filter(|p| !p.media.is_empty())
        .flat_map(|p| {
            let post = Arc::new(p.clone());
            (0..p.media.len()).map(move |index| Tile { post: Arc::clone(&post), index })
        })
        .collect()
}

/// What a list row of the sheet shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Row {
    Header,
    Tabs,
    /// A feed row (Tweets).
    Item(usize),
    /// Tiles `range` side by side (Media).
    Gallery(Range<usize>),
    /// Loading, error or end-of-list message.
    Footer,
}

/// Rows needed to show `content` units: items for Tweets, tiles for Media.
fn content_rows(tab: ProfileTab, content: usize) -> usize {
    match tab {
        ProfileTab::Tweets => content,
        ProfileTab::Media => content.div_ceil(GALLERY_COLUMNS),
    }
}

/// Number of list rows for `tab` with `content` units.
pub fn row_count(tab: ProfileTab, content: usize) -> usize {
    HEAD_ROWS + content_rows(tab, content) + 1
}

/// What row `ix` shows for `tab` with `content` units.
pub fn row(tab: ProfileTab, ix: usize, content: usize) -> Row {
    match ix {
        0 => Row::Header,
        1 => Row::Tabs,
        ix if ix - HEAD_ROWS < content_rows(tab, content) => {
            let r = ix - HEAD_ROWS;
            match tab {
                ProfileTab::Tweets => Row::Item(r),
                ProfileTab::Media => {
                    let start = r * GALLERY_COLUMNS;
                    Row::Gallery(start..(start + GALLERY_COLUMNS).min(content))
                }
            }
        }
        _ => Row::Footer,
    }
}

/// The list splice that turns `old` content units into `new` ones when the
/// first `kept` units did not change: the rows to replace and how many
/// replace them.
pub fn splice(tab: ProfileTab, kept: usize, old: usize, new: usize) -> (Range<usize>, usize) {
    // A gallery row holding a kept tile may also get new ones.
    let first = HEAD_ROWS + kept / if tab == ProfileTab::Media { GALLERY_COLUMNS } else { 1 };
    (first..row_count(tab, old), row_count(tab, new) - first)
}

/// The profile's website as Twitter shows it: without the scheme, `www.`
/// and a trailing `/`.
pub fn website_label(p: &Profile) -> Option<String> {
    let url = p.website.as_ref()?;
    let rest = url.as_str().split_once("://").map_or(url.as_str(), |(_, rest)| rest);
    let rest = rest.strip_prefix("www.").unwrap_or(rest);
    Some(rest.strip_suffix('/').unwrap_or(rest).to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use twitty_model::{FeedPage, PageRequest, UserId, mock::MockBackend};

    #[test]
    fn website_labels_drop_the_scheme_www_and_trailing_slash() {
        let label = |url: &str| {
            let mut p = Profile::new(UserId::new("1"), "a", "A");
            p.website = Some(url.parse().unwrap());
            website_label(&p).unwrap()
        };
        assert_eq!(label("https://www.nasa.gov/"), "nasa.gov");
        assert_eq!(label("http://rust-lang.org/learn"), "rust-lang.org/learn");
        assert_eq!(label("https://a.io/b/"), "a.io/b");
        assert_eq!(website_label(&Profile::new(UserId::new("1"), "a", "A")), None);
    }

    fn mock_items(source: &FeedSource) -> Vec<FeedItem> {
        use twitty_model::Backend;
        let backend = MockBackend::new();
        let page: FeedPage = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(backend.feed(source, PageRequest::First))
            .unwrap();
        page.items
    }

    #[test]
    fn tabs_load_the_users_posts_and_media() {
        let user = Profile::new(UserId::new("42"), "nasa", "NASA");
        assert_eq!(
            ProfileTab::Tweets.source(&user),
            FeedSource::User { id: UserId::new("42"), handle: "nasa".into(), tab: UserTab::Posts }
        );
        assert_eq!(
            ProfileTab::Media.source(&user),
            FeedSource::User { id: UserId::new("42"), handle: "nasa".into(), tab: UserTab::Media }
        );
    }

    #[test]
    fn tiles_cover_every_media_in_order() {
        let items = mock_items(&FeedSource::Bookmarks);
        let tiles = tiles(&items);
        let expected: usize = items
            .iter()
            .filter(|i| !matches!(i, FeedItem::Repost { .. }))
            .flat_map(|i| i.posts())
            .map(|p| p.media.len())
            .sum();
        assert!(expected > 0, "the mock has media");
        assert_eq!(tiles.len(), expected);
        for t in &tiles {
            assert!(t.index < t.post.media.len(), "{t:?} points at a media");
        }
        // A tweet's media stay together, in their order.
        for pair in tiles.windows(2) {
            if pair[0].post.id == pair[1].post.id {
                assert_eq!(pair[1].index, pair[0].index + 1);
            } else {
                assert_eq!(pair[1].index, 0);
            }
        }
    }

    #[test]
    fn reposts_are_not_in_the_gallery() {
        let items = mock_items(&FeedSource::Bookmarks);
        let reposted: Vec<_> = items
            .iter()
            .filter_map(|i| match i {
                FeedItem::Repost { post, .. } => Some(post.id.clone()),
                _ => None,
            })
            .collect();
        assert!(!reposted.is_empty(), "the mock has retweets");
        assert!(tiles(&items).iter().all(|t| !reposted.contains(&t.post.id)));
    }

    #[test]
    fn rows_map_to_header_content_and_footer() {
        assert_eq!(row_count(ProfileTab::Tweets, 0), 3);
        assert_eq!(row(ProfileTab::Tweets, 0, 0), Row::Header);
        assert_eq!(row(ProfileTab::Tweets, 1, 0), Row::Tabs);
        assert_eq!(row(ProfileTab::Tweets, 2, 0), Row::Footer);

        assert_eq!(row_count(ProfileTab::Tweets, 4), 7);
        assert_eq!(row(ProfileTab::Tweets, 2, 4), Row::Item(0));
        assert_eq!(row(ProfileTab::Tweets, 5, 4), Row::Item(3));
        assert_eq!(row(ProfileTab::Tweets, 6, 4), Row::Footer);

        // 7 tiles: two full rows and one with a single tile.
        assert_eq!(row_count(ProfileTab::Media, 7), 6);
        assert_eq!(row(ProfileTab::Media, 2, 7), Row::Gallery(0..3));
        assert_eq!(row(ProfileTab::Media, 4, 7), Row::Gallery(6..7));
        assert_eq!(row(ProfileTab::Media, 5, 7), Row::Footer);
    }

    #[test]
    fn splices_replace_changed_rows_and_the_footer() {
        // Two tweets appended to three: rows from the old footer on.
        assert_eq!(splice(ProfileTab::Tweets, 3, 3, 5), (5..6, 3));
        // Everything replaced.
        assert_eq!(splice(ProfileTab::Tweets, 0, 3, 1), (2..6, 2));
        // Tiles appended to 4: the half-full second row is redone.
        assert_eq!(splice(ProfileTab::Media, 4, 4, 9), (3..5, 3));
        // Appended to a full row: only the footer is redone.
        assert_eq!(splice(ProfileTab::Media, 3, 3, 4), (3..4, 2));
    }

    proptest::proptest! {
        #[test]
        fn splices_keep_the_list_in_step(
            media in proptest::bool::ANY,
            old in 0usize..50,
            kept_frac in 0usize..=100,
            new_extra in 0usize..50,
        ) {
            let tab = if media { ProfileTab::Media } else { ProfileTab::Tweets };
            let kept = old * kept_frac / 100;
            let new = kept + new_extra;
            let (range, count) = splice(tab, kept, old, new);
            proptest::prop_assert!(range.start >= HEAD_ROWS);
            proptest::prop_assert!(range.end == row_count(tab, old));
            proptest::prop_assert!(range.start < range.end, "at least the footer is redone");
            // After the splice the list has as many rows as the model needs.
            proptest::prop_assert_eq!(row_count(tab, old) - range.len() + count, row_count(tab, new));
        }
    }
}
