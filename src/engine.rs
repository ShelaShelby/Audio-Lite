//! The audio thread. It owns every Core Audio object, so the window never waits
//! for Windows audio: the window reads a copy of `Model` and leaves requests
//! here. Volume requests are coalesced (latest wins, about 60 writes a second,
//! see `coalesce`); everything else runs in the order it was asked for.
//!
//! Changes made elsewhere (media keys, a headset wheel, other apps) come as
//! Windows events while the window is open. The callbacks only leave a request
//! here; the audio thread reads the new values and asks for one redraw. While
//! the window is hidden no callback is registered at all.

use crate::audio::{self, Handle};
use crate::coalesce::{self, Coalescer};
use crate::mixer::{self, Row};
use crate::volume;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::Instant;
use windows::core::{implement, Result as WinResult, GUID, PCWSTR};
use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
use windows::Win32::Foundation::BOOL;
use windows::Win32::Media::Audio::Endpoints::{
    IAudioEndpointVolume, IAudioEndpointVolumeCallback, IAudioEndpointVolumeCallback_Impl,
};
use windows::Win32::Media::Audio::{
    eConsole, eRender, AudioSessionDisconnectReason, AudioSessionState, AudioSessionStateExpired, EDataFlow, ERole,
    IAudioSessionControl, IAudioSessionEvents, IAudioSessionEvents_Impl, IAudioSessionManager2,
    IAudioSessionNotification, IAudioSessionNotification_Impl, IMMDevice, IMMDeviceEnumerator, IMMNotificationClient,
    IMMNotificationClient_Impl, AUDIO_VOLUME_NOTIFICATION_DATA, DEVICE_STATE,
};
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};
use windows::Win32::UI::Shell::PropertiesSystem::PROPERTYKEY;

pub use crate::model::{AppLevel, Model, Target};

enum Cmd {
    Refresh,
    /// Read the volumes and mutes again (a change event, or the window's safety poll).
    Values,
    /// The list of apps changed. Carries the new session when Windows reported one.
    Sessions(Option<NewSession>),
    SetDefault(String),
    Mute(Target, bool),
    /// The window was hidden: let go of the change events.
    Unwatch,
    Quit(std::sync::mpsc::Sender<()>),
}

/// A session Windows reported as new, for the audio thread. Core Audio's session
/// objects are free-threaded and the audio thread is in the multithreaded
/// apartment, so the pointer may be used there.
struct NewSession(IAudioSessionControl);
// SAFETY: see above; the object is only used on the audio thread.
unsafe impl Send for NewSession {}

struct Inbox {
    volumes: Coalescer<Target>,
    cmds: Vec<Cmd>,
    /// The slider being dragged. Change events leave it alone and no full
    /// refresh runs until it is let go.
    held: Option<Target>,
}

/// Volumes and mutes the user set before what they control was read (the
/// window shows cache.json until then). They are written as soon as it is.
#[derive(Default)]
struct Late {
    volumes: Vec<(Target, f32)>,
    mutes: Vec<(Target, bool)>,
}

impl Late {
    fn volume(&mut self, t: Target, v: f32) {
        self.volumes.retain(|(x, _)| *x != t);
        self.volumes.push((t, v));
    }

    fn mute(&mut self, t: Target, on: bool) {
        self.mutes.retain(|(x, _)| *x != t);
        self.mutes.push((t, on));
    }
}

impl Inbox {
    /// The user owns these values right now (a slider being dragged, a volume or
    /// mute not written yet): keep the window's values from `old` for them.
    fn keep_user_values(&self, late: &Late, old: &Model, new: &mut Model) {
        let volumes =
            self.volumes.pending().iter().map(|(t, _)| t).chain(&self.held).chain(late.volumes.iter().map(|(t, _)| t));
        for t in volumes {
            if let Some(v) = old.level(t) {
                new.set_level(t, v);
            }
        }
        let mutes = self.cmds.iter().filter_map(|c| match c {
            Cmd::Mute(t, _) => Some(t),
            _ => None,
        });
        for t in mutes.chain(late.mutes.iter().map(|(t, _)| t)) {
            if let Some(on) = old.muted(t) {
                new.set_muted(t, on);
            }
        }
    }
}

