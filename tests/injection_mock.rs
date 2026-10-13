#![cfg(unix)]

use std::{
    env,
    ffi::OsString,
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    sync::Mutex,
    time::Duration,
};

use tempfile::TempDir;
use yuda::injection::{inject_text, PasteBackend};

static ENV_LOCK: Mutex<()> = Mutex::new(());

struct EnvGuard {
    saved: Vec<(&'static str, Option<OsString>)>,
}

impl EnvGuard {
    fn new() -> Self {
        Self { saved: Vec::new() }
    }

    fn set(&mut self, key: &'static str, value: impl Into<OsString>) {
        self.saved.push((key, env::var_os(key)));
        env::set_var(key, value.into());
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (key, value) in self.saved.drain(..).rev() {
            if let Some(value) = value {
                env::set_var(key, value);
            } else {
                env::remove_var(key);
            }
        }
    }
}

struct MockCommands {
    _temp: TempDir,
    state: PathBuf,
}

impl MockCommands {
    fn new(wtype_status: i32, ydotool_status: i32, fail_first_copy: bool) -> Self {
        let temp = TempDir::new().expect("mock command directory should be created");
        let bin = temp.path().join("bin");
        let state = temp.path().join("state");
        fs::create_dir_all(&bin).expect("mock bin should be created");
        fs::create_dir_all(&state).expect("mock state should be created");

        write_executable(
            &bin.join("wl-paste"),
            r#"#!/bin/sh
printf '%s\n' "paste:$*" >> "$YUDA_MOCK_LOG"
if [ "$1" = "--list-types" ]; then
    printf '%s\n' 'text/plain;charset=utf-8'
else
    cat "$YUDA_MOCK_SNAPSHOT"
fi
"#,
        );
        let copy_script = r#"#!/bin/sh
printf '%s\n' "copy:$*" >> "$YUDA_MOCK_LOG"
if [ -f "$YUDA_MOCK_FAIL_FIRST" ] && [ ! -f "$YUDA_MOCK_FAILED" ]; then
    touch "$YUDA_MOCK_FAILED"
    cat >/dev/null
    exit 1
fi
if [ -f "$YUDA_MOCK_FIRST_COPY" ]; then
    cat > "$YUDA_MOCK_SECOND_COPY"
else
    cat > "$YUDA_MOCK_FIRST_COPY"
fi
exit 0
"#
        .to_owned();
        write_executable(&bin.join("wl-copy"), &copy_script);
        write_executable(
            &bin.join("wtype"),
            &format!(
                "#!/bin/sh\nprintf '%s\\n' \"wtype:$*\" >> \"$YUDA_MOCK_LOG\"\nexit {wtype_status}\n"
            ),
        );
        write_executable(
            &bin.join("ydotool"),
            &format!(
                "#!/bin/sh\nprintf '%s\\n' \"ydotool:$*\" >> \"$YUDA_MOCK_LOG\"\nexit {ydotool_status}\n"
            ),
        );
        fs::write(state.join("snapshot"), "原始剪贴板 🗂️\n").expect("snapshot should be written");
        if fail_first_copy {
            fs::write(state.join("fail-first"), b"1").expect("failure marker should be written");
        }
        fs::write(state.join("log"), "").expect("log should be created");

        Self { _temp: temp, state }
    }

    fn configure(&self, env_guard: &mut EnvGuard) {
        let bin = self._temp.path().join("bin");
        let original_path = env::var_os("PATH").unwrap_or_default();
        let mut path = bin.into_os_string();
        path.push(":");
        path.push(original_path);
        env_guard.set("PATH", path);
        env_guard.set("YUDA_MOCK_LOG", self.state.join("log").into_os_string());
        env_guard.set(
            "YUDA_MOCK_SNAPSHOT",
            self.state.join("snapshot").into_os_string(),
        );
        env_guard.set(
            "YUDA_MOCK_FIRST_COPY",
            self.state.join("first-copy").into_os_string(),
        );
        env_guard.set(
            "YUDA_MOCK_SECOND_COPY",
            self.state.join("second-copy").into_os_string(),
        );
        env_guard.set(
            "YUDA_MOCK_FAIL_FIRST",
            self.state.join("fail-first").into_os_string(),
        );
        env_guard.set(
            "YUDA_MOCK_FAILED",
            self.state.join("failed").into_os_string(),
        );
    }

    fn lines(&self) -> Vec<String> {
        fs::read_to_string(self.state.join("log"))
            .expect("mock log should be readable")
            .lines()
            .map(str::to_owned)
            .collect()
    }

    fn copied(&self, name: &str) -> Vec<u8> {
        fs::read(self.state.join(name)).expect("expected clipboard write should exist")
    }
}

fn write_executable(path: &Path, content: &str) {
    fs::write(path, content).expect("mock command should be written");
    let mut permissions = fs::metadata(path)
        .expect("mock command metadata should be readable")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).expect("mock command should be executable");
}

#[test]
fn restores_clipboard_after_successful_unicode_injection() {
    let _lock = ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mock = MockCommands::new(0, 0, false);
    let mut env_guard = EnvGuard::new();
    mock.configure(&mut env_guard);

    let text = "今天写 Rust 🦀";
    let backend =
        inject_text(text, Duration::from_millis(1)).expect("mocked injection should succeed");

    assert_eq!(backend, PasteBackend::Wtype);
    assert_eq!(mock.copied("first-copy"), text.as_bytes());
    assert_eq!(mock.copied("second-copy"), "原始剪贴板 🗂️\n".as_bytes());
    assert_eq!(
        mock.lines(),
        [
            "paste:--list-types",
            "paste:--no-newline --type text/plain;charset=utf-8",
            "copy:--type text/plain;charset=utf-8",
            "wtype:-M ctrl -k v -m ctrl",
            "copy:--type text/plain;charset=utf-8",
        ]
    );
}

#[test]
fn falls_back_to_ydotool_when_wtype_returns_failure() {
    let _lock = ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mock = MockCommands::new(1, 0, false);
    let mut env_guard = EnvGuard::new();
    mock.configure(&mut env_guard);

    let backend = inject_text("回退路径", Duration::ZERO).expect("ydotool fallback should succeed");

    assert_eq!(backend, PasteBackend::Ydotool);
    assert!(mock
        .lines()
        .iter()
        .any(|line| line == "wtype:-M ctrl -k v -m ctrl"));
    assert!(mock
        .lines()
        .iter()
        .any(|line| line == "ydotool:key 29:1 47:1 47:0 29:0"));
    assert_eq!(mock.copied("second-copy"), "原始剪贴板 🗂️\n".as_bytes());
}

#[test]
fn restores_clipboard_even_when_first_copy_fails() {
    let _lock = ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mock = MockCommands::new(0, 0, true);
    let mut env_guard = EnvGuard::new();
    mock.configure(&mut env_guard);

    let error = inject_text("不会上屏", Duration::ZERO).expect_err("copy failure should surface");

    assert!(format!("{error:#}").contains("wl-copy 返回失败状态"));
    assert_eq!(mock.copied("first-copy"), "原始剪贴板 🗂️\n".as_bytes());
    assert_eq!(
        mock.lines(),
        [
            "paste:--list-types",
            "paste:--no-newline --type text/plain;charset=utf-8",
            "copy:--type text/plain;charset=utf-8",
            "copy:--type text/plain;charset=utf-8",
        ]
    );
}
