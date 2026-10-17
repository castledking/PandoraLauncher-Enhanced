use std::{f64::consts::TAU, sync::Arc, time::Duration};

use bridge::{
    instance::InstanceStatus,
    message::{AccountCapesResult, AccountSkinResult, MessageToBackend},
};
use gpui::{prelude::*, *};
use gpui_component::{ActiveTheme as _, Colorize, Icon, StyledExt, h_flex, switch::Switch, tooltip::Tooltip, v_flex};
use schema::minecraft_profile::{SkinState, SkinVariant};
use uuid::Uuid;

use crate::{
    component::{
        background_effect::{self, AnimationClock, EffectPalette, FrameTicker},
        player_model::{PlayerModel, PlayerModelState},
    },
    data_asset_loader::DataAssetLoader,
    entity::{DataEntities, account::AccountExt, instance::InstanceEntry},
    icon::PandoraIcon,
    interface_config::InterfaceConfig,
    pages::{page::Page, skins_page::DEFAULT_SKIN},
    root,
    skin_renderer::IdlePose,
    ui::PageType,
};

/// The skin's resting angles. Dragging turns it, but only around its vertical axis.
const DEFAULT_YAW: f64 = 18.0;
const PITCH: f64 = 6.0;

/// Where a still background is frozen, picked so every effect has something on screen.
const STILL_BACKGROUND_TIME: f64 = 12.0;

/// How long to wait on an account's skin and cape before standing the default skin in for it.
const SKIN_TIMEOUT: Duration = Duration::from_secs(10);

/// The home screen: the player's skin standing over the launcher's background, with the most
/// recently played instance one click away.
pub struct QuickplayPage {
    data: DataEntities,
    model: Entity<PlayerModelState>,
    /// Which account the shown skin belongs to; `None` before the first render has looked.
    skin_account: Option<Option<Uuid>>,
    /// Whether the model holds the skin it should show: the account's own, or the default skin
    /// when there's none to get. Until then nothing is drawn, so the default doesn't flash up
    /// before the real skin.
    skin_ready: bool,
    skin_request: Option<Task<()>>,
    skin_timeout: Option<Task<()>>,
    /// The active cape, until its image has loaded and been put on the model.
    pending_cape: Option<Arc<str>>,
    background_clock: AnimationClock,
    skin_clock: AnimationClock,
    ticker: FrameTicker,
    last_drag_x: Option<Pixels>,
    _activation_subscription: Subscription,
}

impl QuickplayPage {
    pub fn new(data: &DataEntities, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let model = PlayerModelState::new(cx, DEFAULT_SKIN.clone(), SkinVariant::Classic);
        model.update(cx, |model, _| {
            model.yaw = DEFAULT_YAW;
            model.pitch = PITCH;
            model.animation = 0.0;
        });

        // The animations stop while the launcher is in the background, so they need a nudge to
        // start again once it's back in front
        let _activation_subscription = cx.observe_window_activation(window, |_, _, cx| cx.notify());

        Self {
            data: data.clone(),
            model,
            skin_account: None,
            skin_ready: false,
            skin_request: None,
            skin_timeout: None,
            pending_cape: None,
            background_clock: AnimationClock::starting_at(STILL_BACKGROUND_TIME),
            skin_clock: AnimationClock::default(),
            ticker: FrameTicker::default(),
            last_drag_x: None,
            _activation_subscription,
        }
    }

    /// Puts the selected account's skin and cape on the model. Offline accounts, no account and
    /// hidden skins get the default skin, as does an account whose skin can't be fetched.
    fn sync_skin(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let hide_skins = InterfaceConfig::get(cx).hide_skins;
        let account = self.data.accounts.read(cx).selected_account.clone();
        let wanted = account.filter(|account| !account.offline && !hide_skins).map(|account| account.uuid);

        if self.skin_account != Some(wanted) {
            self.skin_account = Some(wanted);
            self.pending_cape = None;
            match wanted {
                Some(uuid) => {
                    self.skin_ready = false;
                    self.request_skin(uuid, cx);
                },
                None => {
                    self.skin_request = None;
                    self.skin_timeout = None;
                    self.show_default_skin(cx);
                },
            }
        }

        if let Some(url) = self.pending_cape.clone() {
            let uri: SharedUri = SharedString::new(url).into();
            // Nothing yet means it's still downloading; an empty result means it couldn't be
            // fetched, and the skin goes without
            if let Some(cape) = window.use_asset::<DataAssetLoader>(&Resource::Uri(uri), cx) {
                self.pending_cape = None;
                if let Some(cape) = cape {
                    self.model.update(cx, |model, cx| {
                        model.cape = Some(cape);
                        cx.notify();
                    });
                }
            }
        }
    }

