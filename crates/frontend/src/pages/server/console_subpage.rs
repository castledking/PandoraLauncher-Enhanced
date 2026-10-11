use bridge::{
    handle::BackendHandle,
    instance::{InstanceID, InstanceStatus},
    message::{GameOutputMsg, MessageToBackend},
};
use gpui::{prelude::*, *};
use gpui_component::{
    ActiveTheme, Disableable, Icon, Sizable, StyledExt,
    button::{Button, ButtonVariants},
    h_flex,
    input::{Input, InputEvent, InputState},
    v_flex,
};

use crate::{
    entity::instance::InstanceEntry,
    game_output::{GameOutput, GameOutputRoot, GameOutputTarget},
    icon::PandoraIcon,
    pages::servers_page::start_server,
};

/// Commands offered as one-click buttons, because they're the ones people reach for constantly.
const QUICK_COMMANDS: &[(&str, fn() -> &'static str, PandoraIcon)] = &[
    ("list", t::server::console::quick_players, PandoraIcon::Users),
    ("save-all", t::server::console::quick_save, PandoraIcon::HardDrive),
    ("time set day", t::server::console::quick_day, PandoraIcon::Sun),
    ("weather clear", t::server::console::quick_weather, PandoraIcon::CloudSunRain),
];

pub struct ServerConsoleSubpage {
    instance: Entity<InstanceEntry>,
    instance_id: InstanceID,
    backend_handle: BackendHandle,
    output: Option<Entity<GameOutputRoot>>,
    /// Whether `output` is attached to the session that is running right now, as opposed to
    /// showing what was printed by a session that has since ended.
    attached: bool,
    command_input: Entity<InputState>,
    history: Vec<SharedString>,
    history_cursor: Option<usize>,
    _attach_task: Task<()>,
    _subscriptions: Vec<Subscription>,
}

impl ServerConsoleSubpage {
    pub fn new(
        instance: &Entity<InstanceEntry>,
        backend_handle: BackendHandle,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let instance_id = instance.read(cx).id;
        let command_input = cx.new(|cx| InputState::new(window, cx).placeholder(t::server::console::placeholder()));

        let subscriptions = vec![
            cx.subscribe_in(&command_input, window, |this, _, event: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { .. } = event {
                    this.submit(window, cx);
                }
            }),
            cx.observe_in(instance, window, |this, _, window, cx| {
                this.sync_attachment(window, cx);
                cx.notify();
            }),
        ];

        let mut this = Self {
            instance: instance.clone(),
            instance_id,
            backend_handle,
            output: None,
            attached: false,
            command_input,
            history: Vec::new(),
            history_cursor: None,
            _attach_task: Task::ready(()),
            _subscriptions: subscriptions,
        };
        this.sync_attachment(window, cx);
        this
    }

    fn is_running(&self, cx: &App) -> bool {
        matches!(self.instance.read(cx).status, InstanceStatus::Running | InstanceStatus::Stopping)
    }

    /// Attaches to the server's output whenever it is running and we aren't already. When it
    /// stops, the last session's output is left on screen until the next start.
    fn sync_attachment(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let running = self.is_running(cx);

        if !running {
            self.attached = false;
            return;
        }
        if self.attached {
            return;
        }
        self.attached = true;

        let (send, recv) = tokio::sync::oneshot::channel();
        self.backend_handle.send(MessageToBackend::SubscribeServerConsole {
            id: self.instance_id,
            channel: send,
        });

        let target = GameOutputTarget {
            instance: Some(self.instance.clone()),
            backend_handle: self.backend_handle.clone(),
        };

        self._attach_task = cx.spawn_in(window, async move |this, cx| {
            let Ok((history, mut live)) = recv.await else {
                return;
            };

            // GameOutput takes a single receiver, so the scrollback is replayed into a fresh
            // channel ahead of the live lines
            let (forward, receiver) = tokio::sync::mpsc::unbounded_channel::<GameOutputMsg>();
            for line in history {
                _ = forward.send(line);
            }

            _ = this.update_in(cx, |this, window, cx| {
                let game_output = cx.new(|cx| GameOutput::new(receiver, cx));
                this.output = Some(cx.new(|cx| GameOutputRoot::new(game_output, Some(target.clone()), window, cx)));
                cx.notify();
            });

            while let Some(line) = live.recv().await {
                if forward.send(line).is_err() {
                    break;
                }
            }
        });
    }

    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let command = self.command_input.read(cx).value();
        let command = command.trim();
        if command.is_empty() || !self.is_running(cx) {
            return;
        }
        let command: SharedString = command.trim_start_matches('/').to_string().into();
        self.send_command(command.clone());

        if self.history.last() != Some(&command) {
            self.history.push(command);
        }
        self.history_cursor = None;
        self.command_input.update(cx, |input, cx| input.set_value("", window, cx));
    }

    fn send_command(&self, command: SharedString) {
        self.backend_handle.send(MessageToBackend::SendServerCommand {
            id: self.instance_id,
            command: command.as_str().into(),
        });
    }

    /// Up and down walk back through previously sent commands, like a terminal.
    fn step_history(&mut self, older: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.history.is_empty() {
            return;
        }
        let next = match (self.history_cursor, older) {
            (None, true) => Some(self.history.len() - 1),
            (None, false) => None,
            (Some(0), true) => Some(0),
            (Some(index), true) => Some(index - 1),
            (Some(index), false) if index + 1 < self.history.len() => Some(index + 1),
            (Some(_), false) => None,
        };
        self.history_cursor = next;
        let value = next.map(|index| self.history[index].clone()).unwrap_or_default();
        self.command_input.update(cx, |input, cx| input.set_value(value, window, cx));
    }

    fn render_offline(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let id = self.instance_id;
        let backend_handle = self.backend_handle.clone();
        let eula_accepted = self
            .instance
            .read(cx)
            .configuration
            .server
            .as_ref()
            .map(|server| server.eula_accepted)
            .unwrap_or(false);

        div().size_full().flex().items_center().justify_center().child(
            v_flex()
                .items_center()
                .gap_3()
                .max_w(px(360.0))
                .child(
                    div()
                        .size_12()
                        .rounded_full()
                        .bg(theme.secondary)
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(Icon::new(PandoraIcon::SquareTerminal).size_6().text_color(theme.muted_foreground)),
                )
                .child(div().font_semibold().child(t::server::console::offline_title()))
                .child(
                    div()
                        .text_sm()
                        .text_center()
                        .text_color(theme.muted_foreground)
                        .child(t::server::console::offline_body()),
                )
                .child(
                    Button::new("console-start")
                        .success()
                        .icon(PandoraIcon::Play)
                        .label(t::server::action::start())
                        .disabled(!eula_accepted)
                        .on_click(move |_, window, cx| start_server(id, &backend_handle, window, cx)),
                ),
        )
    }
}

