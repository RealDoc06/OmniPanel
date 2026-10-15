//! Lucide icon markup embedded in the binary, looked up without copying.

use std::collections::HashMap;
use std::sync::OnceLock;

static ICONS_JSON: &str = include_str!("../../../packages/icons/icons.json");

/// Inner SVG markup (24x24 viewBox) for a Lucide icon name.
pub fn markup(name: &str) -> Option<&'static str> {
    static ICONS: OnceLock<HashMap<&'static str, &'static str>> = OnceLock::new();
    ICONS
        .get_or_init(|| serde_json::from_str(ICONS_JSON).expect("packages/icons/icons.json is valid"))
        .get(name)
        .copied()
}

#[cfg(test)]
mod tests {
    #[test]
    fn known_icon_resolves() {
        assert!(super::markup("mic-off").is_some_and(|m| m.starts_with("<path")));
        assert!(super::markup("not-an-icon").is_none());
    }
}
