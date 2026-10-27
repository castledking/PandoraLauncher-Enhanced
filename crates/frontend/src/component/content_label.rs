//! Showing and editing the labels the user puts on content in a content list: a category, and a
//! highlight colour the item's row is tinted with.

use std::{rc::Rc, sync::Arc};

use bridge::{
    handle::BackendHandle,
    instance::{InstanceContentID, InstanceID},
    message::MessageToBackend,
};
use gpui::{prelude::*, *};
use gpui_component::{
    ActiveTheme, Icon, Side, h_flex,
    menu::{PopupMenu, PopupMenuItem},
};
use schema::content_label::{ContentLabel, ContentLabelEdit, HexColor, edit_global_label};

use crate::{
    icon::PandoraIcon,
    interface_config::{ContentCategory, InterfaceConfig, OTHER_CONTENT_CATEGORY},
    pages::modrinth_page::icon_for,
};

/// The colours offered for highlighting a row or a category. Mid-tone, so a light tint of any of
/// them reads on both light and dark themes.
const PRESET_COLORS: &[(u32, fn() -> &'static str)] = &[
    (0xef4444, t::instance::content::label::color::red),
    (0xf97316, t::instance::content::label::color::orange),
    (0xeab308, t::instance::content::label::color::yellow),
    (0x22c55e, t::instance::content::label::color::green),
    (0x14b8a6, t::instance::content::label::color::teal),
    (0x3b82f6, t::instance::content::label::color::blue),
    (0xa855f7, t::instance::content::label::color::purple),
    (0xec4899, t::instance::content::label::color::pink),
    (0x64748b, t::instance::content::label::color::gray),
];

pub fn preset_colors() -> impl Iterator<Item = (HexColor, &'static str)> {
    PRESET_COLORS.iter().map(|(rgb, name)| (HexColor::new(*rgb), name()))
}

pub fn is_preset_color(color: HexColor) -> bool {
    PRESET_COLORS.iter().any(|(rgb, _)| *rgb == color.rgb())
}

pub fn to_hsla(color: HexColor) -> Hsla {
    gpui::rgb(color.rgb()).into()
}

/// The category's name: translated for the built-in ones, as typed for the user's own.
pub fn category_name(id: &str) -> SharedString {
    category_name_inner(id, false)
}

/// As [`category_name`], but preferring the shorter form some Modrinth categories have.
pub fn category_short_name(id: &str) -> SharedString {
    category_name_inner(id, true)
}

fn category_name_inner(id: &str, short: bool) -> SharedString {
    if let Some(name) = t::modrinth::category::get(id, short) {
        SharedString::new_static(name)
    } else if id == OTHER_CONTENT_CATEGORY {
        SharedString::new_static(t::instance::content::label::other())
    } else {
        SharedString::from(id.to_string())
    }
}

pub fn category_icon(id: &str) -> Option<&'static str> {
    icon_for(id)
}

/// Whether `name` would be taken for a new category: it matches an existing category's id or the
/// name it's shown under, ignoring case.
pub fn category_exists(name: &str, categories: &[ContentCategory]) -> bool {
    let name = name.trim().to_lowercase();
    categories
        .iter()
        .any(|category| category.id.to_lowercase() == name || category_name(&category.id).to_lowercase() == name)
}

/// The order sorting by category puts content in: categorised content first, alphabetically by the
/// name its category is shown under, then content filed under "Other", then uncategorised content.
pub fn compare_categories(a: Option<&str>, b: Option<&str>) -> std::cmp::Ordering {
    fn rank(category: Option<&str>) -> (u8, SharedString) {
        match category.map(str::trim).filter(|category| !category.is_empty()) {
            None => (2, SharedString::default()),
            Some(category) if category.eq_ignore_ascii_case(OTHER_CONTENT_CATEGORY) => (1, SharedString::default()),
            Some(category) => (0, category_name(category)),
        }
    }

    let (rank_a, name_a) = rank(a);
    let (rank_b, name_b) = rank(b);
    rank_a.cmp(&rank_b).then_with(|| lexical_sort::natural_lexical_cmp(&name_a, &name_b))
}

pub fn category_color(category: Option<&str>, categories: &[ContentCategory]) -> Option<HexColor> {
    let category = category?;
    categories.iter().find(|candidate| &*candidate.id == category)?.color
}

/// The colour an item's row is tinted with: the item's own, or else its category's.
pub fn resolve_color(label: &ContentLabel, categories: &[ContentCategory]) -> Option<HexColor> {
    label.color.or_else(|| category_color(label.category.as_deref(), categories))
}

