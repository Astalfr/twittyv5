//! System notifications for new rows. GPUI's `SystemNotification` has only a
//! title and a text, so on Windows Twitty builds its own toasts, with the
//! author's avatar as the picture. Without an installed notifier (tests by
//! default, other platforms) they go through GPUI's
//! `show_system_notification`, without a picture.

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::Context as _;
use futures::AsyncReadExt as _;
use gpui_kit::{
    App, AppContext as _, AsyncApp, Global, SystemNotification, http_client::AsyncBody,
};
use sha2::Digest as _;
use twitty_model::ImageRef;

/// One notification, ready to show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Toast {
    /// A newer toast with the same tag replaces this one.
    pub tag: String,
    pub title: String,
    /// The text under the title, one entry per line.
    pub lines: Vec<String>,
    /// The picture, a local file: Windows doesn't fetch web images for an
    /// app without a package.
    pub picture: Option<PathBuf>,
}

/// Something that shows system notifications.
pub trait Notifier: Send + Sync {
    fn show(&self, toast: &Toast);
}

/// The app's notifier, as a GPUI global, and where avatars are kept for it.
#[derive(Clone)]
pub struct Notifications {
    pub notifier: Arc<dyn Notifier>,
    /// The folder the pictures are downloaded to.
    pub pictures: PathBuf,
}

impl Global for Notifications {}

impl std::fmt::Debug for Notifications {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Notifications").field("pictures", &self.pictures).finish_non_exhaustive()
    }
}

/// Shows `toast` (its picture still unset) with `avatar` as the picture. The
/// avatar is downloaded first, once; without it the toast still shows.
pub fn show(mut toast: Toast, avatar: Option<&ImageRef>, cx: &mut App) {
    let Some(notifications) = cx.try_global::<Notifications>().cloned() else {
        cx.show_system_notification(SystemNotification {
            tag: toast.tag.into(),
            title: toast.title.into(),
            body: toast.lines.join("\n").into(),
            actions: Vec::new(),
        });
        return;
    };
    let Some(avatar) = avatar else {
        notifications.notifier.show(&toast);
        return;
    };
    let url = avatar.url.to_string();
    cx.spawn(async move |cx| {
        match picture(&notifications.pictures, &url, cx).await {
            Ok(path) => toast.picture = Some(path),
            Err(e) => tracing::warn!("notification without a picture: {e:#}"),
        }
        notifications.notifier.show(&toast);
    })
    .detach();
}

/// The local copy of the image at `url`, downloaded if it isn't there yet.
/// Named after the URL, which changes with the picture.
async fn picture(dir: &Path, url: &str, cx: &mut AsyncApp) -> anyhow::Result<PathBuf> {
    let path = dir.join(picture_name(url));
    let exists = {
        let path = path.clone();
        cx.background_spawn(async move { path.is_file() }).await
    };
    if exists {
        return Ok(path);
    }
    let client = cx.update(|cx| cx.http_client());
    let mut response =
        client.get(url, AsyncBody::empty(), true).await.context("could not fetch the avatar")?;
    anyhow::ensure!(response.status().is_success(), "the avatar answered {}", response.status());
    let mut bytes = Vec::new();
    response.body_mut().read_to_end(&mut bytes).await.context("could not read the avatar")?;
    let (dir, target) = (dir.to_owned(), path.clone());
    cx.background_spawn(async move {
        std::fs::create_dir_all(&dir)?;
        // Written whole, then renamed: a toast never sees half a file.
        let partial = target.with_extension("part");
        std::fs::write(&partial, bytes)?;
        std::fs::rename(&partial, &target)
    })
    .await
    .with_context(|| format!("could not save the avatar to {}", path.display()))?;
    Ok(path)
}

/// The file name of the copy of `url`: a hash, and its image extension.
fn picture_name(url: &str) -> String {
    let digest = sha2::Sha256::digest(url.as_bytes());
    let hash: String = digest[..16].iter().map(|b| format!("{b:02x}")).collect();
    let path = url.split(['?', '#']).next().unwrap_or_default();
    let ext = path
        .rsplit_once('.')
        .map(|(_, ext)| ext.to_ascii_lowercase())
        .filter(|ext| matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "gif"))
        .unwrap_or_else(|| "jpg".to_owned());
    format!("{hash}.{ext}")
}

/// `path` as the `file:///` URI a toast's image takes.
pub fn file_uri(path: &Path) -> String {
    let mut uri = String::from("file:///");
    for c in path.to_string_lossy().chars() {
        match c {
            '\\' => uri.push('/'),
            ' ' => uri.push_str("%20"),
            '%' => uri.push_str("%25"),
            '#' => uri.push_str("%23"),
            c => uri.push(c),
        }
    }
    uri
}

/// Windows toasts, shown as the app identity `main.rs` gives GPUI.
#[cfg(windows)]
#[derive(Debug)]
pub struct WindowsToasts {
    app_id: String,
}

