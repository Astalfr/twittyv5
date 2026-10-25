//! The sign-in dialog: sign in on x.com in a window, or paste `auth_token` and
//! `ct0`. Either way the session is checked with `Backend::viewer` before
//! anything is stored.

use std::sync::Arc;

use gpui_kit::{
    Context, Entity, EventEmitter, FontWeight, IntoElement, ParentElement, Render, Styled,
    Subscription, Task, Window, component::button::*, component::input::*, component::*, div,
    prelude::*, px,
};
use twitty_model::{Backend, BackendError, Profile};

use crate::{
    account::{AccountServices, Credentials},
    runtime::Runtime,
    state::login::{LoginForm, LoginStatus},
    ui::button::TextButton,
};

/// What the dialog reports to the app.
pub enum LoginEvent {
    /// The session works and was stored.
    SignedIn { backend: Arc<dyn Backend>, viewer: Arc<Profile> },
    /// The user closed the dialog.
    Dismissed,
}

impl std::fmt::Debug for LoginEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LoginEvent::SignedIn { viewer, .. } => write!(f, "SignedIn(@{})", viewer.handle),
            LoginEvent::Dismissed => f.write_str("Dismissed"),
        }
    }
}

impl EventEmitter<LoginEvent> for LoginView {}

/// The sign-in dialog's view.
pub struct LoginView {
    form: LoginForm,
    services: AccountServices,
    auth_token: Entity<InputState>,
    ct0: Entity<InputState>,
    /// The sign-in window, while it is open.
    browsing: Option<Task<()>>,
    check: Option<Task<()>>,
    _subs: Vec<Subscription>,
}

impl Drop for LoginView {
    /// Closing the dialog closes the sign-in window too.
    fn drop(&mut self) {
        if self.form.status() == &LoginStatus::Browsing
            && let Some(browser) = &self.services.browser
        {
            browser.cancel();
        }
    }
}

impl std::fmt::Debug for LoginView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoginView").field("form", &self.form).finish_non_exhaustive()
    }
}

