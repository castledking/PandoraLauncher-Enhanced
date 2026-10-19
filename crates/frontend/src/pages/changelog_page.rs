use bridge::message::MessageToBackend;
use gpui::{prelude::*, *};
use gpui_component::{
    ActiveTheme, Sizable, StyledExt,
    button::{Button, ButtonVariants},
    h_flex,
    scroll::ScrollableElement,
    spinner::Spinner,
    tag::Tag,
    text::TextView,
    v_flex,
};
use rustc_hash::FxHashSet;
use schema::forge::VersionFragment;

use crate::{entity::DataEntities, icon::PandoraIcon, modals::update_prompt, pages::page::Page};

/// The version of this launcher, which is empty in a build that isn't a release.
const CURRENT_VERSION: &str = match option_env!("PANDORA_RELEASE_VERSION") {
    Some(version) => version,
    None => "",
};

/// The heading the release notes put their technical details under.
const TECHNICAL_HEADING: &str = "## Technical Details";

/// How many releases are shown at first, and how many more each "show older" adds.
const RELEASES_PER_STEP: usize = 10;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Standing {
    /// Newer than this launcher, so it's what updating brings.
    New,
    /// The version that's running.
    Installed,
    Older,
}

struct Release {
    standing: Standing,
    title: SharedString,
    published: Option<SharedString>,
    url: SharedString,
    summary: SharedString,
    /// Written for whoever works on the launcher, so it starts out folded away.
    technical: Option<SharedString>,
}

impl Release {
    fn new(notes: &schema::pandora_update::ReleaseNotes, current: &[VersionFragment]) -> Self {
        // A build that isn't a release can't tell where it stands
        let version = VersionFragment::string_to_parts(&notes.version);
        let standing = if current.is_empty() {
            Standing::Older
        } else {
            match version.as_slice().cmp(current) {
                std::cmp::Ordering::Greater => Standing::New,
                std::cmp::Ordering::Equal => Standing::Installed,
                std::cmp::Ordering::Less => Standing::Older,
            }
        };

        let body = notes.body.replace("\r\n", "\n");
        // The notes start with the version as their title, which the page already shows
        let body = match body.strip_prefix("# ") {
            Some(rest) => rest.split_once('\n').map_or("", |(_, rest)| rest),
            None => &body,
        };

        let (summary, technical) = match body.find(&format!("\n{TECHNICAL_HEADING}")) {
            Some(start) => {
                let (summary, technical) = body.split_at(start);
                let summary = summary.trim_end();
                let summary = summary.strip_suffix("---").unwrap_or(summary);
                let technical = technical.trim_start().trim_start_matches(TECHNICAL_HEADING);
                (summary, Some(technical.trim()))
            },
            None => (body, None),
        };

        Self {
            standing,
            title: SharedString::from(&*notes.title),
            published: notes.published.as_deref().map(SharedString::from),
            url: SharedString::from(&*notes.url),
            summary: summary.trim().to_string().into(),
            technical: technical
                .filter(|technical| !technical.is_empty())
                .map(|technical| technical.to_string().into()),
        }
    }
}

enum Notes {
    Loading,
    Failed(SharedString),
    Loaded(Vec<Release>),
}

/// The notes of every release, with what updating would bring marked as new.
pub struct ChangelogPage {
    data: DataEntities,
    notes: Notes,
    /// How many releases are showing, newest first.
    shown: usize,
    /// The releases whose technical details are showing.
    expanded: FxHashSet<usize>,
    _load_task: Task<()>,
}

impl ChangelogPage {
    pub fn new(data: &DataEntities, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        let (send, recv) = tokio::sync::oneshot::channel();
        data.backend_handle.send(MessageToBackend::GetReleaseNotes { channel: send });

        let load_task = cx.spawn(async move |page, cx| {
            let current = VersionFragment::string_to_parts(CURRENT_VERSION);
            let notes = match recv.await {
                Ok(Ok(notes)) => Notes::Loaded(notes.iter().map(|notes| Release::new(notes, &current)).collect()),
                Ok(Err(error)) => Notes::Failed(SharedString::from(&*error)),
                Err(_) => Notes::Failed(t::system::update::changelog::error().into()),
            };
            _ = page.update(cx, |page, cx| {
                // Everything an update brings, and the installed version to compare it with
                if let Notes::Loaded(releases) = &notes {
                    let new = releases.iter().filter(|release| release.standing == Standing::New).count();
                    page.shown = page.shown.max(new + 1);
                }
                page.notes = notes;
                cx.notify();
            });
        });

        Self {
            data: data.clone(),
            notes: Notes::Loading,
            shown: RELEASES_PER_STEP,
            expanded: FxHashSet::default(),
            _load_task: load_task,
        }
    }

