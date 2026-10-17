//! The audio thread. It owns every Core Audio object, so the window never waits
//! for Windows audio: the window reads a copy of `Model` and leaves requests
//! here. Volume requests are coalesced (latest wins, about 60 writes a second,
//! see `coalesce`); everything else runs in the order it was asked for.

use crate::audio;
use crate::coalesce::{self, Coalescer};
use crate::cycle::{self, Device};
use crate::mixer::{self, Row};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::Instant;
use windows::core::{implement, Result as WinResult, PCWSTR};
use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
use windows::Win32::Media::Audio::{
    eConsole, eRender, EDataFlow, ERole, IMMDevice, IMMDeviceEnumerator, IMMNotificationClient,
    IMMNotificationClient_Impl, ISimpleAudioVolume, DEVICE_STATE,
};
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED};
use windows::Win32::System::Diagnostics::Debug::MessageBeep;
use windows::Win32::UI::Shell::PropertiesSystem::PROPERTYKEY;
use windows::Win32::UI::WindowsAndMessaging::MB_OK;

/// What a slider controls.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Target {
    /// The default device's master volume.
    Master,
    /// One app row, by its key (see `mixer::Row::key`).
    App(String),
}

#[derive(Clone, Debug, Default)]
pub struct AppLevel {
    pub key: String,
    pub name: String,
    /// 0.0-1.0
    pub volume: f32,
    pub muted: bool,
}

/// What the window shows. Written by the audio thread, and by the window for
/// its own volume and mute changes, so the window shows them before they are applied.
#[derive(Clone, Debug, Default)]
pub struct Model {
    pub devices: Vec<Device>,
    pub default_id: Option<String>,
    pub default_name: String,
    /// Master volume (0.0-1.0) and mute of the default device.
    pub master: Option<(f32, bool)>,
    pub apps: Vec<AppLevel>,
    /// No device, or Windows audio not available.
    pub error: Option<String>,
    pub apps_error: Option<String>,
    /// The last switching problem, until the next switch works.
    pub note: Option<String>,
}

impl Model {
    fn set_level(&mut self, t: &Target, v: f32) {
        match t {
            Target::Master => {
                if let Some(m) = &mut self.master {
                    m.0 = v;
                }
            }
            Target::App(k) => {
                if let Some(a) = self.apps.iter_mut().find(|a| &a.key == k) {
                    a.volume = v;
                }
            }
        }
    }

    fn set_muted(&mut self, t: &Target, on: bool) {
        match t {
            Target::Master => {
                if let Some(m) = &mut self.master {
                    m.1 = on;
                }
            }
            Target::App(k) => {
                if let Some(a) = self.apps.iter_mut().find(|a| &a.key == k) {
                    a.muted = on;
                }
            }
        }
    }
}

enum Cmd {
    Refresh,
    SetDefault(String),
    Mute(Target, bool),
    /// The hotkey: go to the next device not in this exclude list, and beep.
    Cycle(Vec<String>),
    Quit(std::sync::mpsc::Sender<()>),
}

struct Inbox {
    volumes: Coalescer<Target>,
    cmds: Vec<Cmd>,
}