impl LoginView {
    pub fn new(
        services: AccountServices,
        notice: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let input = |placeholder: &'static str, window: &mut Window, cx: &mut Context<Self>| {
            cx.new(|cx| InputState::new(window, cx).placeholder(placeholder).masked(true))
        };
        let auth_token = input("auth_token value", window, cx);
        let ct0 = input("ct0 value", window, cx);
        let on_enter = |this: &mut Self,
                        _: &Entity<InputState>,
                        e: &InputEvent,
                        _: &mut Window,
                        cx: &mut Context<Self>| {
            if matches!(e, InputEvent::PressEnter { .. }) {
                this.submit(cx);
            }
        };
        let subs = vec![
            cx.subscribe_in(&auth_token, window, on_enter),
            cx.subscribe_in(&ct0, window, on_enter),
        ];
        let form = LoginForm::new(notice, services.browser.is_some());
        if form.paste_shown() {
            auth_token.update(cx, |s, cx| s.focus(window, cx));
        }
        Self { form, services, auth_token, ct0, browsing: None, check: None, _subs: subs }
    }

    pub fn form(&self) -> &LoginForm {
        &self.form
    }

    /// Fills both fields (tests, and pasting a whole cookie header later).
    pub fn set_values(
        &mut self,
        auth_token: &str,
        ct0: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (a, c) = (auth_token.to_owned(), ct0.to_owned());
        self.auth_token.update(cx, |s, cx| s.set_value(a, window, cx));
        self.ct0.update(cx, |s, cx| s.set_value(c, window, cx));
    }

    /// Shows or hides the cookie fields.
    pub fn toggle_paste(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.form.toggle_paste();
        if self.form.paste_shown() {
            self.auth_token.update(cx, |s, cx| s.focus(window, cx));
        }
        cx.notify();
    }

    /// Opens the x.com sign-in window, then checks the session it hands back.
    pub fn sign_in_with_browser(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(browser) = self.services.browser.clone() else {
            return;
        };
        if !self.form.start_browser() {
            return;
        }
        // Blocks until the window closes, which can take minutes: not on the
        // network runtime's few workers.
        let window_closed = cx.background_spawn(async move { browser.sign_in() });
        self.browsing = Some(cx.spawn_in(window, async move |this, cx| {
            let result = window_closed.await;
            this.update_in(cx, |this, window, cx| {
                window.activate_window();
                if let Some(credentials) = this.form.browser_finished(result) {
                    this.check(credentials, cx);
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    /// Checks the pasted values with the backend.
    pub fn submit(&mut self, cx: &mut Context<Self>) {
        let auth_token = self.auth_token.read(cx).value();
        let ct0 = self.ct0.read(cx).value();
        let Some(credentials) = self.form.submit(&auth_token, &ct0) else {
            cx.notify();
            return;
        };
        self.check(credentials, cx);
    }

    /// Builds a backend for `credentials` and asks it who is signed in.
    fn check(&mut self, credentials: Credentials, cx: &mut Context<Self>) {
        let backend = match (self.services.connect)(&credentials) {
            Ok(backend) => backend,
            Err(e) => {
                self.form.connect_failed(&e);
                cx.notify();
                return;
            }
        };
        let checking = Arc::clone(&backend);
        let result = Runtime::global(cx).spawn(async move { checking.viewer().await });
        self.check = Some(cx.spawn(async move |this, cx| {
            let result = match result.await {
                Ok(r) => r,
                Err(e) => Err(BackendError::Network(e.to_string())),
            };
            this.update(cx, |this, cx| this.finish(credentials, backend, result, cx)).ok();
        }));
        cx.notify();
    }

    fn finish(
        &mut self,
        credentials: Credentials,
        backend: Arc<dyn Backend>,
        result: Result<Profile, BackendError>,
        cx: &mut Context<Self>,
    ) {
        match result {
            Ok(viewer) => {
                if let Err(e) = self.services.store.save(&credentials) {
                    // Still usable for this run; the next launch asks again.
                    tracing::error!("could not store the session: {e}");
                }
                cx.emit(LoginEvent::SignedIn { backend, viewer: Arc::new(viewer) });
            }
            Err(e) => {
                tracing::warn!("sign-in check failed: {e}");
                self.form.check_failed(&e);
            }
        }
        cx.notify();
    }
}

/// Wide enough for the how-to steps to wrap on two lines at most.
const DIALOG_WIDTH: f32 = 560.0;

impl Render for LoginView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let busy = self.form.busy();
        let browsing = self.form.status() == &LoginStatus::Browsing;
        let checking = self.form.status() == &LoginStatus::Checking;
        let has_browser = self.services.browser.is_some();
        let paste_shown = self.form.paste_shown();
        // `min_w_0` lets a step's text wrap instead of pushing past the dialog.
        let step = |n: &'static str, text: &'static str| {
            h_flex()
                .gap_2()
                .items_start()
                .child(div().flex_none().w(px(16.)).text_color(theme.primary).child(n))
                .child(
                    div()
                        .debug_selector(move || format!("login-step-{n}"))
                        .flex_1()
                        .min_w_0()
                        .child(text),
                )
        };
        let field = |label: &'static str, state: &Entity<InputState>| {
            v_flex()
                .gap_1()
                .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child(label))
                .child(Input::new(state).disabled(busy))
        };
        let browser = v_flex()
            .gap_2()
            .child(
                Button::new("login-browser")
                    .primary()
                    .text_button(Size::Medium)
                    .label(if browsing { "Waiting for x.com…" } else { "Sign in on x.com" })
                    .loading(browsing)
                    .disabled(busy)
                    .on_click(
                        cx.listener(|this, _, window, cx| this.sign_in_with_browser(window, cx)),
                    ),
            )
            .child(div().text_sm().text_color(theme.muted_foreground).child(if browsing {
                "Finish signing in in the x.com window. It closes by itself once you're in."
            } else {
                "Opens x.com in a window. Use your username, email or phone and password: \
                 Google and Apple sign-in don't work there. Twitty keeps the session, never \
                 your password."
            }));
        let paste = v_flex()
            .gap_4()
            .child(
                v_flex()
                    .gap_2()
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .child(step("1", "Open x.com in your browser and sign in."))
                    .child(step(
                        "2",
                        "Press F12, then Application (Chrome, Edge) or Storage (Firefox) → Cookies → https://x.com.",
                    ))
                    .child(step(
                        "3",
                        "Copy the values of auth_token and ct0 here. auth_token is HttpOnly, so it only shows in DevTools.",
                    )),
            )
            .child(field("auth_token", &self.auth_token))
            .child(field("ct0", &self.ct0));
        v_flex()
            .debug_selector(|| "login-dialog".into())
            .w(px(DIALOG_WIDTH))
            .gap_4()
            .p_6()
            .rounded_2xl()
            .border_1()
            .border_color(theme.border)
            .bg(theme.popover)
            .text_color(theme.foreground)
            .child(div().text_xl().font_weight(FontWeight::BOLD).child("Sign in to Twitter"))
            .when_some(self.form.notice().map(str::to_owned), |d, notice| {
                d.child(div().text_sm().text_color(theme.warning).child(notice))
            })
            .when(has_browser, |d| {
                d.child(browser).child(
                    h_flex().child(
                        Button::new("login-paste-toggle")
                            .ghost()
                            .text_button(Size::Small)
                            .label(if paste_shown {
                                "Hide the cookie fields"
                            } else {
                                "Paste cookies instead"
                            })
                            .on_click(
                                cx.listener(|this, _, window, cx| this.toggle_paste(window, cx)),
                            ),
                    ),
                )
            })
            .when(paste_shown, |d| d.child(paste))
            .when_some(
                match self.form.status() {
                    LoginStatus::Failed(message) => Some(message.clone()),
                    _ => None,
                },
                |d, message| d.child(div().text_sm().text_color(theme.danger).child(message)),
            )
            .child(
                div()
                    .debug_selector(|| "login-footnote".into())
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(
                        "The session is kept in Windows Credential Manager. Using Twitter's \
                         internal API is against Twitter's terms; a secondary account is safer.",
                    ),
            )
            .child(
                h_flex()
                    .justify_end()
                    .gap_2()
                    .child(
                        div().debug_selector(|| "login-cancel".into()).child(
                            Button::new("login-cancel")
                                .ghost()
                                .text_button(Size::Medium)
                                .label("Cancel")
                                .on_click(
                                    cx.listener(|_, _, _, cx| cx.emit(LoginEvent::Dismissed)),
                                ),
                        ),
                    )
                    .when(paste_shown, |d| {
                        let submit = Button::new("login-submit");
                        let submit = if has_browser { submit.outline() } else { submit.primary() };
                        d.child(
                            div().debug_selector(|| "login-submit".into()).child(
                                submit
                                    .text_button(Size::Medium)
                                    .label(if checking { "Checking…" } else { "Sign in" })
                                    .loading(checking)
                                    .disabled(browsing)
                                    .on_click(cx.listener(|this, _, _, cx| this.submit(cx))),
                            ),
                        )
                    }),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        account::{ConnectError, MemoryStore},
        browser_login::{BrowserError, BrowserSignIn},
    };
    use gpui_kit::{TestAppContext, VisualTestContext};
    use std::sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    };
    use twitty_model::mock::MockBackend;

    /// A sign-in window that closes at once with a set result.
    struct FakeBrowser {
        result: Result<Option<Credentials>, BrowserError>,
        cancels: Arc<AtomicUsize>,
    }

    impl BrowserSignIn for FakeBrowser {
        fn sign_in(&self) -> Result<Option<Credentials>, BrowserError> {
            self.result.clone()
        }
        fn cancel(&self) {
            self.cancels.fetch_add(1, Ordering::SeqCst);
        }
    }

    fn services(backend: MockBackend, store: MemoryStore) -> AccountServices {
        services_with(backend, store, None)
    }

    fn services_with(
        backend: MockBackend,
        store: MemoryStore,
        browser: Option<Arc<dyn BrowserSignIn>>,
    ) -> AccountServices {
        AccountServices {
            connect: Arc::new(move |c: &Credentials| {
                if c.auth_token == "bad;value" {
                    return Err(ConnectError::Malformed("bad".into()));
                }
                Ok(Arc::new(backend.clone()) as Arc<dyn Backend>)
            }),
            store: Arc::new(store),
            browser,
        }
    }

    fn open(
        cx: &mut TestAppContext,
        backend: MockBackend,
        store: MemoryStore,
    ) -> (Entity<LoginView>, &mut VisualTestContext, Arc<std::sync::Mutex<Vec<String>>>) {
        open_with(cx, services(backend, store))
    }

    fn open_with(
        cx: &mut TestAppContext,
        services: AccountServices,
    ) -> (Entity<LoginView>, &mut VisualTestContext, Arc<std::sync::Mutex<Vec<String>>>) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            cx.set_global(Runtime::inline());
        });
        let events = Arc::new(std::sync::Mutex::new(Vec::new()));
        let (view, cx) =
            cx.add_window_view(|window, cx| LoginView::new(services, None, window, cx));
        let seen = Arc::clone(&events);
        cx.update(|_, cx| {
            cx.subscribe(&view, move |_, e: &LoginEvent, _| {
                seen.lock().unwrap().push(match e {
                    LoginEvent::SignedIn { viewer, .. } => format!("signed in @{}", viewer.handle),
                    LoginEvent::Dismissed => "dismissed".into(),
                });
            })
            .detach();
        });
        (view, cx, events)
    }

    #[gpui_kit::test]
    fn the_steps_wrap_inside_the_dialog(cx: &mut TestAppContext) {
        let (_view, cx, _) = open(cx, MockBackend::new(), MemoryStore::default());
        cx.run_until_parked();
        let dialog = cx.debug_bounds("login-dialog").expect("the dialog is drawn");
        assert_eq!(dialog.size.width, px(DIALOG_WIDTH));
        let one = cx.debug_bounds("login-step-1").expect("step 1");
        for name in ["login-step-1", "login-step-2", "login-step-3"] {
            let step = cx.debug_bounds(name).unwrap_or_else(|| panic!("{name} is drawn"));
            assert!(step.right() <= dialog.right(), "{name} overflows: {step:?} {dialog:?}");
        }
        let long = cx.debug_bounds("login-step-2").expect("step 2");
        assert!(long.size.height > one.size.height, "the long step wraps: {long:?} {one:?}");
    }

    #[gpui_kit::test]
    fn valid_cookies_are_checked_then_stored(cx: &mut TestAppContext) {
        let store = MemoryStore::default();
        let (view, cx, events) = open(cx, MockBackend::new(), store.clone());
        view.update_in(cx, |v, window, cx| {
            v.set_values(" auth_token=tok ", "\"csrf\"", window, cx)
        });
        view.update(cx, |v, cx| v.submit(cx));
        cx.run_until_parked();

        assert_eq!(events.lock().unwrap().len(), 1);
        assert!(events.lock().unwrap()[0].starts_with("signed in @"));
        let saved = crate::account::decode(&store.raw().unwrap()).unwrap();
        assert_eq!((saved.auth_token.as_str(), saved.ct0.as_str()), ("tok", "csrf"));
    }

    #[gpui_kit::test]
    fn rejected_session_is_not_stored(cx: &mut TestAppContext) {
        let backend = MockBackend::new();
        backend.fail_next(BackendError::Unauthorized);
        let store = MemoryStore::default();
        let (view, cx, events) = open(cx, backend, store.clone());
        view.update_in(cx, |v, window, cx| v.set_values("tok", "csrf", window, cx));
        view.update(cx, |v, cx| v.submit(cx));
        cx.run_until_parked();

        assert!(events.lock().unwrap().is_empty());
        assert_eq!(store.raw(), None);
        assert!(
            matches!(view.read_with(cx, |v, _| v.form().status().clone()), LoginStatus::Failed(m) if m.contains("didn't accept"))
        );
    }

    #[gpui_kit::test]
    fn malformed_values_never_reach_the_backend(cx: &mut TestAppContext) {
        let store = MemoryStore::default();
        let (view, cx, events) = open(cx, MockBackend::new(), store.clone());
        view.update_in(cx, |v, window, cx| v.set_values("bad;value", "csrf", window, cx));
        view.update(cx, |v, cx| v.submit(cx));
        cx.run_until_parked();
        assert!(events.lock().unwrap().is_empty());
        assert_eq!(store.raw(), None);
        assert!(matches!(
            view.read_with(cx, |v, _| v.form().status().clone()),
            LoginStatus::Failed(_)
        ));
    }

    fn with_browser(
        cx: &mut TestAppContext,
        backend: MockBackend,
        store: MemoryStore,
        result: Result<Option<Credentials>, BrowserError>,
    ) -> (Entity<LoginView>, &mut VisualTestContext, Arc<Mutex<Vec<String>>>) {
        let browser: Arc<dyn BrowserSignIn> =
            Arc::new(FakeBrowser { result, cancels: Arc::default() });
        open_with(cx, services_with(backend, store, Some(browser)))
    }

    fn status(view: &Entity<LoginView>, cx: &mut VisualTestContext) -> LoginStatus {
        view.read_with(cx, |v, _| v.form().status().clone())
    }

    fn browse(view: &Entity<LoginView>, cx: &mut VisualTestContext) {
        view.update_in(cx, |v, window, cx| v.sign_in_with_browser(window, cx));
        cx.run_until_parked();
    }

    #[gpui_kit::test]
    fn cancel_comes_last_below_the_footnote(cx: &mut TestAppContext) {
        let (_view, cx, _) = open(cx, MockBackend::new(), MemoryStore::default());
        cx.run_until_parked();
        let footnote = cx.debug_bounds("login-footnote").expect("the footnote is drawn");
        let cancel = cx.debug_bounds("login-cancel").expect("cancel is drawn");
        let dialog = cx.debug_bounds("login-dialog").expect("the dialog is drawn");
        assert!(cancel.top() >= footnote.bottom(), "{cancel:?} below {footnote:?}");
        // No sign-in window here, so the cookie fields and their button are shown.
        let submit = cx.debug_bounds("login-submit").expect("sign in is drawn");
        assert_eq!(submit.top(), cancel.top(), "side by side: {submit:?} {cancel:?}");
        assert!(submit.left() >= cancel.right(), "sign in comes after cancel");
        assert!(submit.right() <= dialog.right(), "{submit:?} inside {dialog:?}");
    }

    #[gpui_kit::test]
    fn the_cookie_fields_fold_away_behind_the_window(cx: &mut TestAppContext) {
        let session = Credentials { auth_token: "tok".into(), ct0: "csrf".into() };
        let (view, cx, _) =
            with_browser(cx, MockBackend::new(), MemoryStore::default(), Ok(Some(session)));
        cx.run_until_parked();
        assert!(cx.debug_bounds("login-step-1").is_none(), "folded at first");
        view.update_in(cx, |v, window, cx| v.toggle_paste(window, cx));
        cx.run_until_parked();
        assert!(cx.debug_bounds("login-step-1").is_some(), "shown on request");
    }

    #[gpui_kit::test]
    fn window_session_is_checked_then_stored(cx: &mut TestAppContext) {
        let store = MemoryStore::default();
        let session = Credentials { auth_token: "tok".into(), ct0: "csrf".into() };
        let (view, cx, events) =
            with_browser(cx, MockBackend::new(), store.clone(), Ok(Some(session.clone())));
        browse(&view, cx);
        assert_eq!(events.lock().unwrap().len(), 1);
        assert!(events.lock().unwrap()[0].starts_with("signed in @"));
        assert_eq!(crate::account::decode(&store.raw().unwrap()).unwrap(), session);
    }

    #[gpui_kit::test]
    fn closing_the_window_stores_nothing(cx: &mut TestAppContext) {
        let store = MemoryStore::default();
        let (view, cx, events) = with_browser(cx, MockBackend::new(), store.clone(), Ok(None));
        browse(&view, cx);
        assert!(events.lock().unwrap().is_empty());
        assert_eq!(store.raw(), None);
        assert_eq!(status(&view, cx), LoginStatus::Editing);
    }

    #[gpui_kit::test]
    fn a_broken_window_offers_pasting(cx: &mut TestAppContext) {
        let failed = Err(BrowserError::Helper("WebView2 is unavailable".into()));
        let (view, cx, events) =
            with_browser(cx, MockBackend::new(), MemoryStore::default(), failed);
        browse(&view, cx);
        assert!(events.lock().unwrap().is_empty());
        assert!(matches!(status(&view, cx), LoginStatus::Failed(m) if m.contains("WebView2")));
        assert!(cx.debug_bounds("login-step-1").is_some(), "the cookie fields are shown");
    }

    #[gpui_kit::test]
    fn rejected_window_session_is_not_stored(cx: &mut TestAppContext) {
        let backend = MockBackend::new();
        backend.fail_next(BackendError::Unauthorized);
        let store = MemoryStore::default();
        let session = Credentials { auth_token: "tok".into(), ct0: "csrf".into() };
        let (view, cx, events) = with_browser(cx, backend, store.clone(), Ok(Some(session)));
        browse(&view, cx);
        assert!(events.lock().unwrap().is_empty());
        assert_eq!(store.raw(), None);
        assert!(
            matches!(status(&view, cx), LoginStatus::Failed(m) if m.contains("signing in again"))
        );
    }

    #[gpui_kit::test]
    fn closing_the_dialog_closes_the_window(cx: &mut TestAppContext) {
        let cancels = Arc::new(AtomicUsize::new(0));
        let browser: Arc<dyn BrowserSignIn> =
            Arc::new(FakeBrowser { result: Ok(None), cancels: Arc::clone(&cancels) });
        let services = services_with(MockBackend::new(), MemoryStore::default(), Some(browser));
        let (view, cx, _) = open_with(cx, services);
        // Not parked: the window is still open.
        view.update_in(cx, |v, window, cx| v.sign_in_with_browser(window, cx));
        assert_eq!(status(&view, cx), LoginStatus::Browsing);
        drop(view);
        cx.update(|window, _| window.remove_window());
        cx.run_until_parked();
        assert_eq!(cancels.load(Ordering::SeqCst), 1);
    }

    #[gpui_kit::test]
    fn a_finished_window_is_not_cancelled(cx: &mut TestAppContext) {
        let cancels = Arc::new(AtomicUsize::new(0));
        let browser: Arc<dyn BrowserSignIn> =
            Arc::new(FakeBrowser { result: Ok(None), cancels: Arc::clone(&cancels) });
        let services = services_with(MockBackend::new(), MemoryStore::default(), Some(browser));
        let (view, cx, _) = open_with(cx, services);
        browse(&view, cx);
        drop(view);
        cx.update(|window, _| window.remove_window());
        cx.run_until_parked();
        assert_eq!(cancels.load(Ordering::SeqCst), 0);
    }
}