pub struct Engine {
    model: Mutex<Model>,
    inbox: Mutex<Inbox>,
    wake: Condvar,
    /// Called on the audio thread after the model changed.
    changed: fn(),
    /// Called on the audio thread after only volumes or mutes changed.
    redraw: fn(),
    /// The window is open: follow change events. While it is closed none are
    /// registered (opening it reads everything again).
    live: AtomicBool,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Engine {
    /// Start the audio thread. It reads everything once right away; until then
    /// the window shows `first` (the last state, from cache.json).
    pub fn start(changed: fn(), redraw: fn(), first: Model) -> Arc<Engine> {
        let engine = Arc::new(Engine {
            model: Mutex::new(first),
            inbox: Mutex::new(Inbox { volumes: Coalescer::new(coalesce::GAP), cmds: vec![Cmd::Refresh], held: None }),
            wake: Condvar::new(),
            changed,
            redraw,
            live: AtomicBool::new(false),
        });
        let e = engine.clone();
        // Builder, not thread::spawn: that one panics (aborts) if Windows refuses a
        // thread. Without the thread the window shows no devices but still works.
        let _ = std::thread::Builder::new().spawn(move || run(e));
        engine
    }

    /// A copy of what the window shows.
    pub fn model(&self) -> Model {
        lock(&self.model).clone()
    }

    fn send(&self, cmd: Cmd) {
        let mut inbox = lock(&self.inbox);
        // One waiting refresh (or read of the values, or of the app list) is enough.
        let same = |c: &Cmd| {
            matches!(
                (&cmd, c),
                (Cmd::Refresh, Cmd::Refresh) | (Cmd::Values, Cmd::Values) | (Cmd::Sessions(None), Cmd::Sessions(_))
            )
        };
        if !inbox.cmds.iter().any(same) {
            inbox.cmds.push(cmd);
        }
        self.wake.notify_one();
    }

    /// From a Windows callback: never blocks for long, never calls Windows audio.
    fn event(&self, cmd: impl FnOnce() -> Cmd) {
        if self.live.load(Ordering::Relaxed) {
            self.send(cmd());
        }
    }

    pub fn refresh(&self) {
        self.send(Cmd::Refresh);
    }

    /// Read only the volumes and mutes again (cheap; the window's safety poll).
    pub fn poll(&self) {
        self.send(Cmd::Values);
    }

    /// The window was opened (true) or closed (false). Closing also ends a drag
    /// and unregisters the change events; opening is followed by `refresh`,
    /// which registers them again.
    pub fn set_live(&self, on: bool) {
        self.live.store(on, Ordering::Relaxed);
        if !on {
            lock(&self.inbox).held = None;
            self.send(Cmd::Unwatch);
        }
    }

    pub fn set_default(&self, id: String) {
        lock(&self.model).default_id = Some(id.clone());
        self.send(Cmd::SetDefault(id));
    }

    /// Ask for a volume (0.0-1.0). Only the newest value per target is written.
    /// `last` (slider let go, click on the track, arrow key) writes it without
    /// waiting and ends the drag; otherwise the slider is being dragged.
    pub fn set_volume(&self, t: Target, v: f32, last: bool) {
        // NaN would be refused by Windows and shown as 0.
        let Some(v) = volume::to_scalar(v) else { return };
        // Model and inbox change together, so a read of the values never sees one without the other.
        let mut inbox = lock(&self.inbox);
        lock(&self.model).set_level(&t, v);
        if last {
            inbox.held = None;
            inbox.volumes.set_final(t, v);
        } else {
            inbox.held = Some(t.clone());
            inbox.volumes.set(t, v);
        }
        self.wake.notify_one();
    }

    pub fn set_mute(&self, t: Target, on: bool) {
        let mut inbox = lock(&self.inbox);
        lock(&self.model).set_muted(&t, on);
        inbox.cmds.push(Cmd::Mute(t, on));
        self.wake.notify_one();
    }

    /// Stop the audio thread (it lets go of its audio objects), waiting at most 2 s.
    pub fn quit(&self) {
        let (tx, rx) = std::sync::mpsc::channel();
        self.send(Cmd::Quit(tx));
        let _ = rx.recv_timeout(std::time::Duration::from_secs(2));
    }
}

/// Asks for a refresh when the default device changes or a device comes or goes
/// (only while the window is open).
#[implement(IMMNotificationClient)]
struct Notifier {
    engine: Arc<Engine>,
}

impl IMMNotificationClient_Impl for Notifier_Impl {
    fn OnDeviceStateChanged(&self, _: &PCWSTR, _: DEVICE_STATE) -> WinResult<()> {
        self.engine.event(|| Cmd::Refresh);
        Ok(())
    }
    fn OnDeviceAdded(&self, _: &PCWSTR) -> WinResult<()> {
        Ok(())
    }
    fn OnDeviceRemoved(&self, _: &PCWSTR) -> WinResult<()> {
        Ok(())
    }
    fn OnDefaultDeviceChanged(&self, flow: EDataFlow, role: ERole, _: &PCWSTR) -> WinResult<()> {
        if flow == eRender && role == eConsole {
            self.engine.event(|| Cmd::Refresh);
        }
        Ok(())
    }
    fn OnPropertyValueChanged(&self, _: &PCWSTR, key: &PROPERTYKEY) -> WinResult<()> {
        // A device was renamed in Windows' sound settings.
        if *key == PKEY_Device_FriendlyName {
            self.engine.event(|| Cmd::Refresh);
        }
        Ok(())
    }
}

/// The default device's master volume or mute changed.
#[implement(IAudioEndpointVolumeCallback)]
struct MasterEvents {
    engine: Arc<Engine>,
}

impl IAudioEndpointVolumeCallback_Impl for MasterEvents_Impl {
    fn OnNotify(&self, data: *mut AUDIO_VOLUME_NOTIFICATION_DATA) -> WinResult<()> {
        // The app's own changes are already in the model.
        let own = !data.is_null() && unsafe { (*data).guidEventContext } == audio::APP_CONTEXT;
        if !own {
            self.engine.event(|| Cmd::Values);
        }
        Ok(())
    }
}

/// An app started sound on the default device.
#[implement(IAudioSessionNotification)]
struct NewSessions {
    engine: Arc<Engine>,
}

impl IAudioSessionNotification_Impl for NewSessions_Impl {
    fn OnSessionCreated(&self, session: Option<&IAudioSessionControl>) -> WinResult<()> {
        self.engine.event(|| Cmd::Sessions(session.cloned().map(NewSession)));
        Ok(())
    }
}

/// One object for every session: an app's volume or mute changed, or it ended.
#[implement(IAudioSessionEvents)]
struct AppEvents {
    engine: Arc<Engine>,
}

impl IAudioSessionEvents_Impl for AppEvents_Impl {
    fn OnDisplayNameChanged(&self, _: &PCWSTR, _: *const GUID) -> WinResult<()> {
        Ok(())
    }
    fn OnIconPathChanged(&self, _: &PCWSTR, _: *const GUID) -> WinResult<()> {
        Ok(())
    }
    fn OnSimpleVolumeChanged(&self, _: f32, _: BOOL, context: *const GUID) -> WinResult<()> {
        if !audio::is_own(context) {
            self.engine.event(|| Cmd::Values);
        }
        Ok(())
    }
    fn OnChannelVolumeChanged(&self, _: u32, _: *const f32, _: u32, _: *const GUID) -> WinResult<()> {
        Ok(())
    }
    fn OnGroupingParamChanged(&self, _: *const GUID, _: *const GUID) -> WinResult<()> {
        Ok(())
    }
    fn OnStateChanged(&self, state: AudioSessionState) -> WinResult<()> {
        if state == AudioSessionStateExpired {
            self.engine.event(|| Cmd::Sessions(None));
        }
        Ok(())
    }
    fn OnSessionDisconnected(&self, _: AudioSessionDisconnectReason) -> WinResult<()> {
        self.engine.event(|| Cmd::Sessions(None));
        Ok(())
    }
}

/// The callback objects, made once and registered again for every new device.
struct Callbacks {
    master: IAudioEndpointVolumeCallback,
    sessions: IAudioSessionNotification,
    app: IAudioSessionEvents,
}

/// The audio objects, only ever used on the audio thread.
struct Audio {
    engine: Arc<Engine>,
    enumerator: Option<IMMDeviceEnumerator>,
    device: Option<IMMDevice>,
    master: Option<IAudioEndpointVolume>,
    manager: Option<IAudioSessionManager2>,
    rows: Vec<Row<Handle>>,
    /// Sessions Windows reported as new. Its session list may not have them yet.
    new_sessions: Vec<IAudioSessionControl>,
    names: audio::NameCache,
    callbacks: Callbacks,
    /// Where the callbacks are registered now, to unregister them again.
    master_watched: bool,
    sessions_watched: bool,
    apps_watched: Vec<IAudioSessionControl>,
    /// A refresh asked for while a slider was dragged; it runs when it is let go.
    deferred: bool,
    late: Late,
}

impl Audio {
    /// False if there is nothing to write to (yet).
    fn write_volume(&mut self, t: &Target, v: f32) -> bool {
        match t {
            Target::Master => match &self.master {
                Some(m) => {
                    audio::set_endpoint_volume(m, v);
                    true
                }
                None => false,
            },
            Target::App(k) => match self.rows.iter_mut().find(|r| &r.key == k) {
                Some(r) => {
                    for h in &r.handles {
                        audio::set_session_volume(&h.volume, v);
                    }
                    r.volume = v;
                    true
                }
                None => false,
            },
        }
    }

