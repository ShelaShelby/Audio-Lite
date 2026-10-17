//! Audio sessions to mixer rows: one row per app. Pure, no Windows calls.

/// One audio session as Windows reports it. `handle` is whatever controls its
/// volume (a COM pointer in the app, `()` in tests).
#[derive(Clone, Debug)]
pub struct Session<T> {
    /// Session instance id, unique per session.
    pub id: String,
    pub pid: u32,
    /// Full exe path, if the process could be read.
    pub exe: Option<String>,
    /// The exe's file description ("Google Chrome"), if it has one.
    pub description: Option<String>,
    /// The name the session gives itself, if any.
    pub display_name: Option<String>,
    pub system: bool,
    pub expired: bool,
    pub volume: f32,
    pub muted: bool,
    pub handle: T,
}

/// One app in the mixer: all its sessions together.
#[derive(Clone, Debug, PartialEq)]
pub struct Row<T> {
    /// Same app, same key, from one refresh to the next.
    pub key: String,
    pub name: String,
    /// Session ids, sorted. Part of the row's identity.
    pub ids: Vec<String>,
    /// Volume of the first session (the slider moves all of them together).
    pub volume: f32,
    /// True only if every session is muted.
    pub muted: bool,
    pub handles: Vec<T>,
}

pub const SYSTEM_SOUNDS: &str = "System sounds";

fn key_of<T>(s: &Session<T>) -> String {
    if s.system {
        "system".to_string()
    } else if let Some(exe) = s.exe.as_deref().filter(|e| !e.is_empty()) {
        format!("exe:{}", exe.to_lowercase())
    } else {
        format!("pid:{}", s.pid)
    }
}

/// "chrome" from "C:\Program Files\Google\chrome.exe".
fn exe_stem(path: &str) -> Option<String> {
    let file = path.rsplit(['\\', '/']).next()?;
    let stem = match file.len().checked_sub(4) {
        Some(i) if file.is_char_boundary(i) && file[i..].eq_ignore_ascii_case(".exe") => &file[..i],
        _ => file,
    };
    (!stem.is_empty()).then(|| stem.to_string())
}

fn clean(s: &Option<String>) -> Option<String> {
    s.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string)
}

/// The name shown for a session: "System sounds", the exe's description, the
/// exe name without ".exe", the session's own name (unless it is a resource
/// reference like "@%SystemRoot%\..."), or "Process <pid>".
pub fn name_of<T>(s: &Session<T>) -> String {
    if s.system {
        return SYSTEM_SOUNDS.to_string();
    }
    clean(&s.description)
        .or_else(|| s.exe.as_deref().and_then(exe_stem))
        .or_else(|| clean(&s.display_name).filter(|n| !n.starts_with('@')))
        .unwrap_or_else(|| format!("Process {}", s.pid))
}

/// Merge sessions into rows: expired sessions are skipped, sessions of the same
/// exe (or the same process when the exe is unknown) share a row. Rows are
/// sorted by name, with "System sounds" last.
pub fn merge<T>(sessions: Vec<Session<T>>) -> Vec<Row<T>> {
    let mut rows: Vec<Row<T>> = Vec::new();
    for s in sessions.into_iter().filter(|s| !s.expired) {
        let key = key_of(&s);
        match rows.iter_mut().find(|r| r.key == key) {
            Some(r) => {
                r.ids.push(s.id);
                r.muted &= s.muted;
                r.handles.push(s.handle);
            }
            None => rows.push(Row {
                name: name_of(&s),
                key,
                ids: vec![s.id],
                volume: s.volume,
                muted: s.muted,
                handles: vec![s.handle],
            }),
        }
    }
    for r in &mut rows {
        r.ids.sort();
    }
    rows.sort_by(|a, b| {
        (a.key == "system", a.name.to_lowercase(), &a.key).cmp(&(b.key == "system", b.name.to_lowercase(), &b.key))
    });
    rows
}

