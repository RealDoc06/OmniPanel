//! XDG desktop entries (`.desktop` files): lookup by id, parsing and `Exec`
//! expansion. Only used on Linux, but plain text handling so it is tested everywhere.
#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use std::path::{Path, PathBuf};

use crate::proc::home_dir;

/// The parts of a `[Desktop Entry]` OmniPanel needs.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct Entry {
    pub name: String,
    pub exec: String,
    pub icon: Option<String>,
    /// Working directory (`Path=`).
    pub path: Option<String>,
    /// `NoDisplay=true` or `Hidden=true`: not meant for app menus.
    pub hidden: bool,
}

/// Parse the main group of a desktop entry. `None` when it is not an
/// application or has no name.
pub(crate) fn parse(text: &str) -> Option<Entry> {
    let mut in_main_group = false;
    let (mut entry, mut is_application) = (Entry::default(), false);
    for line in text.lines().map(str::trim) {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(group) = line.strip_prefix('[') {
            if in_main_group {
                break; // actions and other groups follow the main one
            }
            in_main_group = group.trim_end_matches(']') == "Desktop Entry";
            continue;
        }
        if !in_main_group {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else { continue };
        let value = value.trim();
        match key.trim() {
            "Type" => is_application = value == "Application",
            "Name" => entry.name = unescape(value),
            "Exec" => entry.exec = unescape(value),
            "Icon" => entry.icon = Some(value.to_owned()),
            "Path" => entry.path = Some(value.to_owned()),
            "NoDisplay" | "Hidden" => entry.hidden |= value == "true",
            _ => {}
        }
    }
    (is_application && !entry.name.is_empty()).then_some(entry)
}

/// Undo the desktop entry string escapes (`\s`, `\n`, `\t`, `\r`, `\\`).
fn unescape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('s') => out.push(' '),
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some(other) => out.push(other),
            None => out.push('\\'),
        }
    }
    out
}

/// Split an `Exec` value into arguments (double quotes group, backslash
/// escapes inside quotes) and expand field codes. `args` fill `%f %F %u %U`;
/// when the line has none of those, they are appended.
pub(crate) fn command_line(entry: &Entry, args: &[String]) -> Vec<String> {
    let mut argv = Vec::new();
    let mut used_args = false;
    for token in split_exec(&entry.exec) {
        match token.as_str() {
            "%F" | "%U" => {
                used_args = true;
                argv.extend(args.iter().cloned());
            }
            "%i" => argv.extend(entry.icon.iter().flat_map(|icon| ["--icon".to_owned(), icon.clone()])),
            _ => {
                let mut expanded = String::new();
                let mut chars = token.chars();
                while let Some(c) = chars.next() {
                    if c != '%' {
                        expanded.push(c);
                        continue;
                    }
                    match chars.next() {
                        Some('%') => expanded.push('%'),
                        Some('f' | 'u') => {
                            used_args = true;
                            expanded.push_str(args.first().map_or("", String::as_str));
                        }
                        Some('c') => expanded.push_str(&entry.name),
                        // %i inside a word, %k, deprecated codes: nothing to insert
                        _ => {}
                    }
                }
                if !expanded.is_empty() || !token.contains('%') {
                    argv.push(expanded);
                }
            }
        }
    }
    if !used_args {
        argv.extend(args.iter().cloned());
    }
    argv
}

fn split_exec(exec: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let (mut in_quotes, mut started) = (false, false);
    let mut chars = exec.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                in_quotes = !in_quotes;
                started = true;
            }
            '\\' if in_quotes => match chars.next() {
                Some(escaped) => current.push(escaped),
                None => current.push('\\'),
            },
            c if c.is_whitespace() && !in_quotes => {
                if started {
                    tokens.push(std::mem::take(&mut current));
                    started = false;
                }
            }
            c => {
                current.push(c);
                started = true;
            }
        }
    }
    if started {
        tokens.push(current);
    }
    tokens
}