    /// False if there is nothing to write to (yet).
    fn write_mute(&mut self, t: &Target, on: bool) -> bool {
        match t {
            Target::Master => match &self.master {
                Some(m) => {
                    audio::set_endpoint_mute(m, on);
                    true
                }
                None => false,
            },
            Target::App(k) => match self.rows.iter_mut().find(|r| &r.key == k) {
                Some(r) => {
                    for h in &r.handles {
                        audio::set_session_mute(&h.volume, on);
                    }
                    r.muted = on;
                    true
                }
                None => false,
            },
        }
    }

    /// Write what the user set before it could be written. `last`: after a
    /// full read; what is still missing then is gone (an app that quit).
    fn flush_late(&mut self, last: bool) {
        for (t, v) in std::mem::take(&mut self.late.volumes) {
            if !self.write_volume(&t, v) && !last {
                self.late.volume(t, v);
            }
        }
        for (t, on) in std::mem::take(&mut self.late.mutes) {
            if !self.write_mute(&t, on) && !last {
                self.late.mute(t, on);
            }
        }
    }

    /// The window's model becomes `m`, keeping what the user owns right now.
    /// `note`: Some(..) replaces the switching note, None keeps the one shown.
    fn store(&self, mut m: Model, note: Option<Option<String>>) {
        let inbox = lock(&self.engine.inbox);
        let mut model = lock(&self.engine.model);
        inbox.keep_user_values(&self.late, &model, &mut m);
        m.note = note.unwrap_or_else(|| model.note.take());
        *model = m;
    }