/// True if the rows stand for different apps or sessions than before (so the
/// list has to be built again). Volume and mute changes do not count.
pub fn rows_changed<A, B>(old: &[Row<A>], new: &[Row<B>]) -> bool {
    old.len() != new.len() || old.iter().zip(new).any(|(a, b)| a.key != b.key || a.ids != b.ids)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(id: &str, pid: u32, exe: Option<&str>) -> Session<()> {
        Session {
            id: id.to_string(),
            pid,
            exe: exe.map(str::to_string),
            description: None,
            display_name: None,
            system: false,
            expired: false,
            volume: 1.0,
            muted: false,
            handle: (),
        }
    }

    #[test]
    fn same_process_is_one_row() {
        let mut a = s("a", 10, Some("C:\\App\\player.exe"));
        a.volume = 0.3;
        let b = s("b", 10, Some("C:\\App\\player.exe"));
        let rows = merge(vec![a, b]);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "player");
        assert_eq!(rows[0].ids, vec!["a", "b"]);
        assert_eq!(rows[0].handles.len(), 2);
        assert_eq!(rows[0].volume, 0.3);
    }

    #[test]
    fn same_exe_in_two_processes_is_one_row() {
        let rows = merge(vec![s("a", 10, Some("C:\\B\\chrome.exe")), s("b", 11, Some("c:\\b\\CHROME.EXE"))]);
        assert_eq!(rows.len(), 1);
    }

    #[test]
    fn unknown_exe_merges_by_pid_only() {
        let rows = merge(vec![s("a", 10, None), s("b", 10, None), s("c", 11, None)]);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].name, "Process 10");
        assert_eq!(rows[1].name, "Process 11");
    }

    #[test]
    fn expired_sessions_are_skipped() {
        let mut gone = s("a", 10, Some("C:\\old.exe"));
        gone.expired = true;
        let rows = merge(vec![gone, s("b", 11, Some("C:\\new.exe"))]);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "new");
        assert!(merge(
            vec![s("x", 1, None)]
                .into_iter()
                .map(|mut x| {
                    x.expired = true;
                    x
                })
                .collect::<Vec<_>>()
        )
        .is_empty());
    }

    #[test]
    fn names_and_fallbacks() {
        let mut d = s("a", 5, Some("C:\\x\\vlc.exe"));
        d.description = Some(" VLC media player ".into());
        assert_eq!(name_of(&d), "VLC media player");
        d.description = Some("  ".into());
        assert_eq!(name_of(&d), "vlc");
        let mut n = s("b", 6, None);
        n.display_name = Some("@%SystemRoot%\\System32\\AudioSrv.Dll,-202".into());
        assert_eq!(name_of(&n), "Process 6");
        n.display_name = Some("Game".into());
        assert_eq!(name_of(&n), "Game");
        assert_eq!(name_of(&s("c", 7, Some("C:\\tools\\noext"))), "noext");
        assert_eq!(name_of(&s("c", 7, Some("/x/Player.EXE"))), "Player");
        let mut sys = s("d", 0, None);
        sys.system = true;
        sys.description = Some("ignored".into());
        assert_eq!(name_of(&sys), SYSTEM_SOUNDS);
    }

    #[test]
    fn mute_only_when_all_sessions_muted_and_system_sounds_last() {
        let mut a = s("a", 1, Some("C:\\zed.exe"));
        a.muted = true;
        let b = s("b", 1, Some("C:\\zed.exe"));
        let mut sys = s("c", 0, None);
        sys.system = true;
        let rows = merge(vec![sys, a, b, s("d", 2, Some("C:\\Alpha.exe"))]);
        let names: Vec<_> = rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, vec!["Alpha", "zed", SYSTEM_SOUNDS]);
        assert!(!rows[1].muted);
    }

    #[test]
    fn change_detection() {
        let one = merge(vec![s("a", 1, Some("C:\\a.exe"))]);
        let mut louder = s("a", 1, Some("C:\\a.exe"));
        louder.volume = 0.1;
        louder.muted = true;
        assert!(!rows_changed(&one, &merge(vec![louder])));
        assert!(rows_changed(&one, &merge(vec![s("a2", 1, Some("C:\\a.exe"))])));
        assert!(rows_changed(&one, &merge(vec![s("a", 1, Some("C:\\a.exe")), s("b", 2, Some("C:\\b.exe"))])));
        assert!(rows_changed(&one, &merge::<()>(vec![])));
        assert!(!rows_changed(&merge::<()>(vec![]), &merge::<()>(vec![])));
    }
}