    fn show_default_skin(&mut self, cx: &mut Context<Self>) {
        self.skin_ready = true;
        self.model.update(cx, |model, cx| {
            model.skin = DEFAULT_SKIN.clone();
            model.variant = SkinVariant::Classic;
            model.cape = None;
            cx.notify();
        });
    }

    /// Whether the skin can be drawn: it's the one meant to be shown, its cape has arrived if it
    /// has one, and the model has finished rendering them.
    fn skin_visible(&self, cx: &App) -> bool {
        self.skin_ready && self.pending_cape.is_none() && self.model.read(cx).shows_current_skin()
    }

    fn request_skin(&mut self, uuid: Uuid, cx: &mut Context<Self>) {
        let (send_skin, recv_skin) = tokio::sync::oneshot::channel();
        self.data.backend_handle.send(MessageToBackend::GetAccountSkin {
            account: uuid,
            result: send_skin,
        });
        let (send_capes, recv_capes) = tokio::sync::oneshot::channel();
        self.data.backend_handle.send(MessageToBackend::GetAccountCapes {
            account: uuid,
            result: send_capes,
        });

        self.skin_request = Some(cx.spawn(async move |page, cx| {
            let skin = recv_skin.await;
            let capes = recv_capes.await;
            _ = page.update(cx, |page, cx| {
                // The account may have been switched while this was loading
                if page.skin_account != Some(Some(uuid)) {
                    return;
                }
                match skin {
                    Ok(AccountSkinResult::Success {
                        skin: Some(skin),
                        variant,
                    }) => {
                        page.skin_ready = true;
                        page.model.update(cx, |model, cx| {
                            model.skin = skin;
                            model.variant = variant;
                            model.cape = None;
                            cx.notify();
                        });
                    },
                    // No skin set on the account, or it couldn't be fetched
                    _ => page.show_default_skin(cx),
                }
                page.pending_cape = match capes {
                    Ok(AccountCapesResult::Success { capes }) => {
                        capes.iter().find(|cape| cape.state == SkinState::Active).map(|cape| cape.url.clone())
                    },
                    _ => None,
                };
                page.skin_request = None;
                cx.notify();
            });
        }));

        // A request that never answers would otherwise leave an empty space for good. If the real
        // skin does turn up later, it still replaces the stand-in
        self.skin_timeout = Some(cx.spawn(async move |page, cx| {
            cx.background_executor().timer(SKIN_TIMEOUT).await;
            _ = page.update(cx, |page, cx| {
                if page.skin_account != Some(Some(uuid)) {
                    return;
                }
                if !page.skin_ready {
                    page.show_default_skin(cx);
                }
                page.pending_cape = None;
                page.skin_timeout = None;
                cx.notify();
            });
        }));
    }

    /// The instance the launch button starts: whichever was played most recently.
    fn latest_instance(&self, cx: &App) -> Option<InstanceEntry> {
        let entries = self.data.instances.read(cx).entries.values().map(|entry| entry.read(cx));
        latest_of(entries.filter(|entry| entry.is_regular_instance())).cloned()
    }

