use bridge::{handle::BackendHandle, java_runtime::JavaRuntimeEntry, message::MessageToBackend};
use gpui::prelude::*;
use gpui_component::select::SelectState;
use ustr::Ustr;

use super::named_dropdown::{DropdownName, NamedDropdown, NamedDropdownItem};

/// The runtime to run on, where `None` means "whatever the Minecraft version asks for".
/// This is deliberately the same shape as the field it fills in, so no conversion is
/// needed at the three places offering the override.
pub type JavaRuntimeChoice = Option<Ustr>;

pub type JavaRuntimeSelect = SelectState<NamedDropdown<JavaRuntimeChoice>>;

pub fn default_choice_item() -> NamedDropdownItem<JavaRuntimeChoice> {
    NamedDropdownItem {
        name: DropdownName::translated(t::settings::java::runtime::use_version_default),
        item: None,
    }
}

/// Turns the backend's list of runtimes into dropdown entries, marking the ones already on
/// disk so it's clear which choice would need a download first.
pub fn runtime_items(runtimes: &[JavaRuntimeEntry]) -> Vec<NamedDropdownItem<JavaRuntimeChoice>> {
    runtimes
        .iter()
        .map(|runtime| {
            let mut name = format!("{} (Java {})", runtime.component, runtime.version);
            if runtime.downloaded {
                name.push_str(" · ");
                name.push_str(t::settings::java::runtime::downloaded());
            }

            NamedDropdownItem {
                name: DropdownName::new(name),
                item: Some(Ustr::from(runtime.component.as_ref())),
            }
        })
        .collect()
}

/// An empty picker, to be filled once the runtimes arrive. `use_keyed_state` wants the
/// state itself rather than an entity of it, since it does the wrapping.
pub fn empty_select(
    selected: JavaRuntimeChoice,
    window: &mut gpui::Window,
    cx: &mut Context<JavaRuntimeSelect>,
) -> JavaRuntimeSelect {
    let mut state = SelectState::new(NamedDropdown::new(vec![default_choice_item()]), None, window, cx);
    state.set_selected_value(&selected, window, cx);
    state
}

/// Fills a picker, keeping whatever is selected if it's still on offer. A selection that has
/// gone leaves it on the version's default rather than on a runtime that can't be launched,
/// which is also what happens to an instance still pointing at a runtime Mojang retired.
pub fn fill_select(
    state: &gpui::Entity<JavaRuntimeSelect>,
    runtimes: &[JavaRuntimeEntry],
    window: &mut gpui::Window,
    cx: &mut gpui::App,
) {
    let mut items = vec![default_choice_item()];
    items.extend(runtime_items(runtimes));

    let selected = state.read(cx).selected_value().copied();
    let selected = match selected {
        Some(selected) if items.iter().any(|item| item.item == selected) => selected,
        _ => None,
    };

    state.update(cx, |state, cx| {
        state.set_items(NamedDropdown::new(items), window, cx);
        state.set_selected_value(&selected, window, cx);
        cx.notify();
    });
}

/// Asks the backend which runtimes it has to offer. Nothing on offer isn't worth an error,
/// so a failed request resolves to an empty list for the caller to render.
pub fn request_runtimes(backend_handle: &BackendHandle) -> tokio::sync::oneshot::Receiver<Vec<JavaRuntimeEntry>> {
    let (send, recv) = tokio::sync::oneshot::channel();
    backend_handle.send(MessageToBackend::GetJavaRuntimes { channel: send });
    recv
}
