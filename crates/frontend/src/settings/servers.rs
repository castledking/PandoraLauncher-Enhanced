use std::{rc::Rc, sync::Arc};

use bridge::account::Account;
use gpui::{prelude::*, *};
use gpui_component::{
    ActiveTheme, Disableable, Sizable,
    button::{Button, ButtonVariants},
    checkbox::Checkbox,
    h_flex,
    input::{Input, InputEvent, InputState},
    menu::{DropdownMenu, PopupMenuItem},
    switch::Switch,
    v_flex,
};
use schema::backend_config::{ServerDefaultPlayer, ServerDefaults};

use crate::{
    entity::{
        DataEntities,
        account::{AccountEntries, AccountExt},
    },
    icon::PandoraIcon,
    interface_config::InterfaceConfig,
    settings::{SettingGroup, SettingItem, SettingItemWidget, SettingPage, SettingsRoot},
};

pub(super) fn create_page(data: &DataEntities) -> SettingPage {
    let accounts = data.accounts.clone();
    SettingPage {
        title: t::settings::servers,
        groups: vec![SettingGroup {
            title: Some(t::settings::servers::new_servers),
            items: vec![
                SettingItem {
                    title: t::settings::servers::new_servers::players,
                    description: t::settings::servers::new_servers::players_desc,
                    widget: SettingItemWidget::BackendFullWidth(Rc::new(move |backend, window, cx| {
                        render_default_players(&backend.server_defaults, &accounts, window, cx)
                    })),
                    ..Default::default()
                },
                SettingItem {
                    title: t::settings::servers::new_servers::enable_whitelist,
                    description: t::settings::servers::new_servers::enable_whitelist_desc,
                    widget: SettingItemWidget::Backend(Rc::new(|backend, _, cx| {
                        Switch::new("server-defaults-whitelist")
                            .checked(backend.server_defaults.enable_whitelist)
                            .on_click(cx.listener(|root, value: &bool, _, cx| {
                                let value = *value;
                                root.update_server_defaults(cx, |defaults| defaults.enable_whitelist = value);
                            }))
                            .into_any_element()
                    })),
                    ..Default::default()
                },
            ]
            .into(),
            searched_items: None,
        }]
        .into(),
        searched_groups: None,
    }
}

/// Adds a player, or fills in the UUID of one already there under the same name, since a server
/// treats names case-insensitively and two entries for one player would only confuse it.
fn add_player(defaults: &mut ServerDefaults, player: ServerDefaultPlayer) {
    match defaults
        .players
        .iter_mut()
        .find(|existing| existing.name.eq_ignore_ascii_case(&player.name))
    {
        Some(existing) => {
            if player.uuid.is_some() {
                existing.uuid = player.uuid;
            }
        },
        None => defaults.players.push(player),
    }
}

fn account_player(account: &Account) -> ServerDefaultPlayer {
    ServerDefaultPlayer::new(account.username.clone(), Some(account.uuid))
}

/// Several names can be typed at once, separated by spaces or commas.
fn typed_names(value: &str) -> Vec<Arc<str>> {
    value
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|name| !name.is_empty())
        .map(Arc::from)
        .collect()
}

fn add_typed_players(
    root: &mut SettingsRoot,
    input: &Entity<InputState>,
    window: &mut Window,
    cx: &mut Context<SettingsRoot>,
) {
    let names = typed_names(&input.read(cx).value());
    if names.is_empty() {
        return;
    }
    root.update_server_defaults(cx, |defaults| {
        for name in names {
            add_player(defaults, ServerDefaultPlayer::new(name, None));
        }
    });
    input.update(cx, |input, cx| input.set_value("", window, cx));
}

