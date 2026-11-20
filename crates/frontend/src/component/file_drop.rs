//! The hint shown while files from outside the launcher are dragged over it, and revealing
//! dropped content once it lands in its folder.

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use bridge::instance::{ContentFolder, InstanceID};
use gpui::{prelude::*, *};
use gpui_component::{ActiveTheme, Icon, v_flex};

use crate::icon::PandoraIcon;

/// A pending request to highlight and scroll to newly dropped content once its tab loads it.
#[derive(Clone, Debug)]
pub struct PendingContentReveal {
    pub instance: InstanceID,
    pub folder: ContentFolder,
    pub filenames: Arc<[Arc<str>]>,
    pub created_at: Instant,
}

#[derive(Default)]
struct PendingContentRevealSlot(Option<PendingContentReveal>);

impl Global for PendingContentRevealSlot {}

const REVEAL_TIMEOUT: Duration = Duration::from_secs(10);

pub fn set_pending_reveal(instance: InstanceID, folder: ContentFolder, filenames: Arc<[Arc<str>]>, cx: &mut App) {
    cx.set_global(PendingContentRevealSlot(Some(PendingContentReveal {
        instance,
        folder,
        filenames,
        created_at: Instant::now(),
    })));
}

pub fn pending_reveal_for_instance(instance: InstanceID, cx: &App) -> Option<PendingContentReveal> {
    let reveal = cx.try_global::<PendingContentRevealSlot>()?.0.as_ref()?;
    if reveal.instance == instance && reveal.created_at.elapsed() < REVEAL_TIMEOUT {
        Some(reveal.clone())
    } else {
        None
    }
}

pub fn clear_pending_reveal(cx: &mut App) {
    if cx.try_global::<PendingContentRevealSlot>().is_some_and(|slot| slot.0.is_some()) {
        cx.set_global(PendingContentRevealSlot(None));
    }
}

/// Whether the files being dragged are over an instance's or server's content, which shows its own
/// hint, so the main window keeps its own out of the way. Set as the drag moves.
#[derive(Default)]
pub struct FileDragOverContent(pub bool);

impl Global for FileDragOverContent {}

pub fn is_over_content(cx: &App) -> bool {
    cx.try_global::<FileDragOverContent>().is_some_and(|over| over.0)
}

pub fn set_over_content(over: bool, window: &mut Window, cx: &mut App) {
    if is_over_content(cx) != over {
        cx.set_global(FileDragOverContent(over));
        window.refresh();
    }
}

/// Laid over a drop target: an upload icon and what dropping there will do.
pub fn drop_overlay(id: &'static str, text: SharedString, cx: &App) -> impl IntoElement {
    let theme = cx.theme();
    div()
        .id(id)
        .absolute()
        .top_0()
        .left_0()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .rounded(theme.radius)
        .border_2()
        .border_color(theme.primary)
        .bg(theme.background.opacity(0.88))
        .child(
            v_flex()
                .items_center()
                .gap_3()
                .px_6()
                .child(Icon::new(PandoraIcon::Upload).size_12().text_color(theme.primary))
                .child(div().text_xl().text_center().child(text)),
        )
}