/// Background and border for a row tinted with `color`.
///
/// The tint is kept light enough that the row's text, toggle and buttons read as well as on an
/// untinted row. A selected row is tinted a little more strongly and takes the theme's selection
/// border, so selecting it doesn't hide what colour it is.
pub fn row_tint(color: HexColor, selected: bool, cx: &App) -> (Hsla, Hsla) {
    let color = to_hsla(color);
    if selected {
        (color.opacity(0.28), cx.theme().selection)
    } else {
        (color.opacity(0.14), color.opacity(0.45))
    }
}

/// A small dot of colour, or an empty ring when there's no colour.
pub fn swatch(color: Option<HexColor>, cx: &App) -> Div {
    let dot = div().flex_none().size_3().rounded_full().border_1();
    match color {
        Some(color) => dot.bg(to_hsla(color)).border_color(cx.theme().foreground.opacity(0.25)),
        None => dot.border_color(cx.theme().muted_foreground),
    }
}

/// The category as a small tag, shown on standard rows.
pub fn category_chip(id: &str, color: Option<HexColor>, cx: &App) -> Div {
    let accent = color.map(to_hsla).unwrap_or(cx.theme().muted_foreground);
    h_flex()
        .flex_none()
        .gap_1()
        .px_1p5()
        .py_0p5()
        .rounded(cx.theme().radius)
        .border_1()
        .border_color(accent.opacity(0.6))
        .bg(accent.opacity(0.15))
        .text_xs()
        .line_height(rems(1.0))
        .when_some(category_icon(id), |this, icon| this.child(Icon::empty().path(icon).size_3()))
        .child(category_short_name(id))
}

/// The items a label menu or dialog changes.
#[derive(Clone)]
pub enum LabelTarget {
    Global {
        keys: Arc<[Arc<str>]>,
        name: Arc<str>,
    },
    Content {
        instance: InstanceID,
        content_ids: Arc<[InstanceContentID]>,
    },
    /// One of a modpack's own files, known by its [`schema::content_label::child_label_keys`].
    Child {
        instance: InstanceID,
        content_id: InstanceContentID,
        child_keys: Arc<[Arc<str>]>,
    },
}

impl LabelTarget {
    pub fn send(&self, edit: ContentLabelEdit, backend_handle: &BackendHandle, cx: &mut App) {
        match self {
            LabelTarget::Global { keys, name } => {
                edit_global_label(&mut InterfaceConfig::get_mut(cx).global_content_labels, keys, name.clone(), &edit);
                cx.refresh_windows();
            },
            LabelTarget::Content { instance, content_ids } => {
                backend_handle.send(MessageToBackend::SetContentLabel {
                    id: *instance,
                    content_ids: content_ids.to_vec(),
                    edit,
                });
            },
            LabelTarget::Child {
                instance,
                content_id,
                child_keys,
            } => {
                backend_handle.send(MessageToBackend::SetContentChildLabel {
                    id: *instance,
                    content_id: *content_id,
                    child_keys: child_keys.clone(),
                    edit,
                });
            },
        }
    }
}

/// What a label menu needs to know about the rows it was opened on.
pub struct LabelMenu {
    pub target: LabelTarget,
    /// The current label of each targeted item, used to tick what they already have.
    pub labels: Vec<ContentLabel>,
    /// A filename to show in the colour dialog's preview.
    pub preview_name: SharedString,
    pub backend_handle: BackendHandle,
}

impl LabelMenu {
    fn send(&self, edit: ContentLabelEdit, cx: &mut App) {
        self.target.send(edit, &self.backend_handle, cx);
    }

    /// `Some` when every targeted item has the same value.
    fn common<T: PartialEq>(&self, value: impl Fn(&ContentLabel) -> T) -> Option<T> {
        let mut values = self.labels.iter().map(value);
        let first = values.next()?;
        values.all(|other| other == first).then_some(first)
    }

