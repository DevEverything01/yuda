use anyhow::{bail, Context, Result};
use evdev::{Device, InputEventKind, Key};
use std::{
    collections::HashSet,
    fs,
    path::PathBuf,
    sync::mpsc::{self, Receiver, Sender},
    thread,
    time::{Duration, Instant},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyAction {
    Pressed,
    Released { held_for: Duration },
    Cancelled,
}

pub struct HotkeyListener {
    receiver: Receiver<HotkeyAction>,
}

impl HotkeyListener {
    pub fn start(name: &str) -> Result<Self> {
        let key = parse_key(name)?;
        ensure_input_access()?;
        let (raw_tx, raw_rx) = mpsc::channel();
        let (action_tx, action_rx) = mpsc::channel();
        thread::Builder::new()
            .name("yuda-hotkey-enumerator".to_owned())
            .spawn(move || enumerate_devices(key, raw_rx, action_tx, raw_tx.clone()))
            .context("启动 evdev 热键枚举线程失败")?;
        Ok(Self {
            receiver: action_rx,
        })
    }

    pub fn recv(&self) -> Result<HotkeyAction> {
        self.receiver.recv().context("evdev 热键监听线程已退出")
    }
}

fn parse_key(name: &str) -> Result<Key> {
    match name {
        "KEY_RIGHTCTRL" => Ok(Key::KEY_RIGHTCTRL),
        "KEY_LEFTCTRL" => Ok(Key::KEY_LEFTCTRL),
        _ => bail!("暂不支持热键 {name}；当前支持 KEY_RIGHTCTRL / KEY_LEFTCTRL"),
    }
}

fn ensure_input_access() -> Result<()> {
    let input = fs::read_dir("/dev/input").with_context(|| {
        "无法访问 /dev/input；请执行 `sudo usermod -aG input $USER`，重新登录后再试"
    })?;
    let mut found_event = false;
    for entry in input {
        let entry = entry.context("读取 /dev/input 目录失败")?;
        if entry.file_name().to_string_lossy().starts_with("event") {
            found_event = true;
            break;
        }
    }
    if !found_event {
        bail!("/dev/input 中没有 event* 设备；请确认 evdev 输入设备已加载");
    }
    Ok(())
}

#[derive(Debug)]
enum RawEvent {
    Key { path: PathBuf, key: Key, value: i32 },
    DeviceEnded(PathBuf),
}

fn enumerate_devices(
    hotkey: Key,
    raw_rx: Receiver<RawEvent>,
    action_tx: Sender<HotkeyAction>,
    raw_tx: Sender<RawEvent>,
) {
    let mut known = HashSet::new();
    let mut gate = HotkeyGate::new(hotkey);
    loop {
        for (path, device) in evdev::enumerate() {
            if known.contains(&path) {
                continue;
            }
            let Some(keys) = device.supported_keys() else {
                continue;
            };
            if !keys.contains(hotkey) {
                continue;
            }
            let path_for_thread = path.clone();
            let tx = raw_tx.clone();
            known.insert(path);
            let spawn_result = thread::Builder::new()
                .name("yuda-hotkey-device".to_owned())
                .spawn(move || read_device(path_for_thread, device, tx));
            if let Err(error) = spawn_result {
                tracing::warn!(%error, "启动 evdev 设备线程失败");
            }
        }

        while let Ok(event) = raw_rx.try_recv() {
            match event {
                RawEvent::Key { path, key, value } => gate.process(path, key, value, &action_tx),
                RawEvent::DeviceEnded(path) => {
                    known.remove(&path);
                    gate.device_ended(&path, &action_tx);
                }
            }
        }
        thread::sleep(Duration::from_millis(200));
    }
}

fn read_device(path: PathBuf, mut device: Device, tx: Sender<RawEvent>) {
    // Device::open/fetch_events only reads evdev and never calls EVIOCGRAB.
    loop {
        match device.fetch_events() {
            Ok(events) => {
                for event in events {
                    if let InputEventKind::Key(key) = event.kind() {
                        if tx
                            .send(RawEvent::Key {
                                path: path.clone(),
                                key,
                                value: event.value(),
                            })
                            .is_err()
                        {
                            return;
                        }
                    }
                }
            }
            Err(error) => {
                tracing::debug!(device = %path.display(), %error, "evdev 设备读取结束");
                let _ = tx.send(RawEvent::DeviceEnded(path));
                return;
            }
        }
    }
}

struct HotkeyGate {
    hotkey: Key,
    pressed_path: Option<PathBuf>,
    candidate_started: Option<Instant>,
    other_keys: HashSet<(PathBuf, Key)>,
}

impl HotkeyGate {
    fn new(hotkey: Key) -> Self {
        Self {
            hotkey,
            pressed_path: None,
            candidate_started: None,
            other_keys: HashSet::new(),
        }
    }

    fn process(&mut self, path: PathBuf, key: Key, value: i32, output: &Sender<HotkeyAction>) {
        match (key == self.hotkey, value) {
            (true, 1) if self.pressed_path.is_none() => {
                self.pressed_path = Some(path);
                if self.other_keys.is_empty() {
                    self.candidate_started = Some(Instant::now());
                    let _ = output.send(HotkeyAction::Pressed);
                }
            }
            (true, 0) if self.pressed_path.as_ref() == Some(&path) => {
                if let Some(started) = self.candidate_started.take() {
                    let _ = output.send(HotkeyAction::Released {
                        held_for: started.elapsed(),
                    });
                } else {
                    let _ = output.send(HotkeyAction::Cancelled);
                }
                self.pressed_path = None;
            }
            (false, 1) => {
                self.other_keys.insert((path, key));
                if self.pressed_path.is_some() && self.candidate_started.take().is_some() {
                    let _ = output.send(HotkeyAction::Cancelled);
                }
            }
            (false, 0) => {
                self.other_keys.remove(&(path, key));
            }
            _ => {}
        }
    }

    fn device_ended(&mut self, path: &PathBuf, output: &Sender<HotkeyAction>) {
        self.other_keys.retain(|(device, _)| device != path);
        if self.pressed_path.as_ref() == Some(path) {
            if self.candidate_started.take().is_some() {
                let _ = output.send(HotkeyAction::Cancelled);
            }
            self.pressed_path = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{HotkeyAction, HotkeyGate};
    use evdev::Key;
    use std::{path::PathBuf, sync::mpsc, thread, time::Duration};

    #[test]
    fn accepts_alone_hotkey_and_reports_duration() {
        let (tx, rx) = mpsc::channel();
        let mut gate = HotkeyGate::new(Key::KEY_RIGHTCTRL);
        let path = PathBuf::from("/dev/input/event3");
        gate.process(path.clone(), Key::KEY_RIGHTCTRL, 1, &tx);
        thread::sleep(Duration::from_millis(1));
        gate.process(path, Key::KEY_RIGHTCTRL, 0, &tx);
        assert!(matches!(rx.recv().unwrap(), HotkeyAction::Pressed));
        assert!(matches!(rx.recv().unwrap(), HotkeyAction::Released { .. }));
    }

    #[test]
    fn cancels_when_another_key_is_pressed() {
        let (tx, rx) = mpsc::channel();
        let mut gate = HotkeyGate::new(Key::KEY_RIGHTCTRL);
        let path = PathBuf::from("/dev/input/event3");
        gate.process(path.clone(), Key::KEY_RIGHTCTRL, 1, &tx);
        gate.process(path.clone(), Key::KEY_A, 1, &tx);
        gate.process(path, Key::KEY_RIGHTCTRL, 0, &tx);
        assert!(matches!(rx.recv().unwrap(), HotkeyAction::Pressed));
        assert!(matches!(rx.recv().unwrap(), HotkeyAction::Cancelled));
        assert!(matches!(rx.recv().unwrap(), HotkeyAction::Cancelled));
    }
}