impl Render for ServerConsoleSubpage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let running = self.is_running(cx);
        let theme = cx.theme().clone();

        let output = match &self.output {
            Some(output) => div().size_full().child(output.clone()).into_any_element(),
            None => self.render_offline(cx).into_any_element(),
        };

        let quick_commands = h_flex().gap_1p5().flex_wrap().children(QUICK_COMMANDS.iter().enumerate().map(
            |(index, (command, label, icon))| {
                let command = *command;
                Button::new(("quick-command", index))
                    .xsmall()
                    .outline()
                    .icon(Icon::new(icon.clone()).size_3p5())
                    .label(label())
                    .disabled(!running)
                    .on_click(cx.listener(move |this, _, _, _| this.send_command(command.into())))
            },
        ));

        let input_row = h_flex()
            .gap_2()
            .child(
                div()
                    .flex_shrink_0()
                    .font_family("Roboto Mono")
                    .text_color(if running { theme.success } else { theme.muted_foreground })
                    .child(">"),
            )
            .child(
                div()
                    .flex_1()
                    .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                        match event.keystroke.key.as_str() {
                            "up" => this.step_history(true, window, cx),
                            "down" => this.step_history(false, window, cx),
                            _ => return,
                        }
                        cx.stop_propagation();
                    }))
                    .child(Input::new(&self.command_input).disabled(!running)),
            )
            .child(
                Button::new("send-command")
                    .primary()
                    .icon(PandoraIcon::ArrowRight)
                    .label(t::server::console::send())
                    .disabled(!running)
                    .on_click(cx.listener(|this, _, window, cx| this.submit(window, cx))),
            );

        v_flex().size_full().child(div().flex_1().min_h_0().child(output)).child(
            v_flex()
                .gap_2()
                .p_3()
                .border_t_1()
                .border_color(theme.border)
                .child(quick_commands)
                .child(input_row),
        )
    }
}