#[cfg(windows)]
impl WindowsToasts {
    /// Toasts from `app_id`, named `app_name` in Windows' notification list.
    /// Registers the id for the current user, like GPUI does before its own
    /// first toast: an app without a package and its id unregistered shows
    /// nothing.
    pub fn new(app_id: &str, app_name: &str) -> Self {
        let registered = windows_registry::CURRENT_USER
            .create(format!(r"Software\Classes\AppUserModelId\{app_id}"))
            .and_then(|key| key.set_string("DisplayName", app_name));
        if let Err(e) = registered {
            tracing::warn!("could not register the notification identity: {e}");
        }
        Self { app_id: app_id.to_owned() }
    }

    fn try_show(&self, toast: &Toast) -> windows::core::Result<()> {
        use windows::UI::Notifications::{ToastNotification, ToastNotificationManager};

        let document = toast_document(toast)?;
        let notification = ToastNotification::CreateToastNotification(&document)?;
        // A toast with the same tag and group replaces the older one.
        notification.SetTag(&toast.tag.as_str().into())?;
        notification.SetGroup(windows::core::h!("twitty"))?;
        let notifier =
            ToastNotificationManager::CreateToastNotifierWithId(&self.app_id.as_str().into())?;
        notifier.Show(&notification)
    }
}

#[cfg(windows)]
impl Notifier for WindowsToasts {
    fn show(&self, toast: &Toast) {
        if let Err(e) = self.try_show(toast) {
            tracing::warn!("could not show a notification: {e}");
        }
    }
}

/// The toast's XML. Text goes in through the DOM, so a tweet's `<` or `&`
/// stays text.
#[cfg(windows)]
fn toast_document(toast: &Toast) -> windows::core::Result<windows::Data::Xml::Dom::XmlDocument> {
    use windows::{Data::Xml::Dom::XmlDocument, core::h};

    let document = XmlDocument::new()?;
    let root = document.CreateElement(h!("toast"))?;
    document.AppendChild(&root)?;
    let visual = document.CreateElement(h!("visual"))?;
    root.AppendChild(&visual)?;
    let binding = document.CreateElement(h!("binding"))?;
    binding.SetAttribute(h!("template"), h!("ToastGeneric"))?;
    visual.AppendChild(&binding)?;
    for text in texts(toast) {
        let element = document.CreateElement(h!("text"))?;
        element.SetInnerText(&text.as_str().into())?;
        binding.AppendChild(&element)?;
    }
    if let Some(picture) = &toast.picture {
        let image = document.CreateElement(h!("image"))?;
        image.SetAttribute(h!("placement"), h!("appLogoOverride"))?;
        image.SetAttribute(h!("hint-crop"), h!("circle"))?;
        image.SetAttribute(h!("src"), &file_uri(picture).as_str().into())?;
        binding.AppendChild(&image)?;
    }
    // The column plays its own sound, at its own volume.
    let audio = document.CreateElement(h!("audio"))?;
    audio.SetAttribute(h!("silent"), h!("true"))?;
    root.AppendChild(&audio)?;
    Ok(document)
}

/// A toast shows at most three texts: the title, then two lines. Further
/// lines join the last one.
#[cfg_attr(
    all(not(windows), not(test)),
    expect(dead_code, reason = "only Windows toasts have the limit")
)]
fn texts(toast: &Toast) -> Vec<String> {
    const MAX_LINES: usize = 2;
    let mut texts = vec![toast.title.clone()];
    let (first, rest) = toast.lines.split_at(toast.lines.len().min(MAX_LINES - 1));
    texts.extend(first.iter().cloned());
    if !rest.is_empty() {
        texts.push(rest.join("\n"));
    }
    texts
}

/// A notifier that records what it was asked to show, for tests.
#[cfg(test)]
#[derive(Debug, Default)]
pub struct RecordingNotifier(std::sync::Mutex<Vec<Toast>>);

#[cfg(test)]
impl RecordingNotifier {
    /// Installs a recorder as the app's notifier, keeping pictures in
    /// `pictures`, and returns it.
    pub fn install(pictures: &Path, cx: &mut App) -> Arc<Self> {
        let recorder = Arc::new(Self::default());
        cx.set_global(Notifications { notifier: recorder.clone(), pictures: pictures.to_owned() });
        recorder
    }

    /// The toasts shown so far.
    pub fn shown(&self) -> Vec<Toast> {
        self.0.lock().map(|v| v.clone()).unwrap_or_default()
    }
}