    fn render_launch_button(&self, instance: Option<InstanceEntry>, cx: &App) -> AnyElement {
        let theme = cx.theme();
        let status = instance.as_ref().map(|instance| instance.status);
        let running = matches!(status, Some(InstanceStatus::Running));
        let busy = matches!(status, Some(InstanceStatus::Launching | InstanceStatus::Stopping));

        let tint = if running { theme.danger } else { theme.blue };
        // Oklab keeps the hue while darkening; mixing in HSL drifts blue towards purple
        let background = tint.mix_oklab(theme.background, 0.4).opacity(0.92);
        let hover = tint.mix_oklab(theme.background, 0.55).opacity(0.95);
        let border = tint.opacity(0.8);

        let label = match status {
            None | Some(InstanceStatus::NotRunning) => t::quickplay::launch(),
            Some(InstanceStatus::Launching) => t::quickplay::launching(),
            Some(InstanceStatus::Stopping) => t::quickplay::stopping(),
            Some(InstanceStatus::Running) => t::instance::kill(),
        };
        let subtitle: SharedString = match &instance {
            Some(instance) => instance.title(),
            None => t::quickplay::no_instances().into(),
        };

        let launch = v_flex()
            .id("quickplay-launch")
            .flex_1()
            .h_full()
            .items_center()
            .justify_center()
            .gap_0p5()
            .when(!busy, |this| this.hover(move |style| style.bg(hover)).cursor_pointer())
            .child(div().text_2xl().font_bold().child(label.to_uppercase()))
            .child(
                div()
                    .text_sm()
                    .text_color(gpui::white().opacity(0.75))
                    .max_w(px(300.0))
                    .truncate()
                    .child(subtitle),
            )
            .on_click({
                let data = self.data.clone();
                move |_, window, cx| {
                    let Some(instance) = &instance else {
                        root::switch_page(PageType::Instances, &[], window, cx);
                        return;
                    };
                    match instance.status {
                        InstanceStatus::NotRunning => {
                            root::start_instance(instance.id, instance.name.clone(), None, &data, window, cx);
                        },
                        InstanceStatus::Running => {
                            data.backend_handle.send(MessageToBackend::KillInstance { id: instance.id });
                        },
                        InstanceStatus::Launching | InstanceStatus::Stopping => {},
                    }
                }
            });

        let instances = div()
            .id("quickplay-instances")
            .w(px(64.0))
            .h_full()
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .hover(move |style| style.bg(hover))
            .child(Icon::new(PandoraIcon::ChevronDown).size_6())
            .tooltip(|window, cx| Tooltip::new(t::quickplay::all_instances()).build(window, cx))
            .on_click(|_, window, cx| {
                root::switch_page(PageType::Instances, &[], window, cx);
            });

        h_flex()
            .w(px(400.0))
            .h(px(78.0))
            .overflow_hidden()
            .rounded(theme.radius_lg)
            .border_2()
            .border_color(border)
            .bg(background)
            .text_color(gpui::white())
            .shadow_lg()
            .child(launch)
            .child(div().w(px(2.0)).h_full().bg(border))
            .child(instances)
            .into_any_element()
    }
}

impl Page for QuickplayPage {
    fn controls(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        gpui::Empty
    }

    fn scrollable(&self, _cx: &App) -> bool {
        false
    }
}

