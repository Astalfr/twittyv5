//! What a column announces (sound, system notification) when new rows
//! arrive while it polls.

use std::sync::Arc;

use twitty_model::{FeedItem, ImageRef, Notification, NotificationKind, Post, Profile};

use crate::state::column::{ListChange, LoadKind};

/// Longest text in a notification, in characters (an ellipsis follows):
/// a whole classic tweet.
pub const TEXT_CHARS: usize = 280;

/// One batch of new rows, as the system notification shows it: the newest
/// row, and how many more came with it.
#[derive(Debug, Clone, PartialEq)]
pub struct Announcement {
    /// How many rows arrived.
    pub count: usize,
    /// Who the newest row is from: the title names them and the picture is
    /// their avatar. `None` (a notice without an actor) titles with the
    /// column alone.
    pub from: Option<Arc<Profile>>,
    /// The notification's text, one line per entry: what the newest row
    /// says, then "+N more tweets" when others came with it.
    pub lines: Vec<String>,
}

impl Announcement {
    /// The title for column `column`: "Column - Name @handle".
    pub fn title(&self, column: &str) -> String {
        match &self.from {
            Some(who) => format!("{column} - {} @{}", who.display_name, who.handle),
            None => column.to_owned(),
        }
    }

    /// The text under the title.
    pub fn body(&self) -> String {
        self.lines.join("\n")
    }

    /// Where the picture comes from, when the actor has one.
    pub fn avatar(&self) -> Option<&ImageRef> {
        self.from.as_ref()?.avatar.as_ref()
    }
}

/// The announcement for the rows a load of `kind` added, if it is one to
/// announce: only newer rows added by polling (or a manual check), which go
/// at the top or right below the pinned tweets. The first page, a refresh
/// and older pages are not news.
///
/// `items` are the column's rows after the change.
pub fn announce(kind: LoadKind, change: &ListChange, items: &[FeedItem]) -> Option<Announcement> {
    let added = match (kind, change) {
        (LoadKind::Newer, ListChange::Splice { range, count }) => range.start..range.start + count,
        _ => return None,
    };
    let fresh = items.get(added)?;
    let news: Vec<&FeedItem> = fresh.iter().filter(|item| is_news(item)).collect();
    let (newest, others) = news.split_first()?;
    let (from, mut lines) = describe(newest)?;
    lines.retain(|line| !line.is_empty());
    if let Some(more) = more(others) {
        lines.push(more);
    }
    Some(Announcement { count: news.len(), from, lines })
}

/// Rows worth telling the user about (not pins, tombstones or unreadable rows).
fn is_news(item: &FeedItem) -> bool {
    matches!(
        item,
        FeedItem::Post(_)
            | FeedItem::Repost { .. }
            | FeedItem::Thread(_)
            | FeedItem::Notification(_)
    )
}

/// Who `item` is from, and what it says.
fn describe(item: &FeedItem) -> Option<(Option<Arc<Profile>>, Vec<String>)> {
    Some(match item {
        FeedItem::Post(post) | FeedItem::Pinned(post) => tweet(post),
        // Titled with the tweet's author, like the row shows the tweet.
        FeedItem::Repost { by, post } => {
            let (from, mut lines) = tweet(post);
            lines.insert(0, format!("{} retweeted", by.display_name));
            (from, lines)
        }
        // The newest tweet of the conversation is the news.
        FeedItem::Thread(posts) => tweet(posts.last()?),
        FeedItem::Notification(n) => notification(n),
        FeedItem::Unavailable { .. } | FeedItem::Unsupported { .. } => return None,
    })
}

fn tweet(post: &Post) -> (Option<Arc<Profile>>, Vec<String>) {
    (Some(Arc::clone(&post.author)), vec![text(post)])
}

/// What `post` says, as the notification shows it: its translation when the
/// provider sent one, cut to one line.
pub fn text(post: &Post) -> String {
    let body = post.translation.as_ref().map_or(&post.body, |t| &t.body);
    shorten(body.text())
}