#[cfg(test)]
impl Notifier for RecordingNotifier {
    fn show(&self, toast: &Toast) {
        if let Ok(mut shown) = self.0.lock() {
            shown.push(toast.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::{TestAppContext, http_client::FakeHttpClient};
    use pretty_assertions::assert_eq;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn toast(lines: &[&str]) -> Toast {
        Toast {
            tag: "column-1".into(),
            title: "Home - Alice @alice".into(),
            lines: lines.iter().map(|l| (*l).to_owned()).collect(),
            picture: None,
        }
    }

    fn avatar(url: &str) -> ImageRef {
        ImageRef::new(url.parse().unwrap())
    }

    /// Serves `status` with a few bytes, and counts the requests.
    fn serve(status: u16, cx: &mut TestAppContext) -> Arc<AtomicUsize> {
        let requests = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&requests);
        cx.update(|cx| {
            cx.set_http_client(FakeHttpClient::create(move |_| {
                counted.fetch_add(1, Ordering::SeqCst);
                async move {
                    Ok(gpui_kit::http_client::Response::builder()
                        .status(status)
                        .body(AsyncBody::from(b"jpeg bytes".to_vec()))?)
                }
            }));
        });
        requests
    }

    #[gpui_kit::test]
    fn the_avatar_is_downloaded_once_and_shown(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let requests = serve(200, cx);
        let recorder = cx.update(|cx| RecordingNotifier::install(dir.path(), cx));
        let pic = avatar("https://pbs.twimg.com/profile_images/1/a_bigger.jpg");
        for _ in 0..2 {
            cx.update(|cx| show(toast(&["hello"]), Some(&pic), cx));
            cx.run_until_parked();
        }

        let shown = recorder.shown();
        assert_eq!(shown.len(), 2);
        let path = shown[0].picture.clone().expect("a picture");
        assert_eq!(path.parent(), Some(dir.path()));
        assert_eq!(path.extension().unwrap(), "jpg");
        assert_eq!(std::fs::read(&path).unwrap(), b"jpeg bytes");
        assert_eq!(shown[1].picture.as_ref(), Some(&path));
        assert_eq!(requests.load(Ordering::SeqCst), 1, "the second toast reuses the copy");
        assert_eq!(shown[0].lines, ["hello"]);
    }

    #[gpui_kit::test]
    fn a_failed_download_still_shows_the_toast(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        serve(404, cx);
        let recorder = cx.update(|cx| RecordingNotifier::install(dir.path(), cx));
        let pic = avatar("https://pbs.twimg.com/profile_images/1/a_bigger.jpg");
        cx.update(|cx| show(toast(&["hello"]), Some(&pic), cx));
        cx.run_until_parked();
        assert_eq!(recorder.shown(), [toast(&["hello"])]);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0, "nothing half saved");
    }

    #[gpui_kit::test]
    fn without_a_notifier_gpui_shows_it(cx: &mut TestAppContext) {
        // Like Windows, GPUI only shows notifications for an identified app.
        cx.update(|cx| cx.set_app_identity("Twitty.Test", "Twitty"));
        let pic = avatar("https://pbs.twimg.com/profile_images/1/a_bigger.jpg");
        cx.update(|cx| show(toast(&["hello", "+1 more tweet"]), Some(&pic), cx));
        let shown = cx.shown_system_notifications();
        assert_eq!(shown.len(), 1);
        assert_eq!(shown[0].tag.as_ref(), "column-1");
        assert_eq!(shown[0].title.as_ref(), "Home - Alice @alice");
        assert_eq!(shown[0].body.as_ref(), "hello\n+1 more tweet");
    }

    #[test]
    fn picture_names_hash_the_url_and_keep_the_image_type() {
        let a = picture_name("https://pbs.twimg.com/profile_images/1/a_bigger.png");
        let b = picture_name("https://pbs.twimg.com/profile_images/2/a_bigger.png");
        assert_ne!(a, b);
        assert!(a.ends_with(".png"), "{a}");
        assert_eq!(a.len(), 32 + ".png".len());
        assert!(picture_name("https://example.test/a?format=webp").ends_with(".jpg"));
        assert!(picture_name("https://example.test/a.JPEG?x=1").ends_with(".jpeg"));
    }

    #[test]
    fn file_uris_use_slashes_and_escape_spaces() {
        assert_eq!(
            file_uri(Path::new(r"C:\Users\A B\Twitty\cache\avatars\x#1.jpg")),
            "file:///C:/Users/A%20B/Twitty/cache/avatars/x%231.jpg"
        );
    }

    #[test]
    fn toasts_fit_in_three_texts() {
        assert_eq!(texts(&toast(&[])), ["Home - Alice @alice"]);
        assert_eq!(texts(&toast(&["a"])), ["Home - Alice @alice", "a"]);
        assert_eq!(texts(&toast(&["a", "b"])), ["Home - Alice @alice", "a", "b"]);
        assert_eq!(texts(&toast(&["a", "b", "c"])), ["Home - Alice @alice", "a", "b\nc"]);
    }

    #[cfg(windows)]
    #[test]
    fn the_xml_keeps_tweet_text_as_text() {
        let mut t = toast(&["1 < 2 & \"quotes\"", "+1 more tweet"]);
        t.picture = Some(PathBuf::from(r"C:\cache\a.jpg"));
        let xml = toast_document(&t).unwrap().GetXml().unwrap().to_string();
        assert!(xml.contains("1 &lt; 2 &amp; \"quotes\""), "{xml}");
        assert!(xml.contains(r#"placement="appLogoOverride""#), "{xml}");
        assert!(xml.contains(r#"src="file:///C:/cache/a.jpg""#), "{xml}");
        assert!(xml.contains(r#"silent="true""#), "{xml}");
    }
}
