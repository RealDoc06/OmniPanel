//! Installed application discovery for the "Open app" picker.

use std::collections::HashSet;
use std::fs;
use std::path::Path;

use omni_core::InstalledApp;

/// Installed GUI applications, sorted by name. Blocking (scans the disk).
///
/// `launch` is what `Action::OpenApp` takes: the `.app` path on macOS, the
/// Start Menu shortcut on Windows, the desktop entry id on Linux.
pub fn installed() -> Vec<InstalledApp> {
    let mut seen = HashSet::new();
    // Earlier (user, then system) entries win over later ones with the same name.
    let mut apps: Vec<InstalledApp> =
        platform_apps().into_iter().filter(|app| seen.insert(app.name.to_lowercase())).collect();
    apps.sort_by_cached_key(|app| app.name.to_lowercase());
    apps
}

/// Immediate children of `dir`, ignoring unreadable directories.
fn children(dir: &Path) -> impl Iterator<Item = std::path::PathBuf> {
    fs::read_dir(dir).into_iter().flatten().flatten().map(|entry| entry.path())
}

#[cfg(target_os = "macos")]
fn platform_apps() -> Vec<InstalledApp> {
    let mut roots = vec!["/Applications".into(), "/System/Applications".into()];
    roots.extend(crate::proc::home_dir().map(|home| home.join("Applications")));

    let mut apps = Vec::new();
    for root in roots {
        collect_bundles(&root, 2, &mut apps);
    }
    apps
}

/// `.app` bundles in `dir`, looking into plain folders (`Utilities`, vendor folders) too.
#[cfg(target_os = "macos")]
fn collect_bundles(dir: &Path, depth: u32, apps: &mut Vec<InstalledApp>) {
    for path in children(dir) {
        if path.extension().is_some_and(|ext| ext == "app") {
            if let Some(name) = path.file_stem() {
                apps.push(InstalledApp {
                    name: name.to_string_lossy().into_owned(),
                    launch: path.to_string_lossy().into_owned(),
                });
            }
        } else if depth > 1 && path.is_dir() {
            collect_bundles(&path, depth - 1, apps);
        }
    }
}

#[cfg(windows)]
fn platform_apps() -> Vec<InstalledApp> {
    let programs =
        |var: &str| std::env::var_os(var).map(|base| Path::new(&base).join(r"Microsoft\Windows\Start Menu\Programs"));
    let mut apps = Vec::new();
    for root in [programs("APPDATA"), programs("ProgramData")].into_iter().flatten() {
        collect_shortcuts(&root, 4, &mut apps);
    }
    apps
}

#[cfg(windows)]
fn collect_shortcuts(dir: &Path, depth: u32, apps: &mut Vec<InstalledApp>) {
    for path in children(dir) {
        if path.is_dir() {
            if depth > 1 {
                collect_shortcuts(&path, depth - 1, apps);
            }
        } else if path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("lnk")) {
            let Some(name) = path.file_stem().map(|n| n.to_string_lossy().into_owned()) else { continue };
            if !name.to_lowercase().contains("uninstall") {
                apps.push(InstalledApp { name, launch: path.to_string_lossy().into_owned() });
            }
        }
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
fn platform_apps() -> Vec<InstalledApp> {
    crate::desktop::application_dirs().iter().flat_map(|dir| desktop_entries(dir)).collect()
}

/// Visible applications of one `applications` directory; sub-directories
/// become dashes in the id (`kde4/foo.desktop` is `kde4-foo.desktop`).
#[cfg(any(not(any(target_os = "macos", windows)), test))]
fn desktop_entries(dir: &Path) -> Vec<InstalledApp> {
    fn walk(root: &Path, dir: &Path, depth: u32, apps: &mut Vec<InstalledApp>) {
        for path in children(dir) {
            if path.is_dir() {
                if depth > 1 {
                    walk(root, &path, depth - 1, apps);
                }
                continue;
            }
            if path.extension().is_none_or(|ext| ext != "desktop") {
                continue;
            }
            let Some(entry) = fs::read_to_string(&path).ok().and_then(|text| crate::desktop::parse(&text)) else {
                continue;
            };
            let Ok(relative) = path.strip_prefix(root) else { continue };
            if !entry.hidden && !entry.exec.is_empty() {
                let id = relative.to_string_lossy().replace(std::path::MAIN_SEPARATOR, "-");
                apps.push(InstalledApp { name: entry.name, launch: id });
            }
        }
    }
    let mut apps = Vec::new();
    walk(dir, dir, 3, &mut apps);
    apps
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_visible_desktop_entries_with_ids() {
        let root = std::env::temp_dir().join(format!("omni-apps-{}", omni_core::new_id()));
        let write = |name: &str, text: &str| {
            let path = root.join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, text).unwrap();
        };
        write("firefox.desktop", "[Desktop Entry]\nType=Application\nName=Firefox\nExec=firefox %u");
        write("kde4/konsole.desktop", "[Desktop Entry]\nType=Application\nName=Konsole\nExec=konsole");
        write("hidden.desktop", "[Desktop Entry]\nType=Application\nName=Hidden\nExec=x\nNoDisplay=true");
        write("notes.txt", "[Desktop Entry]\nType=Application\nName=Not an entry\nExec=x");

        let mut apps = desktop_entries(&root);
        apps.sort_by(|a, b| a.name.cmp(&b.name));
        fs::remove_dir_all(&root).unwrap();
        let apps: Vec<_> = apps.iter().map(|a| (a.name.as_str(), a.launch.as_str())).collect();
        assert_eq!(apps, [("Firefox", "firefox.desktop"), ("Konsole", "kde4-konsole.desktop")]);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn lists_system_apps_sorted_and_unique() {
        let apps = installed();
        let calculator = apps.iter().find(|a| a.name == "Calculator").expect("Calculator is a system app");
        assert!(calculator.launch.ends_with("Calculator.app"));
        assert!(apps.windows(2).all(|pair| pair[0].name.to_lowercase() <= pair[1].name.to_lowercase()));
        let names: HashSet<_> = apps.iter().map(|a| a.name.to_lowercase()).collect();
        assert_eq!(names.len(), apps.len());
    }
}
