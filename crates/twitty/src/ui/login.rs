//! The sign-in dialog: paste `auth_token` and `ct0`, checked with
//! `Backend::viewer` before anything is stored.

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
    check: Option<Task<()>>,
    _subs: Vec<Subscription>,
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
        auth_token.update(cx, |s, cx| s.focus(window, cx));
        Self { form: LoginForm::new(notice), services, auth_token, ct0, check: None, _subs: subs }
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

    /// Checks the pasted values with the backend.
    pub fn submit(&mut self, cx: &mut Context<Self>) {
        let auth_token = self.auth_token.read(cx).value();
        let ct0 = self.ct0.read(cx).value();
        let Some(credentials) = self.form.submit(&auth_token, &ct0) else {
            cx.notify();
            return;
        };
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

impl Render for LoginView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let checking = self.form.status() == &LoginStatus::Checking;
        let step = |n: &'static str, text: &'static str| {
            h_flex()
                .gap_2()
                .items_start()
                .child(div().flex_none().w(px(16.)).text_color(theme.primary).child(n))
                .child(div().flex_1().child(text))
        };
        let field = |label: &'static str, state: &Entity<InputState>| {
            v_flex()
                .gap_1()
                .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child(label))
                .child(Input::new(state).disabled(checking))
        };
        v_flex()
            .w(px(460.))
            .gap_4()
            .p_6()
            .rounded_2xl()
            .border_1()
            .border_color(theme.border)
            .bg(theme.popover)
            .text_color(theme.foreground)
            .child(div().text_xl().font_weight(FontWeight::BOLD).child("Sign in to X"))
            .when_some(self.form.notice().map(str::to_owned), |d, notice| {
                d.child(div().text_sm().text_color(theme.warning).child(notice))
            })
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
            .child(field("ct0", &self.ct0))
            .when_some(
                match self.form.status() {
                    LoginStatus::Failed(message) => Some(message.clone()),
                    _ => None,
                },
                |d, message| d.child(div().text_sm().text_color(theme.danger).child(message)),
            )
            .child(
                h_flex()
                    .justify_end()
                    .gap_2()
                    .child(
                        Button::new("login-cancel")
                            .ghost()
                            .label("Cancel")
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(LoginEvent::Dismissed))),
                    )
                    .child(
                        Button::new("login-submit")
                            .primary()
                            .label(if checking { "Checking…" } else { "Sign in" })
                            .loading(checking)
                            .on_click(cx.listener(|this, _, _, cx| this.submit(cx))),
                    ),
            )
            .child(div().text_xs().text_color(theme.muted_foreground).child(
                "The cookies are kept in Windows Credential Manager. Using X's internal API is \
                 against X's terms; a secondary account is safer.",
            ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::account::{ConnectError, MemoryStore};
    use gpui_kit::{TestAppContext, VisualTestContext};
    use twitty_model::mock::MockBackend;

    fn services(backend: MockBackend, store: MemoryStore) -> AccountServices {
        AccountServices {
            connect: Arc::new(move |c: &Credentials| {
                if c.auth_token == "bad;value" {
                    return Err(ConnectError::Malformed("bad".into()));
                }
                Ok(Arc::new(backend.clone()) as Arc<dyn Backend>)
            }),
            store: Arc::new(store),
        }
    }

    fn open(
        cx: &mut TestAppContext,
        backend: MockBackend,
        store: MemoryStore,
    ) -> (Entity<LoginView>, &mut VisualTestContext, Arc<std::sync::Mutex<Vec<String>>>) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            cx.set_global(Runtime::inline());
        });
        let events = Arc::new(std::sync::Mutex::new(Vec::new()));
        let (view, cx) = cx.add_window_view(|window, cx| {
            LoginView::new(services(backend, store), None, window, cx)
        });
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
}
