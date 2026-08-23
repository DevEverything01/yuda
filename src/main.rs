use anyhow::Result;
use std::{
    env, fs,
    io::{self, Read},
    path::PathBuf,
};
use yuda::{
    app::{Session, Transcript},
    config::Config,
    ui::{demo_recording_session, demo_session, UiSnapshot},
};

fn main() -> Result<()> {
    #[cfg(feature = "linux-runtime")]
    init_logging();
    let mut args = env::args().skip(1);
    match args.next().as_deref() {
        Some("--daemon") => run_daemon(),
        Some("--ui") => {
            print_ui(&demo_session());
            Ok(())
        }
        Some("--demo-recording") => {
            print_ui(&demo_recording_session());
            Ok(())
        }
        Some("--json") => print_json(&demo_session()),
        Some("--config") => {
            print_config_path();
            Ok(())
        }
        Some("--simulate") => run_simulation(),
        Some("--help") | None => {
            print_help();
            Ok(())
        }
        Some(command) => {
            eprintln!("未知命令：{command}");
            print_help();
            Ok(())
        }
    }
}

#[cfg(feature = "linux-runtime")]
fn init_logging() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();
}

#[cfg(feature = "linux-runtime")]
fn run_daemon() -> Result<()> {
    #[cfg(feature = "sensevoice")]
    {
        return yuda::daemon::run();
    }
    #[cfg(not(feature = "sensevoice"))]
    {
        anyhow::bail!("daemon 需要使用 `--features linux-runtime,sensevoice` 构建");
    }
}

#[cfg(not(feature = "linux-runtime"))]
fn run_daemon() -> Result<()> {
    anyhow::bail!(
        "daemon 仅支持 Linux；请在 Omarchy 上使用 --features linux-runtime,sensevoice 构建"
    )
}

fn print_help() {
    println!("语打 Yuda · 中文优先的语音输入");
    println!();
    println!("用法：");
    println!("  yuda --daemon          启动 Linux 全局语音输入 daemon");
    println!("  yuda --ui              查看空闲 UI 状态");
    println!("  yuda --demo-recording 查看录音中 UI 状态");
    println!("  yuda --json            输出 Waybar 风格状态");
    println!("  yuda --config          显示配置路径");
    println!("  yuda --simulate        在终端走一遍键盘/语音状态机");
}

fn print_ui(session: &Session) {
    let snapshot = UiSnapshot::from(session);
    println!("{}", snapshot.eyebrow);
    println!("{}", snapshot.title);
    if !snapshot.transcript.is_empty() {
        println!("“{}”", snapshot.transcript);
    }
    println!(
        "{} · {} · {}",
        snapshot.engine_label, snapshot.hotkey_label, snapshot.hint
    );
}

fn print_json(session: &Session) -> Result<()> {
    let snapshot = UiSnapshot::from(session);
    let state = match snapshot.state {
        yuda::app::SessionState::Recording => "recording",
        yuda::app::SessionState::Transcribing => "transcribing",
        yuda::app::SessionState::Refining => "refining",
        yuda::app::SessionState::Ready => "ready",
        yuda::app::SessionState::Injecting => "injecting",
        yuda::app::SessionState::Error => "error",
        yuda::app::SessionState::Idle => "idle",
    };
    println!(
        "{}",
        serde_json::json!({"text": snapshot.title, "class": state, "alt": snapshot.hint})
    );
    Ok(())
}

fn print_config_path() {
    match Config::default_path() {
        Some(path) => println!("{}", path.display()),
        None => println!("无法确定 HOME，配置路径不可用"),
    }
}

fn run_simulation() -> Result<()> {
    let mut input = String::new();
    io::stdin().read_to_string(&mut input)?;
    let text = input.trim();
    let text = if text.is_empty() {
        "测试键盘与语音输入流程"
    } else {
        text
    };
    let mut session = demo_session();
    print_ui(&session);
    session.start_recording();
    session.update_meter(68, 1_600);
    session.stop_recording(Transcript::new(text, 92));
    println!();
    print_ui(&session);
    session.begin_refining();
    print_ui(&session);
    session.mark_ready();
    print_ui(&session);
    Ok(())
}

#[allow(dead_code)]
fn read_optional_file(path: &PathBuf) -> Result<Option<String>> {
    if !path.exists() {
        return Ok(None);
    }
    Ok(Some(fs::read_to_string(path)?))
}