pub struct Engine {
    model: Mutex<Model>,
    inbox: Mutex<Inbox>,
    wake: Condvar,
    /// Called on the audio thread after the model changed.
    changed: fn(),
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Engine {
    /// Start the audio thread. It reads everything once right away.
    pub fn start(changed: fn()) -> Arc<Engine> {
        let engine = Arc::new(Engine {
            model: Mutex::new(Model::default()),
            inbox: Mutex::new(Inbox { volumes: Coalescer::new(coalesce::GAP), cmds: vec![Cmd::Refresh] }),
            wake: Condvar::new(),
            changed,
        });
        let e = engine.clone();
        std::thread::spawn(move || run(e));
        engine
    }

    /// A copy of what the window shows.
    pub fn model(&self) -> Model {
        lock(&self.model).clone()
    }

    fn send(&self, cmd: Cmd) {
        let mut inbox = lock(&self.inbox);
        // One waiting refresh is enough.
        if !(matches!(cmd, Cmd::Refresh) && inbox.cmds.iter().any(|c| matches!(c, Cmd::Refresh))) {
            inbox.cmds.push(cmd);
        }
        self.wake.notify_one();
    }

    pub fn refresh(&self) {
        self.send(Cmd::Refresh);
    }

    pub fn set_default(&self, id: String) {
        lock(&self.model).default_id = Some(id.clone());
        self.send(Cmd::SetDefault(id));
    }

    pub fn cycle(&self, excluded: Vec<String>) {
        self.send(Cmd::Cycle(excluded));
    }

    /// Ask for a volume (0.0-1.0). Only the newest value per target is written.
    /// `last` (slider let go, click, wheel) writes it without waiting.
    pub fn set_volume(&self, t: Target, v: f32, last: bool) {
        let v = v.clamp(0.0, 1.0);
        lock(&self.model).set_level(&t, v);
        let mut inbox = lock(&self.inbox);
        if last {
            inbox.volumes.set_final(t, v);
        } else {
            inbox.volumes.set(t, v);
        }
        self.wake.notify_one();
    }

    pub fn set_mute(&self, t: Target, on: bool) {
        lock(&self.model).set_muted(&t, on);
        self.send(Cmd::Mute(t, on));
    }

    /// Stop the audio thread (it lets go of its audio objects), waiting at most 2 s.
    pub fn quit(&self) {
        let (tx, rx) = std::sync::mpsc::channel();
        self.send(Cmd::Quit(tx));
        let _ = rx.recv_timeout(std::time::Duration::from_secs(2));
    }
}

/// Asks for a refresh when the default device changes or a device comes or goes.
#[implement(IMMNotificationClient)]
struct Notifier {
    engine: Arc<Engine>,
}

impl IMMNotificationClient_Impl for Notifier_Impl {
    fn OnDeviceStateChanged(&self, _: &PCWSTR, _: DEVICE_STATE) -> WinResult<()> {
        self.engine.refresh();
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
            self.engine.refresh();
        }
        Ok(())
    }
    fn OnPropertyValueChanged(&self, _: &PCWSTR, _: &PROPERTYKEY) -> WinResult<()> {
        Ok(())
    }
}

/// The audio objects, only ever used on the audio thread.
struct Audio {
    engine: Arc<Engine>,
    enumerator: Option<IMMDeviceEnumerator>,
    device: Option<IMMDevice>,
    master: Option<IAudioEndpointVolume>,
    rows: Vec<Row<ISimpleAudioVolume>>,
    names: audio::NameCache,
}

impl Audio {
    fn write_volume(&mut self, t: &Target, v: f32) {
        match t {
            Target::Master => {
                if let Some(m) = &self.master {
                    audio::set_endpoint_volume(m, v);
                }
            }
            Target::App(k) => {
                if let Some(r) = self.rows.iter_mut().find(|r| &r.key == k) {
                    for h in &r.handles {
                        audio::set_session_volume(h, v);
                    }
                    r.volume = v;
                }
            }
        }
    }

    fn write_mute(&mut self, t: &Target, on: bool) {
        match t {
            Target::Master => {
                if let Some(m) = &self.master {
                    audio::set_endpoint_mute(m, on);
                }
            }
            Target::App(k) => {
                if let Some(r) = self.rows.iter_mut().find(|r| &r.key == k) {
                    for h in &r.handles {
                        audio::set_session_mute(h, on);
                    }
                    r.muted = on;
                }
            }
        }
    }

