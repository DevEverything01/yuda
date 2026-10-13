#![cfg(unix)]
//! 上屏契约：用 PATH 注入的假 wl-paste / wl-copy / hyprctl / wtype / ydotool 验证命令顺序。

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
use yuda::injection::{inject_text, InjectOptions, PasteBackend};

static ENV_LOCK: Mutex<()> = Mutex::new(());

const ORIGINAL: &str = "原始剪贴板 🗂️\n";

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

    fn unset(&mut self, key: &'static str) {
        self.saved.push((key, env::var_os(key)));
        env::remove_var(key);
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (key, value) in self.saved.drain(..).rev() {
            match value {
                Some(value) => env::set_var(key, value),
                None => env::remove_var(key),
            }
        }
    }
}

#[derive(Clone, Copy, Default)]
struct Scenario {
    hyprland: bool,
    hyprctl_status: i32,
    wtype_status: i32,
    ydotool_status: i32,
    fail_first_copy: bool,
    empty_clipboard: bool,
    sensitive_clipboard: bool,
}

struct MockCommands {
    _temp: TempDir,
    state: PathBuf,
    _env: EnvGuard,
}

impl MockCommands {
    fn new(scenario: Scenario) -> Self {
        let temp = TempDir::new().expect("mock command directory should be created");
        let bin = temp.path().join("bin");
        let state = temp.path().join("state");
        fs::create_dir_all(&bin).expect("mock bin should be created");
        fs::create_dir_all(&state).expect("mock state should be created");

        write_executable(
            &bin.join("wl-paste"),
            r#"#!/bin/sh
printf '%s\n' "paste:$*" >> "$YUDA_MOCK_STATE/log"
[ -f "$YUDA_MOCK_STATE/empty" ] && exit 1
if [ "$1" = "--list-types" ]; then
    printf '%s\n' 'TEXT' 'text/plain;charset=utf-8' 'UTF8_STRING'
    if [ -f "$YUDA_MOCK_STATE/sensitive" ]; then
        printf '%s\n' 'x-kde-passwordManagerHint'
    fi
else
    cat "$YUDA_MOCK_STATE/snapshot"
fi
"#,
        );
        write_executable(
            &bin.join("wl-copy"),
            r#"#!/bin/sh
printf '%s\n' "copy:$*" >> "$YUDA_MOCK_STATE/log"
if [ -f "$YUDA_MOCK_STATE/fail-first" ] && [ ! -f "$YUDA_MOCK_STATE/failed" ]; then
    touch "$YUDA_MOCK_STATE/failed"
    cat >/dev/null
    exit 1
fi
n=$(ls "$YUDA_MOCK_STATE" | grep -c '^copy-')
cat > "$YUDA_MOCK_STATE/copy-$n"
"#,
        );
        for (name, status) in [
            ("hyprctl", scenario.hyprctl_status),
            ("wtype", scenario.wtype_status),
            ("ydotool", scenario.ydotool_status),
        ] {
            // hyprctl 只记录子命令名，Lua 正文由单元测试覆盖。
            let logged = if name == "hyprctl" { "$1" } else { "$*" };
            write_executable(
                &bin.join(name),
                &format!(
                    "#!/bin/sh\nprintf '%s\\n' \"{name}:{logged}\" >> \"$YUDA_MOCK_STATE/log\"\nexit {status}\n"
                ),
            );
        }
        fs::write(state.join("snapshot"), ORIGINAL).expect("snapshot should be written");
        if scenario.fail_first_copy {
            fs::write(state.join("fail-first"), b"1").expect("failure marker");
        }
        if scenario.empty_clipboard {
            fs::write(state.join("empty"), b"1").expect("empty marker");
        }
        if scenario.sensitive_clipboard {
            fs::write(state.join("sensitive"), b"1").expect("sensitive marker");
        }
        fs::write(state.join("log"), "").expect("log should be created");

        let mut env_guard = EnvGuard::new();
        let mut path = bin.into_os_string();
        path.push(":");
        path.push(env::var_os("PATH").unwrap_or_default());
        env_guard.set("PATH", path);
        env_guard.set("YUDA_MOCK_STATE", state.clone().into_os_string());
        if scenario.hyprland {
            env_guard.set("HYPRLAND_INSTANCE_SIGNATURE", "mock");
        } else {
            env_guard.unset("HYPRLAND_INSTANCE_SIGNATURE");
        }
        Self {
            _temp: temp,
            state,
            _env: env_guard,
        }
    }

    fn lines(&self) -> Vec<String> {
        fs::read_to_string(self.state.join("log"))
            .expect("mock log should be readable")
            .lines()
            .map(str::to_owned)
            .collect()
    }

    /// 第 n 次成功的 wl-copy 写入的内容。
    fn copy(&self, n: usize) -> Option<Vec<u8>> {
        fs::read(self.state.join(format!("copy-{n}"))).ok()
    }
}

fn write_executable(path: &Path, content: &str) {
    fs::write(path, content).expect("mock command should be written");
    let mut permissions = fs::metadata(path).expect("metadata").permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).expect("mock command should be executable");
}

fn lock() -> std::sync::MutexGuard<'static, ()> {
    ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn quick() -> InjectOptions {
    InjectOptions {
        paste_delay: Duration::from_millis(1),
        auto_paste: true,
    }
}