    fn watch_apps(&mut self) {
        // A change event that came in just before the window was hidden must not
        // register callbacks again while it is hidden.
        if !self.engine.live.load(Ordering::Relaxed) {
            return;
        }
        for h in self.rows.iter().flat_map(|r| &r.handles) {
            if unsafe { h.control.RegisterAudioSessionNotification(&self.callbacks.app) }.is_ok() {
                self.apps_watched.push(h.control.clone());
            }
        }
    }

    fn unwatch_apps(&mut self) {
        for c in self.apps_watched.drain(..) {
            unsafe {
                let _ = c.UnregisterAudioSessionNotification(&self.callbacks.app);
            }
        }
    }

    /// Register the change events on the current device, its master volume and its apps.
    fn watch(&mut self) {
        if !self.engine.live.load(Ordering::Relaxed) {
            return;
        }
        unsafe {
            if let Some(m) = &self.master {
                self.master_watched = m.RegisterControlChangeNotify(&self.callbacks.master).is_ok();
            }
            if let Some(mgr) = &self.manager {
                self.sessions_watched = mgr.RegisterSessionNotification(&self.callbacks.sessions).is_ok();
            }
        }
        self.watch_apps();
    }

    fn unwatch(&mut self) {
        unsafe {
            if let Some(m) = self.master.as_ref().filter(|_| self.master_watched) {
                let _ = m.UnregisterControlChangeNotify(&self.callbacks.master);
            }
            if let Some(mgr) = self.manager.as_ref().filter(|_| self.sessions_watched) {
                let _ = mgr.UnregisterSessionNotification(&self.callbacks.sessions);
            }
        }
        self.master_watched = false;
        self.sessions_watched = false;
        self.unwatch_apps();
    }

    fn app_levels(&self) -> Vec<AppLevel> {
        self.rows
            .iter()
            .map(|r| AppLevel { key: r.key.clone(), name: r.name.clone(), volume: r.volume, muted: r.muted })
            .collect()
    }