    /// Read everything again: devices, the default one, its volume and its apps.
    fn refresh(&mut self, note: Option<Option<String>>) {
        let mut m = Model::default();
        self.device = None;
        self.master = None;
        let old_rows = std::mem::take(&mut self.rows);
        match &self.enumerator {
            None => m.error = Some("Windows audio is not available".into()),
            Some(e) => {
                match audio::devices(e) {
                    Ok(list) => m.devices = list,
                    Err(err) => m.error = Some(format!("Could not list devices: {}", err.message())),
                }
                match audio::default_endpoint(e) {
                    Ok((device, control)) => {
                        let def = audio::default_device(e);
                        m.default_id = def.as_ref().map(|d| d.id.clone());
                        m.default_name = audio::device_display_name(&device);
                        match audio::endpoint_state(&control) {
                            Ok(state) => m.master = Some(state),
                            Err(err) => m.error = Some(format!("Could not read the volume: {}", err.message())),
                        }
                        match audio::sessions(&device, &mut self.names) {
                            Ok(list) => self.rows = mixer::merge(list),
                            Err(err) => m.apps_error = Some(format!("Could not read apps: {}", err.message())),
                        }
                        self.device = Some(device);
                        self.master = Some(control);
                    }
                    Err(_) => {
                        m.default_name = "No playback device".into();
                        m.error.get_or_insert_with(|| "No audio output device found".into());
                    }
                }
            }
        }
        // Other apps now: forget the names of processes that are gone.
        if mixer::rows_changed(&old_rows, &self.rows) {
            self.names.retain_rows(&self.rows);
        }
        drop(old_rows);
        m.apps = self
            .rows
            .iter()
            .map(|r| AppLevel { key: r.key.clone(), name: r.name.clone(), volume: r.volume, muted: r.muted })
            .collect();
        // Volumes asked for but not written yet win over what Windows says now.
        {
            let inbox = lock(&self.engine.inbox);
            for (t, v) in inbox.volumes.pending() {
                m.set_level(t, *v);
            }
        }
        let mut model = lock(&self.engine.model);
        m.note = note.unwrap_or_else(|| model.note.take());
        *model = m;
    }

    fn switch_to(&mut self, id: &str) -> Option<String> {
        audio::set_default(id).err()
    }

    fn cycle(&mut self, excluded: &[String]) -> Option<Option<String>> {
        let e = self.enumerator.as_ref()?;
        let devices = audio::devices(e).ok()?;
        let current = audio::default_device(e).map(|d| d.id);
        let i = cycle::next_index(&devices, current.as_deref(), excluded)?;
        let err = self.switch_to(&devices[i].id);
        if err.is_none() {
            // The default sound now plays on the new device.
            unsafe {
                let _ = MessageBeep(MB_OK);
            }
        }
        Some(err)
    }
}

fn run(engine: Arc<Engine>) {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
    }
    let enumerator = audio::enumerator().ok();
    let notifier: Option<IMMNotificationClient> =
        enumerator.as_ref().map(|_| Notifier { engine: engine.clone() }.into());
    if let (Some(e), Some(n)) = (&enumerator, &notifier) {
        unsafe {
            let _ = e.RegisterEndpointNotificationCallback(n);
        }
    }
    let mut a = Audio {
        engine: engine.clone(),
        enumerator,
        device: None,
        master: None,
        rows: Vec::new(),
        names: audio::NameCache::default(),
    };
    loop {
        let (cmds, volumes) = {
            let mut inbox = lock(&engine.inbox);
            loop {
                let now = Instant::now();
                let due = inbox.volumes.due(now);
                if !inbox.cmds.is_empty() || due.is_some_and(|t| t <= now) {
                    break;
                }
                inbox = match due {
                    Some(t) => engine.wake.wait_timeout(inbox, t - now).unwrap_or_else(|e| e.into_inner()).0,
                    None => engine.wake.wait(inbox).unwrap_or_else(|e| e.into_inner()),
                };
            }
            (std::mem::take(&mut inbox.cmds), inbox.volumes.take(Instant::now()))
        };
        // No lock is held while Windows audio is called.
        for (t, v) in &volumes {
            a.write_volume(t, *v);
        }
        let mut refresh = false;
        // Some(..): the result of a switch, shown until the next one.
        let mut note = None;
        for cmd in cmds {
            match cmd {
                Cmd::Refresh => refresh = true,
                Cmd::Mute(t, on) => a.write_mute(&t, on),
                Cmd::SetDefault(id) => {
                    note = Some(a.switch_to(&id));
                    refresh = true;
                }
                Cmd::Cycle(excluded) => {
                    if let Some(err) = a.cycle(&excluded) {
                        note = Some(err);
                        refresh = true;
                    }
                }
                Cmd::Quit(done) => {
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
        // Volume writes need no repaint: the window already shows those values.
        if refresh {
            a.refresh(note);
            (engine.changed)();
        }
    }
}