    fn render_release(&self, index: usize, release: &Release, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let expanded = self.expanded.contains(&index);

        let header = h_flex()
            .gap_4()
            .justify_between()
            .child(
                v_flex()
                    .child(
                        h_flex()
                            .gap_3()
                            .child(div().text_2xl().font_semibold().child(release.title.clone()))
                            .children(match release.standing {
                                Standing::New => Some(Tag::success().child(t::system::update::changelog::new())),
                                Standing::Installed => {
                                    Some(Tag::secondary().child(t::system::update::changelog::installed()))
                                },
                                Standing::Older => None,
                            }),
                    )
                    .children(
                        release
                            .published
                            .clone()
                            .map(|date| div().text_sm().text_color(theme.muted_foreground).child(date)),
                    ),
            )
            .child(
                Button::new(("github", index))
                    .ghost()
                    .small()
                    .icon(PandoraIcon::Github)
                    .label(t::system::update::changelog::open_github())
                    .on_click({
                        let url = release.url.clone();
                        move |_, _, cx| cx.open_url(&url)
                    }),
            );

        let technical = release.technical.clone().map(|technical| {
            let toggle = Button::new(("technical", index))
                .ghost()
                .small()
                .icon(if expanded {
                    PandoraIcon::ChevronDown
                } else {
                    PandoraIcon::ChevronRight
                })
                .label(t::system::update::changelog::technical())
                .on_click(cx.listener(move |page, _, _, cx| {
                    if !page.expanded.remove(&index) {
                        page.expanded.insert(index);
                    }
                    cx.notify();
                }));

            v_flex()
                .gap_3()
                .pt_3()
                .border_t_1()
                .border_color(theme.border)
                .child(h_flex().child(toggle))
                .when(expanded, |this| {
                    this.child(TextView::markdown(("release-technical", index), technical).selectable(true))
                })
        });

        v_flex()
            .gap_4()
            .p_6()
            .border_1()
            .border_color(theme.border)
            .rounded(theme.radius_lg)
            .bg(theme.background)
            .child(header)
            .child(TextView::markdown(("release-notes", index), release.summary.clone()).selectable(true))
            .children(technical)
    }
}

impl Page for ChangelogPage {
    fn controls(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
    }

    fn scrollable(&self, _cx: &App) -> bool {
        false
    }
}

impl Render for ChangelogPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let content = match &self.notes {
            Notes::Loading => h_flex()
                .w_full()
                .h_48()
                .gap_2()
                .items_center()
                .justify_center()
                .child(Spinner::new())
                .child(t::system::update::changelog::loading())
                .into_any_element(),
            Notes::Failed(error) => {
                let releases_url = option_env!("GITHUB_REPOSITORY_URL")
                    .map(|repository| SharedString::from(format!("{repository}/releases")));
                v_flex()
                    .w_full()
                    .h_48()
                    .gap_3()
                    .items_center()
                    .justify_center()
                    .child(div().text_lg().child(t::system::update::changelog::error()))
                    .child(div().text_sm().text_color(cx.theme().muted_foreground).child(error.clone()))
                    .children(releases_url.map(|url| {
                        Button::new("github")
                            .icon(PandoraIcon::Github)
                            .label(t::system::update::changelog::open_github())
                            .on_click(move |_, _, cx| cx.open_url(&url))
                    }))
                    .into_any_element()
            },
            Notes::Loaded(releases) => {
                let mut list = v_flex().gap_6();
                for (index, release) in releases.iter().enumerate().take(self.shown) {
                    list = list.child(self.render_release(index, release, cx));
                }
                if self.shown < releases.len() {
                    list = list.child(
                        h_flex().justify_center().child(
                            Button::new("older")
                                .icon(PandoraIcon::ChevronDown)
                                .label(t::system::update::changelog::older())
                                .on_click(cx.listener(|page, _, _, cx| {
                                    page.shown += RELEASES_PER_STEP;
                                    cx.notify();
                                })),
                        ),
                    );
                }
                list.into_any_element()
            },
        };

        // Only offered while there's an update to be had
        let update = update_prompt::pending_update(cx);

        let footer = h_flex()
            .w_full()
            .px_6()
            .py_3()
            .gap_2()
            .justify_between()
            .border_t_1()
            .border_color(cx.theme().border)
            .child(
                Button::new("back")
                    .icon(PandoraIcon::ArrowLeft)
                    .label(t::system::update::changelog::back())
                    .on_click({
                        let data = self.data.clone();
                        move |_, window, cx| {
                            crate::root::go_back(window, cx);
                            // An update on offer is what the changelog was most likely opened from
                            if let Some(update) = update_prompt::pending_update(cx) {
                                update_prompt::open_update_dialog(update, &data, window, cx);
                            }
                        }
                    }),
            )
            .children(update.map(|update| {
                Button::new("update")
                    .success()
                    .icon(PandoraIcon::Download)
                    .label(t::common::update())
                    .on_click({
                        let data = self.data.clone();
                        move |_, window, cx| update_prompt::start_update(update.clone(), &data, window, cx)
                    })
            }));

        v_flex()
            .size_full()
            .child(
                div().flex_1().min_h_0().overflow_hidden().child(
                    v_flex()
                        .size_full()
                        .overflow_y_scrollbar()
                        .child(div().w_full().max_w(px(900.0)).mx_auto().p_6().child(content)),
                ),
            )
            .child(footer)
    }
}