/// An activity notification: its newest actor, what they did, and the start
/// of the tweet it is about.
fn notification(n: &Notification) -> (Option<Arc<Profile>>, Vec<String>) {
    let target = n.target.as_deref().map(text);
    let Some(actor) = n.actors.first() else {
        // A notice from Twitter itself: its summary is all there is.
        return (None, [Some(shorten(n.text.text())), target].into_iter().flatten().collect());
    };
    let what = match n.kind {
        NotificationKind::Like => "liked your tweet",
        NotificationKind::Repost => "retweeted your tweet",
        NotificationKind::Follow => "followed you",
        // Only Twitter's summary says what happened. `Quote` also stands for
        // replies (both arrive with the same kind), so it can't be worded
        // for one of them.
        NotificationKind::Quote | NotificationKind::Other => {
            return (
                Some(Arc::clone(actor)),
                [Some(shorten(n.text.text())), target].into_iter().flatten().collect(),
            );
        }
    };
    // Twitter sends only the first few actors, so their number isn't the
    // count: "and others" stays true whatever it is.
    let action = if n.actors.len() > 1 { format!("and others {what}") } else { what.to_owned() };
    (Some(Arc::clone(actor)), [Some(action), target].into_iter().flatten().collect())
}

/// "+N more tweets" for the rows that came with the newest one.
fn more(others: &[&FeedItem]) -> Option<String> {
    let n = others.len();
    if n == 0 {
        return None;
    }
    let notes = others.iter().filter(|i| matches!(i, FeedItem::Notification(_))).count();
    let noun = match (notes, n == 1) {
        (0, true) => " tweet",
        (0, false) => " tweets",
        (all, true) if all == n => " notification",
        (all, false) if all == n => " notifications",
        // Mentions and activity together.
        _ => "",
    };
    Some(format!("+{n} more{noun}"))
}

