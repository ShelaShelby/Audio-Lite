//! The command line. No arguments: the tray process. `--window ...`: the window
//! process, started by the tray. Pure, no Windows calls.

/// What the tray tells a window process it starts.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WindowArgs {
    /// Process id of the tray. The window process exits when the tray is gone.
    pub parent: u32,
    /// Middle of the tray icon in screen pixels: the window opens next to it.
    pub at: Option<(i32, i32)>,
    /// How long a hidden window waits before its process exits, in ms (default 30 s).
    pub linger_ms: Option<u64>,
    /// The tray icon could not be made: the window stays open and its X quits.
    pub no_tray: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Mode {
    Tray {
        cycle_test: Option<u32>,
    },
    Window(WindowArgs),
    /// `--window` without a usable parent: nothing to do.
    Invalid,
}

fn point(s: &str) -> Option<(i32, i32)> {
    let (x, y) = s.split_once(',')?;
    Some((x.trim().parse().ok()?, y.trim().parse().ok()?))
}

/// The arguments after the program name. Unknown ones are ignored.
pub fn parse(args: &[String]) -> Mode {
    let mut window = false;
    let mut w = WindowArgs::default();
    let mut cycle_test = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--window" => window = true,
            "--no-tray" => w.no_tray = true,
            "--parent" => w.parent = it.next().and_then(|v| v.parse().ok()).unwrap_or(0),
            "--at" => w.at = it.next().and_then(|v| point(v)),
            "--linger-ms" => w.linger_ms = it.next().and_then(|v| v.parse().ok()),
            "--cycle-test" => cycle_test = it.next().and_then(|v| v.parse().ok()).filter(|n| *n > 0),
            _ => {}
        }
    }
    match (window, w.parent) {
        (false, _) => Mode::Tray { cycle_test },
        (true, 0) => Mode::Invalid,
        (true, _) => Mode::Window(w),
    }
}

/// The arguments for `parse` to read back as `Mode::Window(w)`.
pub fn window_args(w: &WindowArgs) -> Vec<String> {
    let mut out = vec!["--window".to_string(), "--parent".to_string(), w.parent.to_string()];
    if let Some((x, y)) = w.at {
        out.extend(["--at".to_string(), format!("{x},{y}")]);
    }
    if let Some(ms) = w.linger_ms {
        out.extend(["--linger-ms".to_string(), ms.to_string()]);
    }
    if w.no_tray {
        out.push("--no-tray".to_string());
    }
    out
}

/// The full command line for CreateProcess: the exe in quotes, then the arguments
/// (which never contain spaces or quotes).
pub fn command_line(exe: &str, w: &WindowArgs) -> String {
    let mut s = format!("\"{exe}\"");
    for a in window_args(w) {
        s.push(' ');
        s.push_str(&a);
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &str) -> Vec<String> {
        s.split_whitespace().map(str::to_string).collect()
    }

    #[test]
    fn no_arguments_is_the_tray() {
        assert_eq!(parse(&[]), Mode::Tray { cycle_test: None });
        assert_eq!(parse(&v("--something else")), Mode::Tray { cycle_test: None });
    }

    #[test]
    fn cycle_test_count() {
        assert_eq!(parse(&v("--cycle-test 100")), Mode::Tray { cycle_test: Some(100) });
        assert_eq!(parse(&v("--cycle-test 0")), Mode::Tray { cycle_test: None });
        assert_eq!(parse(&v("--cycle-test x")), Mode::Tray { cycle_test: None });
        assert_eq!(parse(&v("--cycle-test")), Mode::Tray { cycle_test: None });
    }

    #[test]
    fn window_needs_a_parent() {
        assert_eq!(parse(&v("--window")), Mode::Invalid);
        assert_eq!(parse(&v("--window --parent 0")), Mode::Invalid);
        assert_eq!(parse(&v("--window --parent abc")), Mode::Invalid);
        assert_eq!(parse(&v("--window --parent")), Mode::Invalid);
    }

    #[test]
    fn window_arguments() {
        let m = parse(&v("--window --parent 1234 --at -1910,1050 --linger-ms 500 --no-tray"));
        let want = WindowArgs { parent: 1234, at: Some((-1910, 1050)), linger_ms: Some(500), no_tray: true };
        assert_eq!(m, Mode::Window(want));
        let m = parse(&v("--parent 7 --window --at 5"));
        assert_eq!(m, Mode::Window(WindowArgs { parent: 7, ..Default::default() }));
    }

    #[test]
    fn round_trip() {
        for w in [
            WindowArgs { parent: 1, ..Default::default() },
            WindowArgs { parent: 99999, at: Some((0, -3)), linger_ms: None, no_tray: false },
            WindowArgs { parent: 42, at: Some((3839, 2159)), linger_ms: Some(1000), no_tray: true },
        ] {
            assert_eq!(parse(&window_args(&w)), Mode::Window(w.clone()));
            let line = command_line("C:\\My Tools\\audio-lite.exe", &w);
            assert!(line.starts_with("\"C:\\My Tools\\audio-lite.exe\" --window"));
            let rest: Vec<String> = line.rsplit('"').next().unwrap().split_whitespace().map(str::to_string).collect();
            assert_eq!(parse(&rest), Mode::Window(w));
        }
    }
}