    /// Read everything again: devices, the default one and its volume first
    /// (the window shows them at once), then its apps. The change events are
    /// registered again on what was read.
    fn refresh(&mut self, note: Option<Option<String>>) {
        self.unwatch();
        let mut m = Model::default();
        self.device = None;
        self.master = None;
        self.manager = None;
        self.new_sessions.clear();
        let old_rows = std::mem::take(&mut self.rows);
        let mut device = None;
        match &self.enumerator {
            None => m.error = Some("Windows audio is not available".into()),
            Some(e) => {
                match audio::devices(e) {
                    Ok(list) => m.devices = list,
                    Err(err) => m.error = Some(format!("Could not list devices: {}", err.message())),
                }
                match audio::default_endpoint(e) {
                    Ok((d, control)) => {
                        let info = audio::device_info(&d);
                        m.default_name =
                            info.as_ref().map_or_else(|| audio::device_display_name(&d), |i| i.name.clone());
                        m.default_id = info.map(|i| i.id);
                        match audio::endpoint_state(&control) {
                            Ok(state) => m.master = Some(state),
                            Err(err) => m.error = Some(format!("Could not read the volume: {}", err.message())),
                        }
                        self.master = Some(control);
                        device = Some(d);
                    }
                    Err(_) => {
                        m.default_name = "No playback device".into();
                        m.error.get_or_insert_with(|| "No audio output device found".into());
                    }
                }
            }
        }
        let mut note = note;
        if let Some(d) = device {
            // Shown at once; the apps of the same device stay as they were until read.
            let mut first = m.clone();
            let same = lock(&self.engine.model).default_id == first.default_id;
            if same {
                first.apps = lock(&self.engine.model).apps.clone();
            }
            self.store(first, note.take());
            self.flush_late(false);
            (self.engine.changed)();
            // The session list is read before the manager is watched,
            // otherwise Windows does not report new sessions.
            let list = audio::session_manager(&d).and_then(|mgr| Ok((audio::sessions(&mgr, &mut self.names)?, mgr)));
            match list {
                Ok((list, mgr)) => {
                    self.rows = mixer::merge(list);
                    self.manager = Some(mgr);
                }
                Err(err) => m.apps_error = Some(format!("Could not read apps: {}", err.message())),
            }
            self.device = Some(d);
        }
        // Other apps now: forget the names of processes that are gone.
        if mixer::rows_changed(&old_rows, &self.rows) {
            self.names.retain_rows(&self.rows);
        }
        drop(old_rows);
        if self.engine.live.load(Ordering::Relaxed) {
            self.watch();
        }
        m.apps = self.app_levels();
        self.store(m, note);
        self.flush_late(true);
    }

    /// The list of apps again, without the devices. True if the window has to be redrawn.
    fn rescan_apps(&mut self) -> bool {
        let Some(mgr) = &self.manager else { return false };
        let Ok(mut list) = audio::sessions(mgr, &mut self.names) else { return false };
        // New sessions the list does not have yet; forget them once it has them or they ended.
        let names = &mut self.names;
        self.new_sessions.retain(|c| {
            let Some(s) = audio::session(c, -1, names) else { return false };
            let keep = !s.expired && !list.iter().any(|x| x.id == s.id);
            if keep {
                list.push(s);
            }
            keep
        });
        self.unwatch_apps();
        let old_rows = std::mem::replace(&mut self.rows, mixer::merge(list));
        if mixer::rows_changed(&old_rows, &self.rows) {
            self.names.retain_rows(&self.rows);
        }
        drop(old_rows);
        self.watch_apps();
        let apps = self.app_levels();
        let changed = self.publish(|m| m.apps = apps);
        self.flush_late(false);
        changed
    }

    /// Volumes and mutes only: no devices, no names. True if one of them changed.
    fn read_values(&mut self) -> bool {
        let master = self.master.as_ref().and_then(|m| audio::endpoint_state(m).ok());
        for r in &mut self.rows {
            let mut states = r.handles.iter().filter_map(|h| audio::session_state(&h.volume));
            if let Some((v, first_muted)) = states.next() {
                r.volume = v;
                r.muted = first_muted && states.all(|s| s.1);
            }
        }
        let apps = self.app_levels();
        self.publish(|m| {
            if master.is_some() {
                m.master = master;
            }
            for a in &mut m.apps {
                if let Some(n) = apps.iter().find(|n| n.key == a.key) {
                    a.volume = n.volume;
                    a.muted = n.muted;
                }
            }
            if m.apps.len() != apps.len() {
                m.apps = apps;
            }
        })
    }

