//! Turns a profile page into the render-ready `DeckLayout` sent to decks.

use omni_core::{
    ActiveStyle, Config, ControlKind, ControlState, DeckControl, DeckControlKind, DeckIcon, DeckLayout, DeckSettings,
    Icon, PageRef, Profile, Theme, defaults,
};

use crate::icons;

/// Theme of a profile: its own, then the desktop theme, then the default.
pub fn resolve_theme(config: &Config, profile: &Profile) -> Theme {
    let builtin = defaults::builtin_themes();
    let find = |id: &str| config.themes.iter().chain(builtin.iter()).find(|t| t.id == id).cloned();
    profile
        .theme_id
        .as_deref()
        .and_then(find)
        .or_else(|| find(&config.settings.theme_id))
        .or_else(|| find(defaults::DEFAULT_THEME_ID))
        .expect("default theme exists")
}

pub fn resolve_icon(icon: &Icon) -> Option<DeckIcon> {
    match icon {
        Icon::Builtin { name } => icons::markup(name).map(|m| DeckIcon::Svg { markup: m.to_string() }),
        Icon::Emoji { value } => Some(DeckIcon::Emoji { value: value.clone() }),
        Icon::Image { id } => Some(DeckIcon::Image { url: format!("/img/{id}") }),
    }
}

/// Layout for `page_index` of `profile`; `state_of` supplies the current state per control.
/// `portrait` is the deck's screen orientation, when known.
pub fn build(
    config: &Config,
    profile: &Profile,
    page_index: usize,
    deck: &DeckSettings,
    portrait: Option<bool>,
    state_of: impl Fn(&str) -> ControlState,
) -> DeckLayout {
    let theme = resolve_theme(config, profile);
    let page_index = page_index.min(profile.pages.len() - 1);
    let page = &profile.pages[page_index];
    let controls = page
        .controls
        .iter()
        .map(|c| DeckControl {
            id: c.id.clone(),
            x: c.x,
            y: c.y,
            w: c.w,
            h: c.h,
            kind: match &c.kind {
                ControlKind::Button { .. } => DeckControlKind::Button,
                ControlKind::Slider { vertical, .. } => DeckControlKind::Slider { vertical: *vertical },
            },
            label: c.style.label.clone(),
            icon: c.style.icon.as_ref().and_then(resolve_icon),
            bg: c.style.bg.clone().unwrap_or_else(|| theme.colors.button.clone()),
            fg: c.style.fg.clone().unwrap_or_else(|| theme.colors.button_text.clone()),
            label_position: c.style.label_position,
            active: ActiveStyle {
                bg: c.style.active_bg.clone().unwrap_or_else(|| theme.colors.accent.clone()),
                label: c.style.active_label.clone(),
                icon: c.style.active_icon.as_ref().and_then(resolve_icon),
            },
            state: state_of(&c.id),
        })
        .collect();
    let mut layout = DeckLayout {
        profile_id: profile.id.clone(),
        profile_name: profile.name.clone(),
        page_id: page.id.clone(),
        page_name: page.name.clone(),
        page_index: page_index as u32,
        page_count: profile.pages.len() as u32,
        pages: profile.pages.iter().map(|p| PageRef { id: p.id.clone(), name: p.name.clone() }).collect(),
        rows: profile.rows,
        cols: profile.cols,
        theme: theme.colors,
        radius: theme.radius,
        settings: deck.clone(),
        controls,
    };
    let grid_portrait = layout.rows > layout.cols;
    if deck.auto_rotate && layout.rows != layout.cols && portrait.is_some_and(|p| p != grid_portrait) {
        transpose(&mut layout);
    }
    layout
}

/// Mirror the grid along its diagonal: rows <-> cols, x <-> y, w <-> h.
/// Sliders flip orientation so they still run along their span.
fn transpose(layout: &mut DeckLayout) {
    (layout.rows, layout.cols) = (layout.cols, layout.rows);
    for c in &mut layout.controls {
        (c.x, c.y, c.w, c.h) = (c.y, c.x, c.h, c.w);
        if let DeckControlKind::Slider { vertical } = &mut c.kind {
            *vertical = !*vertical;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_defaults_from_theme() {
        let config = Config::default();
        let profile = &config.profiles[0];
        let layout = build(&config, profile, 0, &DeckSettings::default(), None, |id| ControlState {
            id: id.into(),
            ..ControlState::default()
        });
        let theme = resolve_theme(&config, profile);
        assert_eq!(layout.page_count, 2);
        assert_eq!(layout.controls.len(), profile.pages[0].controls.len());
        let play = &layout.controls[0];
        assert_eq!(play.bg, theme.colors.button);
        assert!(matches!(play.icon, Some(DeckIcon::Svg { .. })));
        assert_eq!(play.active.bg, theme.colors.accent);
        // out of range page index clamps to the last page
        assert_eq!(
            build(&config, profile, 9, &DeckSettings::default(), None, |_| ControlState::default()).page_index,
            1
        );
    }

    #[test]
    fn portrait_decks_get_a_transposed_grid() {
        let config = Config::default();
        let profile = &config.profiles[0]; // 5 cols x 3 rows, master slider at x=4 spanning 3 rows
        let state = |_: &str| ControlState::default();
        let landscape = build(&config, profile, 0, &DeckSettings::default(), Some(false), state);
        let portrait = build(&config, profile, 0, &DeckSettings::default(), Some(true), state);
        assert_eq!((landscape.cols, landscape.rows), (5, 3));
        assert_eq!((portrait.cols, portrait.rows), (3, 5));
        let slider = portrait.controls.iter().find(|c| matches!(c.kind, DeckControlKind::Slider { .. })).unwrap();
        assert_eq!((slider.x, slider.y, slider.w, slider.h), (0, 4, 3, 1));
        assert_eq!(slider.kind, DeckControlKind::Slider { vertical: false });
        let fixed = DeckSettings { auto_rotate: false, ..DeckSettings::default() };
        assert_eq!(build(&config, profile, 0, &fixed, Some(true), state).cols, 5);
    }
}