impl Render for QuickplayPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_skin(window, cx);

        let config = InterfaceConfig::get(cx);
        let effect = config.background_effect;
        let quality = config.background_quality;
        let skin_animation = config.skin_animation;
        let custom_background = config.custom_background.clone().filter(|path| path.exists());
        let hide_usernames = config.hide_usernames;

        // Everything but the snow holds still while the launcher isn't the focused window
        let active = window.is_window_active();
        let animate_background = config.background_animated && effect.moves() && active;
        let animate_skin = skin_animation && active;

        let background_time = self.background_clock.tick(animate_background);
        let skin_time = self.skin_clock.tick(animate_skin);
        let pose = if skin_animation {
            idle_pose(skin_time)
        } else {
            IdlePose::default()
        };
        self.model.update(cx, |model, _| model.pose = pose);

        if animate_background || animate_skin {
            self.ticker.request(quality.fps(), cx);
        }

        let palette = EffectPalette::from_theme(cx.theme());
        let background = div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .when_some(custom_background.clone(), |this, path| {
                this.child(img(path).absolute().size_full().object_fit(ObjectFit::Cover))
                    // Darkened so the nameplate and button stay readable on any picture
                    .child(div().absolute().size_full().bg(gpui::black().opacity(0.3)))
            })
            .child(
                background_effect::effect_canvas(
                    effect,
                    background_time,
                    quality,
                    palette,
                    custom_background.is_none(),
                )
                .absolute()
                .size_full(),
            );

        // The name floats over the head as part of the model, so it turns as the skin is dragged
        let nameplate = self
            .data
            .accounts
            .read(cx)
            .selected_account
            .as_ref()
            .map(|account| account.username(hide_usernames));
        self.model.update(cx, |model, _| {
            if model.nameplate != nameplate {
                model.nameplate = nameplate;
            }
        });

        let skin_visible = self.skin_visible(cx);
        let model_height = (window.viewport_size().height.as_f32() * 0.58).clamp(240.0, 640.0);
        // Where the feet end up, given the model is lowered to make room for the nameplate
        let launch_overlap = model_height * 0.04 + 105.0;
        let model = v_flex()
            .id("quickplay-skin")
            .h(px(model_height))
            .items_center()
            .cursor_grab()
            .child(PlayerModel::new(&self.model).zoom(0.88).hidden(!skin_visible))
            .on_drag(SkinDrag, |_, _, _, cx| cx.new(|_| SkinDrag))
            .on_drag_move(cx.listener(|page, event: &DragMoveEvent<SkinDrag>, window, cx| {
                if cx.active_drag_cursor_style() != Some(CursorStyle::ClosedHand) {
                    cx.set_active_drag_cursor_style(CursorStyle::ClosedHand, window);
                }
                let x = event.event.position.x;
                if let Some(last_x) = page.last_drag_x {
                    let delta = (x - last_x).as_f32() as f64 * 0.5;
                    page.model.update(cx, |model, cx| {
                        model.yaw = wrap_degrees(model.yaw + delta);
                        cx.notify();
                    });
                }
                page.last_drag_x = Some(x);
            }))
            .on_mouse_up(MouseButton::Left, cx.listener(|page, _: &MouseUpEvent, _, _| page.last_drag_x = None))
            .on_mouse_up_out(MouseButton::Left, cx.listener(|page, _: &MouseUpEvent, _, _| page.last_drag_x = None))
            .on_drop(cx.listener(|page, _: &SkinDrag, _, _| page.last_drag_x = None));

        let instance = self.latest_instance(cx);
        let launch_button = self.render_launch_button(instance, cx);

        let skin_toggle = h_flex()
            .absolute()
            .top_3()
            .right_3()
            .gap_2()
            .px_2()
            .py_1()
            .rounded(cx.theme().radius)
            .bg(gpui::black().opacity(0.3))
            .text_sm()
            .text_color(gpui::white().opacity(0.9))
            .child(t::quickplay::skin_animation())
            .child(Switch::new("quickplay-skin-animation").checked(skin_animation).on_click(|value, _, cx| {
                InterfaceConfig::get_mut(cx).skin_animation = *value;
                cx.refresh_windows();
            }));

        div()
            .id("quickplay")
            .relative()
            .size_full()
            .overflow_hidden()
            .child(background)
            .child(
                v_flex()
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .items_center()
                    .justify_center()
                    .child(model)
                    // The button stands in front of the skin's legs, leaving the feet showing
                    .child(div().mt(px(-launch_overlap)).child(launch_button)),
            )
            .child(skin_toggle)
    }
}

/// The most recently played of `entries`, which come most recently launched or added first.
///
/// One that's running or starting wins, having just been launched. Otherwise it's the one played
/// last; entry order alone would favour instances that were only just created, such as the
/// presets set up on first start. Nothing played at all falls back to the first entry.
fn latest_of<'a>(entries: impl Iterator<Item = &'a InstanceEntry>) -> Option<&'a InstanceEntry> {
    most_recent(entries, |entry| {
        (entry.status != InstanceStatus::NotRunning, entry.playtime.last_played_unix_ms)
    })
}

/// The item with the greatest `(active, last played)`, ties going to the earliest.
fn most_recent<T>(items: impl Iterator<Item = T>, recency: impl Fn(&T) -> (bool, Option<i64>)) -> Option<T> {
    items
        .enumerate()
        .max_by_key(|(position, item)| (recency(item), std::cmp::Reverse(*position)))
        .map(|(_, item)| item)
}

/// What dragging the skin around carries; there is nothing to drop.
#[derive(Clone, Copy)]
struct SkinDrag;

impl Render for SkinDrag {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        gpui::Empty
    }
}

fn wrap_degrees(degrees: f64) -> f64 {
    (degrees + 180.0).rem_euclid(360.0) - 180.0
}

/// How long the head takes to settle on where it's looking next, out of each glance.
const GLANCE_SECONDS: f64 = 4.0;
const GLANCE_TURN_SECONDS: f64 = 0.9;

/// The skin's idle animation at `time` seconds: breathing, arms swaying the way they do in game,
/// and every few seconds a glance somewhere else, often up.
pub fn idle_pose(time: f64) -> IdlePose {
    let glance = (time / GLANCE_SECONDS).floor();
    let into_glance = time - glance * GLANCE_SECONDS;
    let turn = smoothstep((into_glance / GLANCE_TURN_SECONDS).min(1.0));
    let (from_pitch, from_yaw) = glance_target(glance as i64 - 1);
    let (to_pitch, to_yaw) = glance_target(glance as i64);

    IdlePose {
        head_pitch: from_pitch + (to_pitch - from_pitch) * turn,
        head_yaw: from_yaw + (to_yaw - from_yaw) * turn,
        arm_roll: 3.5 + 3.0 * (time * 1.8).cos(),
        arm_swing: 4.0 * (time * 1.34).sin(),
        breath: 0.35 * (0.5 - 0.5 * (time * TAU / 3.6).cos()),
    }
}