    /// Builds the right-click menu for a row: a category submenu, a highlight colour submenu and a
    /// way to clear both.
    pub fn build(self: &Rc<Self>, mut menu: PopupMenu, window: &mut Window, cx: &mut Context<PopupMenu>) -> PopupMenu {
        if self.labels.len() > 1 {
            menu = menu.label(t::instance::content::label::selected_count(self.labels.len()));
        }
        if !matches!(self.target, LabelTarget::Global { .. }) {
            menu = menu.label(t::settings::content_labels::local_hint());
        }

        let this = self.clone();
        menu = menu.submenu_with_icon(
            Some(PandoraIcon::Tags.into()),
            t::instance::content::label::category(),
            window,
            cx,
            move |menu, _, cx| this.category_menu(menu, cx),
        );

        let this = self.clone();
        menu = menu.submenu_with_icon(
            Some(PandoraIcon::Palette.into()),
            t::instance::content::label::highlight_color(),
            window,
            cx,
            move |menu, _, cx| this.color_menu(menu, cx),
        );

        if self.labels.iter().any(|label| !label.is_empty()) {
            let this = self.clone();
            menu = menu.separator().item(
                PopupMenuItem::new(t::instance::content::label::clear())
                    .icon(PandoraIcon::Close)
                    .on_click(move |_, _, cx| this.send(ContentLabelEdit::Clear, cx)),
            );
        }

        if !matches!(self.target, LabelTarget::Global { .. })
            && self.labels.iter().any(|label| label.category_override || label.color_override)
        {
            let this = self.clone();
            menu = menu.item(
                PopupMenuItem::new(t::settings::content_labels::use_global())
                    .icon(PandoraIcon::RefreshCcw)
                    .on_click(move |_, _, cx| this.send(ContentLabelEdit::UseGlobal, cx)),
            );
        }

        menu
    }

    fn category_menu(self: &Rc<Self>, menu: PopupMenu, cx: &mut Context<PopupMenu>) -> PopupMenu {
        let current = self.common(|label| label.category.clone());
        let categories = InterfaceConfig::get(cx).content_categories.clone();

        let this = self.clone();
        let mut menu = menu.check_side(Side::Right).item(
            PopupMenuItem::new(t::instance::content::label::no_category())
                .checked(current == Some(None))
                .on_click(move |_, _, cx| this.send(ContentLabelEdit::SetCategory(None), cx)),
        );

        // A category that's no longer in the list (deleted, or the instance came from elsewhere)
        // is still listed while something is in it, so it can be seen and picked again
        let mut entries: Vec<(Arc<str>, Option<HexColor>)> =
            categories.iter().map(|category| (category.id.clone(), category.color)).collect();
        for label in &self.labels {
            if let Some(category) = &label.category
                && !entries.iter().any(|(id, _)| id == category)
            {
                entries.push((category.clone(), None));
            }
        }

        if !entries.is_empty() {
            menu = menu.separator();
        }
        for (id, color) in entries {
            let checked = current.as_ref().is_some_and(|current| current.as_deref() == Some(&*id));
            let name = category_name(&id);
            let this = self.clone();
            menu = menu.item(
                PopupMenuItem::element(move |_, cx| h_flex().gap_2().child(swatch(color, cx)).child(name.clone()))
                    .when_some(category_icon(&id), |item, icon| item.icon(Icon::empty().path(icon)))
                    .checked(checked)
                    .on_click(move |_, _, cx| this.send(ContentLabelEdit::SetCategory(Some(id.clone())), cx)),
            );
        }

        let this = self.clone();
        menu.separator()
            .item(
                PopupMenuItem::new(t::instance::content::label::new_category())
                    .icon(PandoraIcon::Plus)
                    .on_click(move |_, window, cx| {
                        let this = this.clone();
                        // Opened once the menu has closed, or closing it takes focus back from the dialog
                        window.defer(cx, move |window, cx| {
                            let on_created: Rc<dyn Fn(Arc<str>, &mut Window, &mut App)> = Rc::new(move |id, _, cx| {
                                this.send(ContentLabelEdit::SetCategory(Some(id)), cx);
                            });
                            crate::modals::content_label::open_new_category(Some(on_created), window, cx);
                        });
                    }),
            )
            .item(
                PopupMenuItem::new(t::instance::content::label::manage_categories())
                    .icon(PandoraIcon::Settings2)
                    .on_click(move |_, window, cx| {
                        window.defer(cx, |window, cx| {
                            crate::modals::content_label::open_manage_categories(window, cx);
                        });
                    }),
            )
    }

