//! Shareable links to posts.

use twitty_model::Post;

/// Host of the embed-friendly mirror used for shared post links.
const VX_HOST: &str = "vxtwitter.com";

/// The post's canonical link with its host swapped for vxtwitter, so it
/// embeds properly when pasted into chat apps. Falls back to the canonical
/// link if the host can't be replaced.
pub fn vx_link(post: &Post) -> String {
    let mut url = post.url.clone();
    match url.set_host(Some(VX_HOST)) {
        Ok(()) => url.into(),
        Err(_) => post.url.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use twitty_model::{Backend, FeedSource, PageRequest, mock::MockBackend};

    fn mock_post() -> Post {
        let page = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(MockBackend::new().feed(&FeedSource::Bookmarks, PageRequest::First))
            .unwrap();
        page.items.iter().find_map(|i| i.posts().first().map(|p| (*p).clone())).unwrap()
    }

    #[test]
    fn swaps_the_host_and_keeps_the_path() {
        let mut post = mock_post();
        post.url = "https://x.com/ferris/status/123".parse().unwrap();
        assert_eq!(vx_link(&post), "https://vxtwitter.com/ferris/status/123");
        post.url = "https://twitter.com/ferris/status/456?s=20".parse().unwrap();
        assert_eq!(vx_link(&post), "https://vxtwitter.com/ferris/status/456?s=20");
    }

    #[test]
    fn mock_posts_link_to_vxtwitter() {
        let post = mock_post();
        let link = vx_link(&post);
        assert!(link.starts_with("https://vxtwitter.com/"), "{link}");
        assert!(link.ends_with(post.url.path()), "{link} keeps {}", post.url.path());
    }
}
