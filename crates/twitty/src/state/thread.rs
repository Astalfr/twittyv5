//! The thread panel: the conversation around one tweet.

use twitty_model::{BackendError, FeedItem, PostId, ThreadView};

/// What the panel shows.
#[derive(Debug, Clone, PartialEq)]
pub enum ThreadState {
    Loading,
    Loaded(Box<ThreadView>),
    Failed(BackendError),
}

/// Rows of a loaded thread: ancestors and the focused tweet as one connected
/// thread, then each reply.
pub fn rows(view: &ThreadView) -> Vec<FeedItem> {
    let head = if view.ancestors.is_empty() {
        FeedItem::Post(view.focus.clone())
    } else {
        let mut chain = view.ancestors.clone();
        chain.push(view.focus.clone());
        FeedItem::Thread(chain)
    };
    std::iter::once(head).chain(view.replies.iter().cloned()).collect()
}

/// The tweet a panel for `view` is about.
pub fn focus_id(view: &ThreadView) -> &PostId {
    &view.focus.id
}

#[cfg(test)]
mod tests {
    use super::*;
    use twitty_model::{Backend, FeedSource, HomeMode, PageRequest, mock::MockBackend};

    fn posts(n: usize) -> Vec<twitty_model::Post> {
        let backend = MockBackend::new();
        let page = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(
                backend.feed(&FeedSource::Home { mode: HomeMode::Following }, PageRequest::First),
            )
            .unwrap();
        page.items.iter().flat_map(FeedItem::posts).take(n).cloned().collect()
    }

    #[test]
    fn ancestors_and_focus_form_one_thread() {
        let p = posts(4);
        let view = ThreadView {
            ancestors: vec![p[0].clone(), p[1].clone()],
            focus: p[2].clone(),
            replies: vec![FeedItem::Post(p[3].clone())],
            more_replies: None,
        };
        let rows = rows(&view);
        assert_eq!(rows.len(), 2);
        let FeedItem::Thread(chain) = &rows[0] else { panic!("thread first") };
        assert_eq!(chain.last().map(|p| &p.id), Some(focus_id(&view)));
        assert_eq!(chain.len(), 3);
    }

    #[test]
    fn a_post_without_ancestors_stands_alone() {
        let p = posts(1);
        let view = ThreadView {
            ancestors: vec![],
            focus: p[0].clone(),
            replies: vec![],
            more_replies: None,
        };
        assert_eq!(rows(&view), vec![FeedItem::Post(p[0].clone())]);
    }
}