    fn color_menu(self: &Rc<Self>, menu: PopupMenu, cx: &mut Context<PopupMenu>) -> PopupMenu {
        let current = self.common(|label| label.color);
        let categories = &InterfaceConfig::get(cx).content_categories;

        // Without a colour of its own a row falls back to its category's, so say so when that's
        // what clearing the colour will actually show
        let fallback = self.common(|label| category_color(label.category.as_deref(), categories)).flatten();
        let any_fallback = self
            .labels
            .iter()
            .any(|label| category_color(label.category.as_deref(), categories).is_some());
        let none_label = if any_fallback {
            t::instance::content::label::category_color()
        } else {
            t::instance::content::label::no_color()
        };

        let this = self.clone();
        let mut menu = menu
            .check_side(Side::Right)
            .item(
                PopupMenuItem::element(move |_, cx| h_flex().gap_2().child(swatch(fallback, cx)).child(none_label))
                    .checked(current == Some(None))
                    .on_click(move |_, _, cx| this.send(ContentLabelEdit::SetColor(None), cx)),
            )
            .separator();

        for (color, name) in preset_colors() {
            let this = self.clone();
            menu = menu.item(
                PopupMenuItem::element(move |_, cx| h_flex().gap_2().child(swatch(Some(color), cx)).child(name))
                    .checked(current == Some(Some(color)))
                    .on_click(move |_, _, cx| this.send(ContentLabelEdit::SetColor(Some(color)), cx)),
            );
        }

        // A colour typed in by hand has no entry of its own, so show which one it is
        if let Some(Some(color)) = current
            && !is_preset_color(color)
        {
            menu = menu.item(
                PopupMenuItem::element(move |_, cx| {
                    h_flex()
                        .gap_2()
                        .child(swatch(Some(color), cx))
                        .child(SharedString::from(color.to_string()))
                })
                .checked(true),
            );
        }

        let this = self.clone();
        let initial = current.flatten().or(fallback);
        menu.separator().item(
            PopupMenuItem::new(t::instance::content::label::custom_color())
                .icon(PandoraIcon::Palette)
                .on_click(move |_, window, cx| {
                    let this = this.clone();
                    window.defer(cx, move |window, cx| {
                        let preview_name = this.preview_name.clone();
                        let on_apply: Rc<dyn Fn(Option<HexColor>, &mut Window, &mut App)> =
                            Rc::new(move |color, _, cx| this.send(ContentLabelEdit::SetColor(color), cx));
                        crate::modals::content_label::open_color_picker(
                            t::instance::content::label::highlight_color().into(),
                            initial,
                            preview_name,
                            on_apply,
                            window,
                            cx,
                        );
                    });
                }),
        )
    }
}

#[cfg(test)]
mod tests {
    // Not a glob import: that would bring in gpui's `test` attribute in place of the built-in one
    use std::sync::Arc;

    use schema::content_label::{ContentLabel, HexColor};

    use super::{
        category_color, category_exists, category_name, category_short_name, compare_categories, resolve_color,
    };
    use crate::interface_config::{ContentCategory, OTHER_CONTENT_CATEGORY, default_content_categories};

    fn label(category: Option<&str>, color: Option<u32>) -> ContentLabel {
        ContentLabel {
            category: category.map(Arc::from),
            color: color.map(HexColor::new),
            ..Default::default()
        }
    }

    #[test]
    fn own_colour_beats_category_colour() {
        let categories = default_content_categories();
        let combat = category_color(Some("combat"), &categories).unwrap();

        assert_eq!(resolve_color(&label(Some("combat"), None), &categories), Some(combat));
        assert_eq!(
            resolve_color(&label(Some("combat"), Some(0x123456)), &categories),
            Some(HexColor::new(0x123456))
        );
        assert_eq!(resolve_color(&label(None, None), &categories), None);
    }

    #[test]
    fn unknown_or_colourless_category_leaves_row_untinted() {
        let categories = default_content_categories();
        assert_eq!(resolve_color(&label(Some("deleted long ago"), None), &categories), None);
        assert_eq!(resolve_color(&label(Some(OTHER_CONTENT_CATEGORY), None), &categories), None);
    }

    #[test]
    fn category_names_are_matched_loosely() {
        let mut categories = default_content_categories();
        categories.push(ContentCategory {
            id: "Quality of life".into(),
            color: None,
        });

        assert!(category_exists("library", &categories));
        assert!(category_exists("  QUALITY of LIFE ", &categories));
        // Built-in ids are also taken under the name they're shown as
        assert!(category_exists(&category_name("worldgen"), &categories));
        assert!(!category_exists("Magic", &categories));
    }

    #[test]
    fn sorting_by_category_puts_other_then_uncategorised_last() {
        let mut categories = vec![
            None,
            Some("other"),
            Some("utility"),
            Some("Quality of life"),
            Some("library"),
            Some("  "),
            Some("combat"),
        ];
        categories.sort_by(|a, b| compare_categories(*a, *b));
        // Alphabetical by the shown names: Combat, Library, Quality of life, Utility
        assert_eq!(
            categories,
            [
                Some("combat"),
                Some("library"),
                Some("Quality of life"),
                Some("utility"),
                Some("other"),
                None,
                Some("  ")
            ]
        );
    }

    #[test]
    fn custom_category_keeps_its_own_name() {
        assert_eq!(category_name("Quality of life"), "Quality of life");
        assert_eq!(category_short_name("Quality of life"), "Quality of life");
    }
}