/// `text` on one line, cut to [`TEXT_CHARS`] characters.
pub fn shorten(text: &str) -> String {
    let words: Vec<&str> = text.split_whitespace().collect();
    let line = words.join(" ");
    match line.char_indices().nth(TEXT_CHARS) {
        Some((cut, _)) => format!("{}…", line[..cut].trim_end()),
        None => line,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use proptest::prelude::*;
    use twitty_model::{
        Backend, FeedSource, HomeMode, NotificationFilter, PageRequest, RichText, mock::MockBackend,
    };

    fn page(source: FeedSource) -> Vec<FeedItem> {
        let backend = MockBackend::new();
        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(backend.feed(&source, PageRequest::First))
            .unwrap()
            .items
    }

    fn home() -> Vec<FeedItem> {
        page(FeedSource::Home { mode: HomeMode::Following })
    }

    fn posts() -> Vec<Post> {
        home()
            .into_iter()
            .filter_map(|i| match i {
                FeedItem::Post(p) => Some(p),
                _ => None,
            })
            .collect()
    }

    /// The mock's activity notifications, newest first.
    fn notifications() -> Vec<Notification> {
        page(FeedSource::Notifications { filter: NotificationFilter::All })
            .into_iter()
            .filter_map(|i| match i {
                FeedItem::Notification(n) => Some(n),
                _ => None,
            })
            .collect()
    }

    fn notification_of(kind: NotificationKind) -> Notification {
        notifications().into_iter().find(|n| n.kind == kind).unwrap()
    }

    fn prepended(count: usize) -> ListChange {
        ListChange::Splice { range: 0..0, count }
    }

    fn one(item: FeedItem) -> Announcement {
        announce(LoadKind::Newer, &prepended(1), &[item]).unwrap()
    }

    #[test]
    fn a_tweet_is_titled_with_its_author() {
        let post = posts().remove(0);
        let a = one(FeedItem::Post(post.clone()));
        assert_eq!(a.count, 1);
        assert_eq!(
            a.title("Friends"),
            format!("Friends - {} @{}", post.author.display_name, post.author.handle)
        );
        assert_eq!(a.body(), text(&post));
        assert_eq!(a.avatar(), post.author.avatar.as_ref());
    }

    #[test]
    fn a_translated_tweet_shows_its_translation() {
        let mut post = posts().remove(0);
        post.body = RichText::plain("Bonjour tout le monde");
        post.translation = Some(twitty_model::Translation {
            body: RichText::plain("Hello   everyone"),
            from: Some("fr".into()),
            to: Some("en".into()),
        });
        assert_eq!(one(FeedItem::Post(post.clone())).lines, ["Hello everyone"]);

        // Also the tweet an activity notification is about.
        let mut n = notification_of(NotificationKind::Like);
        n.actors.truncate(1);
        n.target = Some(Box::new(post));
        assert_eq!(one(FeedItem::Notification(n)).lines, ["liked your tweet", "Hello everyone"]);
    }

    #[test]
    fn several_tweets_show_the_newest_and_count_the_rest() {
        let items = home();
        let newest = describe(&items[0]).unwrap();
        let three = announce(LoadKind::Newer, &prepended(3), &items).unwrap();
        assert_eq!(three.count, 3);
        assert_eq!(three.from, newest.0);
        assert_eq!(three.lines.last().unwrap(), "+2 more tweets");

        let two = announce(LoadKind::Newer, &prepended(2), &items).unwrap();
        assert_eq!(two.lines.last().unwrap(), "+1 more tweet");
    }

    #[test]
    fn a_retweet_is_titled_with_the_author_and_names_the_retweeter() {
        let mut posts = posts();
        let post = posts.remove(0);
        let by = Arc::clone(&posts[0].author);
        let a = one(FeedItem::Repost { by: Arc::clone(&by), post: post.clone() });
        assert_eq!(a.from, Some(Arc::clone(&post.author)));
        assert_eq!(a.lines, [format!("{} retweeted", by.display_name), text(&post)]);
    }

    #[test]
    fn a_thread_announces_its_newest_tweet() {
        let posts: Vec<Post> = posts().into_iter().take(3).collect();
        let last = posts.last().unwrap().clone();
        let a = one(FeedItem::Thread(posts));
        assert_eq!(a.from, Some(Arc::clone(&last.author)));
        assert_eq!(a.lines, [text(&last)]);
    }

    #[test]
    fn activity_names_the_actor_then_quotes_the_tweet() {
        for (kind, what) in [
            (NotificationKind::Like, "liked your tweet"),
            (NotificationKind::Repost, "retweeted your tweet"),
        ] {
            let mut n = notification_of(kind);
            n.actors.truncate(1);
            let target = text(n.target.as_ref().unwrap());
            let a = one(FeedItem::Notification(n.clone()));
            assert_eq!(a.from, Some(Arc::clone(&n.actors[0])), "{kind:?}");
            assert_eq!(a.lines, [what.to_owned(), target], "{kind:?}");
        }
    }

    #[test]
    fn quotes_and_replies_say_what_twitter_says() {
        let mut n = notification_of(NotificationKind::Quote);
        n.text = RichText::plain("Alice replied to your tweet");
        let target = text(n.target.as_ref().unwrap());
        let a = one(FeedItem::Notification(n.clone()));
        assert_eq!(a.from, Some(Arc::clone(&n.actors[0])));
        assert_eq!(a.lines, ["Alice replied to your tweet".to_owned(), target]);
    }

    #[test]
    fn a_follow_has_no_tweet() {
        let mut n = notification_of(NotificationKind::Follow);
        n.actors.truncate(1);
        assert!(n.target.is_none(), "the mock's follows have no target");
        let a = one(FeedItem::Notification(n));
        assert_eq!(a.lines, ["followed you"]);
    }

    #[test]
    fn several_actors_are_and_others() {
        let n = notifications().into_iter().find(|n| n.actors.len() > 1).unwrap();
        let a = one(FeedItem::Notification(n.clone()));
        assert_eq!(a.from, Some(Arc::clone(&n.actors[0])), "the newest actor");
        assert!(a.lines[0].starts_with("and others "), "{:?}", a.lines);
    }

    #[test]
    fn other_activity_uses_twitters_summary() {
        let mut n = notification_of(NotificationKind::Like);
        n.kind = NotificationKind::Other;
        n.text = RichText::plain("Alice voted in   your poll");
        n.target = None;
        let a = one(FeedItem::Notification(n.clone()));
        assert_eq!(a.from, Some(Arc::clone(&n.actors[0])));
        assert_eq!(a.lines, ["Alice voted in your poll"]);

        // Without an actor, the column alone titles it.
        n.actors.clear();
        let a = one(FeedItem::Notification(n));
        assert_eq!(a.from, None);
        assert_eq!(a.title("Notifications"), "Notifications");
        assert_eq!(a.avatar(), None);
    }

    #[test]
    fn the_rest_is_named_by_what_it_is() {
        let notes: Vec<FeedItem> =
            notifications().into_iter().take(3).map(FeedItem::Notification).collect();
        let a = announce(LoadKind::Newer, &prepended(3), &notes).unwrap();
        assert_eq!(a.lines.last().unwrap(), "+2 more notifications");

        let mut mixed = notes;
        mixed.insert(1, FeedItem::Post(posts().remove(0)));
        let a = announce(LoadKind::Newer, &prepended(3), &mixed).unwrap();
        assert_eq!(a.lines.last().unwrap(), "+2 more");
    }

    #[test]
    fn empty_text_adds_no_line() {
        let mut post = posts().remove(0);
        post.body = RichText::plain("  ");
        assert!(one(FeedItem::Post(post)).lines.is_empty());
    }

    #[test]
    fn other_loads_are_not_news() {
        let items = home();
        let n = items.len();
        assert_eq!(announce(LoadKind::Refresh, &ListChange::Reset { count: n }, &items), None);
        assert_eq!(
            announce(LoadKind::Older, &ListChange::Splice { range: 5..5, count: 3 }, &items),
            None
        );
        assert_eq!(announce(LoadKind::Newer, &ListChange::None, &items), None);
        assert_eq!(announce(LoadKind::Newer, &prepended(0), &items), None);
        assert_eq!(announce(LoadKind::Newer, &prepended(n + 1), &items), None, "out of range");
    }

    #[test]
    fn newer_rows_below_a_pin_are_announced_without_it() {
        let mut items = home();
        let pin = FeedItem::Pinned(posts().remove(0));
        items.insert(0, pin.clone());
        let below = ListChange::Splice { range: 1..1, count: 2 };
        let a = announce(LoadKind::Newer, &below, &items).unwrap();
        assert_eq!(a.count, 2);
        assert_eq!(Some(a), announce(LoadKind::Newer, &prepended(2), &items[1..]));
        assert_eq!(announce(LoadKind::Newer, &prepended(1), &[pin]), None, "a pin isn't news");
    }

    #[test]
    fn only_readable_rows_count() {
        let mut items = vec![
            FeedItem::Unsupported { entry: "a".into(), reason: "new shape".into() },
            FeedItem::Unavailable {
                entry: "b".into(),
                id: None,
                reason: twitty_model::UnavailableReason::Deleted,
            },
        ];
        assert_eq!(announce(LoadKind::Newer, &prepended(2), &items), None);
        items.insert(1, home().remove(0));
        let one = announce(LoadKind::Newer, &prepended(3), &items).unwrap();
        assert_eq!(one.count, 1);
        assert!(!one.lines.last().unwrap().starts_with('+'), "nothing more to count");
    }

    #[test]
    fn text_is_one_line_of_a_tweet() {
        assert_eq!(shorten("  hello\n\nworld  "), "hello world");
        let long = "word ".repeat(100);
        let cut = shorten(&long);
        assert!(cut.ends_with('…'));
        assert!(cut.chars().count() <= TEXT_CHARS + 1);
    }

    proptest! {
        #[test]
        fn shorten_never_panics_and_stays_short(text in "\\PC{0,400}") {
            let line = shorten(&text);
            prop_assert!(line.chars().count() <= TEXT_CHARS + 1);
            prop_assert!(!line.contains('\n'));
        }
    }
}