#[test]
fn hyprland_session_pastes_through_hyprctl_and_restores_clipboard() {
    let _lock = lock();
    let mock = MockCommands::new(Scenario {
        hyprland: true,
        ..Scenario::default()
    });

    let text = "今天写 Rust 🦀";
    let backend = inject_text(text, &quick()).expect("mocked injection should succeed");

    assert_eq!(backend, PasteBackend::Hyprland);
    assert_eq!(mock.copy(0).as_deref(), Some(text.as_bytes()));
    assert_eq!(mock.copy(1).as_deref(), Some(ORIGINAL.as_bytes()));
    assert_eq!(
        mock.lines(),
        [
            "paste:--list-types",
            "paste:--no-newline --type text/plain;charset=utf-8",
            "copy:--type text/plain;charset=utf-8 --sensitive",
            "hyprctl:repl",
            "hyprctl:eval",
            "copy:--type text/plain;charset=utf-8",
        ]
    );
}

#[test]
fn falls_back_from_hyprctl_to_wtype() {
    let _lock = lock();
    let mock = MockCommands::new(Scenario {
        hyprland: true,
        hyprctl_status: 7,
        ..Scenario::default()
    });

    let backend = inject_text("回退到 wtype", &quick()).expect("wtype fallback");

    assert_eq!(backend, PasteBackend::Wtype);
    let lines = mock.lines();
    assert!(lines.contains(&"hyprctl:eval".to_owned()));
    assert!(lines.contains(&"wtype:-M ctrl -k v -m ctrl".to_owned()));
    assert_eq!(mock.copy(1).as_deref(), Some(ORIGINAL.as_bytes()));
}

#[test]
fn outside_hyprland_uses_wtype_then_ydotool() {
    let _lock = lock();
    let mock = MockCommands::new(Scenario {
        wtype_status: 1,
        ..Scenario::default()
    });

    let backend = inject_text("回退路径", &quick()).expect("ydotool fallback should succeed");

    assert_eq!(backend, PasteBackend::Ydotool);
    let lines = mock.lines();
    assert!(!lines.iter().any(|line| line.starts_with("hyprctl")));
    assert!(lines.contains(&"ydotool:key 29:1 47:1 47:0 29:0".to_owned()));
    assert_eq!(mock.copy(1).as_deref(), Some(ORIGINAL.as_bytes()));
}

#[test]
fn keeps_result_on_clipboard_when_no_backend_can_paste() {
    let _lock = lock();
    let mock = MockCommands::new(Scenario {
        wtype_status: 1,
        ydotool_status: 1,
        ..Scenario::default()
    });

    let backend = inject_text("手动粘贴", &quick()).expect("clipboard-only is not an error");

    assert_eq!(backend, PasteBackend::ClipboardOnly);
    assert_eq!(mock.copy(0).as_deref(), Some("手动粘贴".as_bytes()));
    assert_eq!(
        mock.copy(1),
        None,
        "the dictated text must stay on the clipboard"
    );
}

#[test]
fn restores_clipboard_even_when_first_copy_fails() {
    let _lock = lock();
    let mock = MockCommands::new(Scenario {
        fail_first_copy: true,
        ..Scenario::default()
    });

    let error = inject_text("不会上屏", &quick()).expect_err("copy failure should surface");

    assert!(format!("{error:#}").contains("wl-copy 返回失败状态"));
    assert_eq!(mock.copy(0).as_deref(), Some(ORIGINAL.as_bytes()));
    assert!(!mock.lines().iter().any(|line| line.starts_with("wtype")));
}

#[test]
fn clears_clipboard_afterwards_when_it_was_empty() {
    let _lock = lock();
    let mock = MockCommands::new(Scenario {
        empty_clipboard: true,
        ..Scenario::default()
    });

    let backend = inject_text("空剪贴板", &quick()).expect("injection should succeed");

    assert_eq!(backend, PasteBackend::Wtype);
    assert_eq!(
        mock.lines().last().map(String::as_str),
        Some("copy:--clear")
    );
}

#[test]
fn copy_only_mode_never_simulates_keys() {
    let _lock = lock();
    let mock = MockCommands::new(Scenario {
        hyprland: true,
        ..Scenario::default()
    });

    let options = InjectOptions {
        auto_paste: false,
        ..quick()
    };
    let backend = inject_text("只复制", &options).expect("copy-only should succeed");

    assert_eq!(backend, PasteBackend::ClipboardOnly);
    assert!(!mock
        .lines()
        .iter()
        .any(|line| line.starts_with("hyprctl") || line.starts_with("wtype")));
    assert_eq!(mock.copy(1), None);
}

#[test]
fn restores_password_manager_content_as_sensitive() {
    let _lock = lock();
    let mock = MockCommands::new(Scenario {
        sensitive_clipboard: true,
        ..Scenario::default()
    });

    inject_text("不要泄露密码", &quick()).expect("injection should succeed");

    assert_eq!(
        mock.lines().last().map(String::as_str),
        Some("copy:--type text/plain;charset=utf-8 --sensitive"),
        "恢复带敏感标记的原内容时必须保留 --sensitive，否则剪贴板历史会记下密码"
    );
    assert_eq!(mock.copy(1).as_deref(), Some(ORIGINAL.as_bytes()));
}