/// Where the head looks during a glance, as pitch and yaw in degrees.
fn glance_target(glance: i64) -> (f64, f64) {
    if glance < 0 {
        return (0.0, 0.0);
    }
    let seed = glance as u32;
    let kind = background_effect::hash(seed, 1) as f64;
    let a = background_effect::hash(seed, 2) as f64;
    let b = background_effect::hash(seed, 3) as f64;
    if kind < 0.3 {
        (0.0, 0.0)
    } else if kind < 0.65 {
        // Looking up at the sky
        (14.0 + a * 14.0, (b - 0.5) * 16.0)
    } else {
        // Looking around
        let side = if b < 0.5 { -1.0 } else { 1.0 };
        (-3.0 + a * 10.0, side * (14.0 + b * 18.0))
    }
}

fn smoothstep(x: f64) -> f64 {
    x * x * (3.0 - 2.0 * x)
}

#[cfg(test)]
mod tests {
    // Not a glob: gpui exports its own `test` macro, which would shadow the standard one
    use super::{glance_target, idle_pose, most_recent, wrap_degrees};

    /// `(name, running, last played)`
    fn latest(items: &[(&'static str, bool, Option<i64>)]) -> Option<&'static str> {
        most_recent(items.iter(), |(_, running, played)| (*running, *played)).map(|(name, ..)| *name)
    }

    #[test]
    fn freshly_made_presets_dont_beat_what_was_played() {
        // The presets are created on first start, so they come first, but have never been played
        let items = [
            ("Vanilla", false, None),
            ("Performance", false, None),
            ("MyPack", false, Some(1_000)),
        ];
        assert_eq!(latest(&items), Some("MyPack"));
    }

    #[test]
    fn the_last_played_wins() {
        let items = [("Old", false, Some(1_000)), ("New", false, Some(5_000))];
        assert_eq!(latest(&items), Some("New"));
    }

    #[test]
    fn an_instance_being_launched_wins_before_its_play_time_is_recorded() {
        let items = [("Starting", true, Some(1_000)), ("Played", false, Some(5_000))];
        assert_eq!(latest(&items), Some("Starting"));
    }

    #[test]
    fn with_nothing_played_the_first_entry_wins() {
        let items = [("Vanilla", false, None), ("Performance", false, None)];
        assert_eq!(latest(&items), Some("Vanilla"));
        assert_eq!(latest(&[]), None);
    }

    #[test]
    fn the_pose_moves_smoothly() {
        let mut previous = idle_pose(0.0);
        let mut time = 0.0;
        while time < 60.0 {
            time += 1.0 / 60.0;
            let pose = idle_pose(time);
            assert!((pose.head_pitch - previous.head_pitch).abs() < 1.5, "head pitch jumped at {time}");
            assert!((pose.head_yaw - previous.head_yaw).abs() < 2.0, "head yaw jumped at {time}");
            assert!((pose.breath - previous.breath).abs() < 0.05, "breath jumped at {time}");
            previous = pose;
        }
    }

    #[test]
    fn the_head_looks_up_now_and_then() {
        let looks_up = (0..30).any(|glance| glance_target(glance).0 > 10.0);
        assert!(looks_up);
    }

    #[test]
    fn the_pose_stays_natural() {
        let mut time = 0.0;
        while time < 120.0 {
            let pose = idle_pose(time);
            assert!((-5.0..=30.0).contains(&pose.head_pitch), "{pose:?}");
            assert!(pose.head_yaw.abs() <= 32.0, "{pose:?}");
            assert!((0.0..=7.0).contains(&pose.arm_roll), "{pose:?}");
            assert!((0.0..=0.35).contains(&pose.breath), "{pose:?}");
            time += 0.1;
        }
    }

    #[test]
    fn yaw_wraps_into_a_half_turn_either_way() {
        assert_eq!(wrap_degrees(190.0), -170.0);
        assert_eq!(wrap_degrees(-190.0), 170.0);
        assert_eq!(wrap_degrees(45.0), 45.0);
    }
}