fn render_default_players(
    defaults: &ServerDefaults,
    accounts: &Entity<AccountEntries>,
    window: &mut Window,
    cx: &mut Context<SettingsRoot>,
) -> AnyElement {
    let mut created = false;
    let input = window.use_keyed_state("server-default-player-name", cx, |window, cx| {
        created = true;
        InputState::new(window, cx).placeholder(t::settings::servers::new_servers::name_placeholder())
    });
    if created {
        cx.subscribe_in(&input, window, |root, input, event: &InputEvent, window, cx| match event {
            InputEvent::PressEnter { .. } => add_typed_players(root, input, window, cx),
            // Keeps the Add button's disabled state in step with what's typed
            InputEvent::Change => cx.notify(),
            _ => {},
        })
        .detach();
    }

    let hide_usernames = InterfaceConfig::get(cx).hide_usernames;
    let account_list: Arc<[Account]> = accounts.read(cx).accounts.clone();
    let has_typed = !typed_names(&input.read(cx).value()).is_empty();
    let root = cx.entity().downgrade();

    let add_account = Button::new("server-default-add-account")
        .outline()
        .icon(PandoraIcon::Users)
        .label(t::settings::servers::new_servers::add_account())
        .dropdown_menu({
            let account_list = account_list.clone();
            let root = root.clone();
            move |mut menu, _, _| {
                if account_list.is_empty() {
                    return menu
                        .item(PopupMenuItem::new(t::settings::servers::new_servers::no_accounts()).disabled(true));
                }

                if account_list.len() > 1 {
                    menu = menu
                        .item(PopupMenuItem::new(t::settings::servers::new_servers::all_accounts()).on_click({
                            let account_list = account_list.clone();
                            let root = root.clone();
                            move |_, _, cx| {
                                _ = root.update(cx, |root, cx| {
                                    root.update_server_defaults(cx, |defaults| {
                                        for account in account_list.iter() {
                                            add_player(defaults, account_player(account));
                                        }
                                    });
                                });
                            }
                        }))
                        .separator();
                }

                for account in account_list.iter() {
                    let player = account_player(account);
                    let root = root.clone();
                    menu = menu.item(PopupMenuItem::new(account.username(hide_usernames)).on_click(move |_, _, cx| {
                        let player = player.clone();
                        _ = root.update(cx, |root, cx| {
                            root.update_server_defaults(cx, |defaults| add_player(defaults, player));
                        });
                    }));
                }
                menu
            }
        });

    let add_row = h_flex()
        .w_full()
        .gap_2()
        .child(div().flex_1().min_w_0().child(Input::new(&input).prefix(PandoraIcon::User)))
        .child(
            Button::new("server-default-add-player")
                .primary()
                .icon(PandoraIcon::Plus)
                .label(t::settings::servers::new_servers::add())
                .disabled(!has_typed)
                .on_click(cx.listener({
                    let input = input.clone();
                    move |root, _, window, cx| add_typed_players(root, &input, window, cx)
                })),
        )
        .child(add_account);

    let theme = cx.theme();
    let rows = defaults.players.iter().enumerate().map(|(index, player)| {
        let is_account = player.uuid.is_some_and(|uuid| account_list.iter().any(|account| account.uuid == uuid));
        let name: SharedString = match account_list.iter().find(|account| Some(account.uuid) == player.uuid) {
            Some(account) => account.username(hide_usernames),
            None => player.name.clone().into(),
        };
        let key = player.name.clone();

        h_flex()
            .id(("server-default-player", index))
            .w_full()
            .gap_3()
            .px_2()
            .py_1p5()
            .rounded(theme.radius)
            .border_1()
            .border_color(theme.border)
            .child(
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_2()
                    .child(gpui_component::Icon::new(if is_account {
                        PandoraIcon::CircleUser
                    } else {
                        PandoraIcon::User
                    }))
                    .child(div().truncate().child(name))
                    .when(is_account, |this| {
                        this.child(
                            div()
                                .flex_shrink_0()
                                .text_xs()
                                .px_1p5()
                                .rounded(theme.radius)
                                .bg(theme.secondary)
                                .text_color(theme.muted_foreground)
                                .child(t::settings::servers::new_servers::account()),
                        )
                    }),
            )
            .child(
                Checkbox::new(("server-default-op", index))
                    .label(t::settings::servers::new_servers::operator())
                    .checked(player.operator)
                    .on_click(cx.listener({
                        let key = key.clone();
                        move |root, value: &bool, _, cx| {
                            let value = *value;
                            root.update_server_defaults(cx, |defaults| {
                                if let Some(player) = defaults.players.iter_mut().find(|player| player.name == key) {
                                    player.operator = value;
                                }
                            });
                        }
                    })),
            )
            .child(
                Checkbox::new(("server-default-whitelist", index))
                    .label(t::settings::servers::new_servers::whitelisted())
                    .checked(player.whitelisted)
                    .on_click(cx.listener({
                        let key = key.clone();
                        move |root, value: &bool, _, cx| {
                            let value = *value;
                            root.update_server_defaults(cx, |defaults| {
                                if let Some(player) = defaults.players.iter_mut().find(|player| player.name == key) {
                                    player.whitelisted = value;
                                }
                            });
                        }
                    })),
            )
            .child(
                Button::new(("server-default-remove", index))
                    .small()
                    .ghost()
                    .icon(PandoraIcon::Trash2)
                    .tooltip(t::settings::servers::new_servers::remove())
                    .on_click(cx.listener(move |root, _, _, cx| {
                        root.update_server_defaults(cx, |defaults| {
                            defaults.players.retain(|player| player.name != key)
                        });
                    })),
            )
    });

    let list = if defaults.players.is_empty() {
        div()
            .py_2()
            .text_sm()
            .text_color(theme.muted_foreground)
            .child(t::settings::servers::new_servers::empty())
            .into_any_element()
    } else {
        v_flex().w_full().gap_1p5().children(rows).into_any_element()
    };

    v_flex().w_full().gap_3().child(add_row).child(list).into_any_element()
}

#[cfg(test)]
mod tests {
    use schema::backend_config::{ServerDefaultPlayer, ServerDefaults};
    use uuid::Uuid;

    use super::{add_player, typed_names};

    #[test]
    fn several_names_can_be_typed_at_once() {
        let names = typed_names(" Notch, jeb_  Dinnerbone,,");
        assert_eq!(names.iter().map(|name| &**name).collect::<Vec<_>>(), ["Notch", "jeb_", "Dinnerbone"]);
        assert!(typed_names("  , ").is_empty());
    }

    #[test]
    fn a_name_is_only_added_once() {
        let mut defaults = ServerDefaults::default();
        add_player(&mut defaults, ServerDefaultPlayer::new("Notch".into(), None));
        add_player(&mut defaults, ServerDefaultPlayer::new("notch".into(), None));
        assert_eq!(defaults.players.len(), 1);

        // An account picked for a name that was typed in gives it the account's UUID
        let uuid = Uuid::from_u128(7);
        add_player(&mut defaults, ServerDefaultPlayer::new("NOTCH".into(), Some(uuid)));
        assert_eq!(defaults.players.len(), 1);
        assert_eq!(defaults.players[0].uuid, Some(uuid));
    }
}