/// Every `applications` directory, user dirs first.
pub(crate) fn application_dirs() -> Vec<PathBuf> {
    let home = home_dir();
    let data_home = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| home.as_ref().map(|h| h.join(".local/share")));
    let data_dirs = std::env::var("XDG_DATA_DIRS")
        .ok()
        .filter(|d| !d.is_empty())
        .unwrap_or_else(|| "/usr/local/share:/usr/share".into());

    let mut roots: Vec<PathBuf> = data_home.into_iter().collect();
    roots.extend(data_dirs.split(':').filter(|d| !d.is_empty()).map(PathBuf::from));
    // Flatpak and snap export their entries outside the usual dirs.
    roots.push("/var/lib/flatpak/exports/share".into());
    roots.extend(home.map(|h| h.join(".local/share/flatpak/exports/share")));
    roots.push("/var/lib/snapd/desktop".into());

    let mut dirs: Vec<PathBuf> = Vec::new();
    for root in roots {
        let dir = root.join("applications");
        if !dirs.contains(&dir) {
            dirs.push(dir);
        }
    }
    dirs
}

/// Locate a desktop entry by id (`firefox`, `org.mozilla.firefox.desktop`) or by file path.
pub(crate) fn find(app: &str) -> Option<PathBuf> {
    if app.ends_with(".desktop") && Path::new(app).is_absolute() {
        return Path::new(app).is_file().then(|| PathBuf::from(app));
    }
    let id = app.strip_suffix(".desktop").unwrap_or(app);
    if id.is_empty() || id.contains('/') {
        return None;
    }
    // "kde4-foo.desktop" may live in the sub-directory "kde4/foo.desktop".
    let nested = id.match_indices('-').map(|(i, _)| format!("{}/{}", &id[..i], &id[i + 1..]));
    let names: Vec<String> = std::iter::once(id.to_owned()).chain(nested).map(|n| format!("{n}.desktop")).collect();
    application_dirs().iter().flat_map(|dir| names.iter().map(move |name| dir.join(name))).find(|path| path.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIREFOX: &str = "\
# comment
[Desktop Entry]
Version=1.0
Type=Application
Name=Firefox
Name[de]=Feuerfuchs
Exec=firefox %u
Icon=firefox
Terminal=false

[Desktop Action new-window]
Name=New Window
Exec=firefox --new-window %u
";

    fn entry(exec: &str) -> Entry {
        Entry { name: "App".into(), exec: exec.into(), icon: Some("app-icon".into()), ..Entry::default() }
    }

    fn argv(exec: &str, args: &[&str]) -> Vec<String> {
        let args: Vec<String> = args.iter().map(|a| (*a).to_owned()).collect();
        command_line(&entry(exec), &args)
    }

    #[test]
    fn parses_the_main_group_only() {
        let parsed = parse(FIREFOX).unwrap();
        assert_eq!(parsed.name, "Firefox");
        assert_eq!(parsed.exec, "firefox %u");
        assert_eq!(parsed.icon.as_deref(), Some("firefox"));
        assert!(!parsed.hidden);
    }

    #[test]
    fn skips_non_applications_and_flags_hidden_entries() {
        assert_eq!(parse("[Desktop Entry]\nType=Link\nName=Docs\nURL=https://example.com"), None);
        assert!(parse("[Desktop Entry]\nType=Application\nName=X\nExec=x\nNoDisplay=true").unwrap().hidden);
        assert!(parse("[Desktop Entry]\nType=Application\nName=X\nExec=x\nHidden=true").unwrap().hidden);
    }

    #[test]
    fn exec_field_codes() {
        assert_eq!(argv("firefox %u", &[]), ["firefox"]);
        assert_eq!(argv("firefox %u", &["https://a.test"]), ["firefox", "https://a.test"]);
        assert_eq!(argv("gimp %F", &["a.png", "b.png"]), ["gimp", "a.png", "b.png"]);
        assert_eq!(argv("viewer --file=%f", &["a.png"]), ["viewer", "--file=a.png"]);
        assert_eq!(argv("app %i --name %c", &[]), ["app", "--icon", "app-icon", "--name", "App"]);
        assert_eq!(argv("app 100%% %k %v", &[]), ["app", "100%"]);
    }

    #[test]
    fn exec_without_field_code_appends_args() {
        assert_eq!(argv("code", &["--new-window"]), ["code", "--new-window"]);
    }

    #[test]
    fn exec_quoting() {
        assert_eq!(argv(r#""/opt/My App/run" --name "a b" %U"#, &["x"]), ["/opt/My App/run", "--name", "a b", "x"]);
        assert_eq!(argv(r#"sh -c "echo \"hi\"""#, &[]), ["sh", "-c", r#"echo "hi""#]);
        assert_eq!(argv("env  FOO=1   app", &[]), ["env", "FOO=1", "app"]);
    }

    #[test]
    fn string_escapes() {
        assert_eq!(unescape(r"a\sb\\c"), r"a b\c");
    }
}