    /// Change the model's volumes and apps, keeping what the user owns right
    /// now. True if anything the window shows changed.
    fn publish(&self, f: impl FnOnce(&mut Model)) -> bool {
        let inbox = lock(&self.engine.inbox);
        let mut model = lock(&self.engine.model);
        let mut m = model.clone();
        f(&mut m);
        inbox.keep_user_values(&self.late, &model, &mut m);
        let changed = m.master != model.master || m.apps != model.apps;
        *model = m;
        changed
    }

    fn switch_to(&mut self, id: &str) -> Option<String> {
        audio::set_default(id).err()
    }
}

fn run(engine: Arc<Engine>) {
    // Multithreaded: Windows only reports new audio sessions to such threads.
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
    let enumerator = audio::enumerator().ok();
    let notifier: Option<IMMNotificationClient> =
        enumerator.as_ref().map(|_| Notifier { engine: engine.clone() }.into());
    if let (Some(e), Some(n)) = (&enumerator, &notifier) {
        unsafe {
            let _ = e.RegisterEndpointNotificationCallback(n);
        }
    }
    let callbacks = Callbacks {
        master: MasterEvents { engine: engine.clone() }.into(),
        sessions: NewSessions { engine: engine.clone() }.into(),
        app: AppEvents { engine: engine.clone() }.into(),
    };
    let mut a = Audio {
        engine: engine.clone(),
        enumerator,
        device: None,
        master: None,
        manager: None,
        rows: Vec::new(),
        new_sessions: Vec::new(),
        names: audio::NameCache::default(),
        callbacks,
        master_watched: false,
        sessions_watched: false,
        apps_watched: Vec::new(),
        deferred: false,
        late: Late::default(),
    };
    loop {
        let (cmds, volumes, held) = {
            let mut inbox = lock(&engine.inbox);
            loop {
                let now = Instant::now();
                let due = inbox.volumes.due(now);
                let deferred_ready = a.deferred && inbox.held.is_none();
                if !inbox.cmds.is_empty() || due.is_some_and(|t| t <= now) || deferred_ready {
                    break;
                }
                inbox = match due {
                    Some(t) => engine.wake.wait_timeout(inbox, t - now).unwrap_or_else(|e| e.into_inner()).0,
                    None => engine.wake.wait(inbox).unwrap_or_else(|e| e.into_inner()),
                };
            }
            (std::mem::take(&mut inbox.cmds), inbox.volumes.take(Instant::now()), inbox.held.is_some())
        };
        // No lock is held while Windows audio is called.
        for (t, v) in volumes {
            // Not read yet (the window shows cache.json): written once it is.
            if !a.write_volume(&t, v) {
                a.late.volume(t, v);
            }
        }
        let mut refresh = std::mem::take(&mut a.deferred);
        let mut values = false;
        let mut apps = false;
        // Some(..): the result of a switch, shown until the next one.
        let mut note = None;
        for cmd in cmds {
            match cmd {
                Cmd::Refresh => refresh = true,
                Cmd::Values => values = true,
                Cmd::Sessions(new) => {
                    a.new_sessions.extend(new.map(|n| n.0));
                    apps = true;
                }
                Cmd::Mute(t, on) => {
                    if !a.write_mute(&t, on) {
                        a.late.mute(t, on);
                    }
                }
                Cmd::Unwatch => {
                    // Asked again since: the newest wish wins.
                    if !engine.live.load(Ordering::Relaxed) {
                        a.unwatch();
                    }
                }
                Cmd::SetDefault(id) => {
                    note = Some(a.switch_to(&id));
                    refresh = true;
                }
                Cmd::Quit(done) => {
                    a.unwatch();
                    if let (Some(e), Some(n)) = (&a.enumerator, &notifier) {
                        unsafe {
                            let _ = e.UnregisterEndpointNotificationCallback(n);
                        }
                    }
                    drop(a);
                    drop(notifier);
                    unsafe { CoUninitialize() };
                    let _ = done.send(());
                    return;
                }
            }
        }
        // Never a full refresh while a slider is dragged: it runs when it is let go.
        if refresh && held {
            if let Some(n) = note.take() {
                lock(&engine.model).note = n;
                (engine.changed)();
            }
            a.deferred = true;
            refresh = false;
        }
        // Volume writes need no repaint: the window already shows those values.
        if refresh {
            a.refresh(note);
            (engine.changed)();
        } else {
            let mut redraw = apps && a.rescan_apps();
            redraw |= values && a.read_values();
            if redraw {
                (engine.redraw)();
            }
        }
    }
}
