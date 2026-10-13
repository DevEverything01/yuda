use anyhow::{bail, Context, Result};
use std::{
    io::Write,
    path::Path,
    process::{Command as Process, ExitCode, Stdio},
    thread,
    time::{Duration, Instant},
};
use yuda::{
    config::{self, Config, EngineMode, Language},
    history,
    injection::{inject_text, InjectOptions, PasteBackend},
    ipc::{self, Command},
};

const USAGE: &str = "语打 Yuda · Omarchy 上的中文语音输入

用法：
  yuda daemon                     启动 daemon（由 systemd 用户服务 yuda.service 调用）
  yuda start | stop | toggle      开始 / 结束 / 切换录音（Omarchy 听写快捷键调用）
  yuda start --edit | toggle --edit   录一条修改指令，作用于上一段（Shift+F9）
  yuda edit <指令>                直接修改上一段，例如 yuda edit 把9点改成10点
  yuda cancel                     丢弃当前录音
  yuda status [--json]            查看状态
  yuda watch                      持续打印状态流（调试）
  yuda reload                     让 daemon 重新读取配置
  yuda history [条数]             最近的识别结果
  yuda paste-last                 重新上屏最近一条结果
  yuda transcribe <文件.wav>      用离线引擎转写 WAV 文件
  yuda config path                配置文件路径
  yuda config get <键>            读取配置，例如 yuda config get engine
  yuda config set <键> <值>       修改配置并保留注释，例如 yuda config set llm.enabled true
  yuda config set-string <键> <值> 按字符串保存，例如 yuda config set-string cloud.access_key 123
  yuda menu <engine|language|fillers|polish|history>  通过 Omarchy 菜单设置
  yuda omarchy-menu <install|remove>   合并 / 移除 Omarchy 菜单扩展条目
  yuda doctor                     检查运行环境
  yuda version                    版本";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    init_logging(args.first().map(String::as_str) == Some("daemon"));
    match run(&args) {
        Ok(code) => code,
        Err(error) => {
            eprintln!("语打：{error:#}");
            ExitCode::FAILURE
        }
    }
}

fn init_logging(daemon: bool) {
    let default = if daemon { "info" } else { "warn" };
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(default));
    // systemd 下 stderr 进 journald：不输出 ANSI 颜色码。
    let ansi = std::io::IsTerminal::is_terminal(&std::io::stderr());
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_ansi(ansi)
        .with_writer(std::io::stderr)
        .with_target(false)
        .init();
}

fn run(args: &[String]) -> Result<ExitCode> {
    let rest = args.get(1..).unwrap_or_default();
    match args.first().map(String::as_str) {
        Some("daemon") => run_daemon(),
        Some("start") if rest.first().map(String::as_str) == Some("--edit") => {
            control(Command::StartEdit, true)
        }
        Some("toggle") if rest.first().map(String::as_str) == Some("--edit") => {
            control(Command::ToggleEdit, true)
        }
        Some("start") => control(Command::Start, true),
        Some("toggle") => control(Command::Toggle, true),
        Some("edit") => edit(rest),
        Some("stop") => control(Command::Stop, false),
        Some("cancel") => control(Command::Cancel, false),
        Some("reload") => control(Command::Reload, false),
        Some("status") => status(rest.first().map(String::as_str) == Some("--json")),
        Some("watch") => watch(),
        Some("history") => show_history(rest.first()),
        Some("paste-last") => paste_last(),
        Some("transcribe") => transcribe(rest),
        Some("config") => config_command(rest),
        Some("menu") => menu(rest.first().map(String::as_str)),
        Some("omarchy-menu") => omarchy_menu(rest.first().map(String::as_str)),
        Some("doctor") => doctor(),
        Some("version" | "--version" | "-V") => {
            println!("yuda {}", env!("CARGO_PKG_VERSION"));
            Ok(ExitCode::SUCCESS)
        }
        Some("help" | "--help" | "-h") | None => {
            println!("{USAGE}");
            Ok(ExitCode::SUCCESS)
        }
        Some(other) => bail!("未知命令：{other}\n\n{USAGE}"),
    }
}

#[cfg(all(feature = "audio", feature = "sensevoice"))]
fn run_daemon() -> Result<ExitCode> {
    yuda::daemon::run()?;
    Ok(ExitCode::SUCCESS)
}

#[cfg(not(all(feature = "audio", feature = "sensevoice")))]
fn run_daemon() -> Result<ExitCode> {
    bail!("此构建不含 daemon；请用 `cargo build --release --features daemon` 构建")
}

const CONTROL_TIMEOUT: Duration = Duration::from_millis(1_500);

/// 发送控制命令。`autostart` 为真时，daemon 未运行就先通过 systemd 拉起。
///
/// 退出码：0 成功；1 daemon 拒绝（界面上已有提示）；2 daemon 无法连接或启动。
fn control(command: Command, autostart: bool) -> Result<ExitCode> {
    let reply = match ipc::call(command, CONTROL_TIMEOUT) {
        Ok(reply) => reply,
        Err(error) if !autostart => {
            // stop / cancel：daemon 不在就等于已经停了（Omarchy 后端契约要求成功返回）。
            tracing::debug!(%error, "daemon 未运行");
            return Ok(ExitCode::SUCCESS);
        }
        Err(error) => {
            tracing::info!(%error, "daemon 未运行，尝试启动 yuda.service");
            match start_service().and_then(|()| ipc::call(command, CONTROL_TIMEOUT)) {
                Ok(reply) => reply,
                Err(error) => {
                    eprintln!("语打：{error:#}");
                    return Ok(ExitCode::from(2));
                }
            }
        }
    };
    if reply.ok {
        Ok(ExitCode::SUCCESS)
    } else {
        eprintln!(
            "语打：{}",
            reply
                .error
                .unwrap_or_else(|| "daemon 拒绝了请求".to_owned())
        );
        Ok(ExitCode::FAILURE)
    }
}

fn edit(args: &[String]) -> Result<ExitCode> {
    let instruction = args.join(" ");
    if instruction.trim().is_empty() {
        bail!("用法：yuda edit <指令>，例如 yuda edit 把9点改成10点");
    }
    let reply = ipc::edit(&instruction, CONTROL_TIMEOUT)?;
    if reply.ok {
        Ok(ExitCode::SUCCESS)
    } else {
        eprintln!(
            "语打：{}",
            reply
                .error
                .unwrap_or_else(|| "daemon 拒绝了请求".to_owned())
        );
        Ok(ExitCode::FAILURE)
    }
}

fn start_service() -> Result<()> {
    let status = Process::new("systemctl")
        .args(["--user", "start", "--no-block", "yuda.service"])
        .status()
        .context("无法执行 systemctl；请手动运行 yuda daemon")?;
    if !status.success() {
        bail!("启动 yuda.service 失败；运行 `systemctl --user status yuda` 查看原因");
    }
    let path = ipc::socket_path()?;
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if path.exists() && ipc::status(Duration::from_millis(300)).is_ok() {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(50));
    }
    bail!("yuda.service 5 秒内没有就绪；运行 `journalctl --user -u yuda` 查看日志")
}

fn status(json: bool) -> Result<ExitCode> {
    match ipc::status(CONTROL_TIMEOUT) {
        Ok(snapshot) if json => println!("{}", snapshot.to_line().trim_end()),
        Ok(snapshot) => {
            let mut line = format!(
                "{} · {}{} · {}",
                snapshot.state.label(),
                snapshot.engine.label(),
                if snapshot.fallback {
                    "（已降级）"
                } else {
                    ""
                },
                snapshot.language
            );
            if !snapshot.message.is_empty() {
                line.push_str(&format!(" · {}", snapshot.message));
            }
            println!("{line}");
            if !snapshot.text.is_empty() {
                println!("{}", snapshot.text);
            }
        }
        Err(_) if json => println!(r#"{{"v":1,"state":"stopped"}}"#),
        Err(_) => {
            println!("语打 daemon 未运行（systemctl --user start yuda）");
            return Ok(ExitCode::from(3));
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn watch() -> Result<ExitCode> {
    for snapshot in ipc::subscribe()? {
        println!("{}", snapshot?.to_line().trim_end());
    }
    Ok(ExitCode::SUCCESS)
}

fn show_history(limit: Option<&String>) -> Result<ExitCode> {
    let limit = match limit {
        Some(value) => value.parse().context("条数必须是正整数")?,
        None => 10,
    };
    for entry in history::recent(&history::default_path()?, limit)? {
        println!("{}  [{}]  {}", entry.local_time(), entry.engine, entry.text);
    }
    Ok(ExitCode::SUCCESS)
}

fn paste_last() -> Result<ExitCode> {
    let entry = history::recent(&history::default_path()?, 1)?
        .into_iter()
        .next()
        .context("还没有识别历史")?;
    paste_text(&entry.text)
}

fn paste_text(text: &str) -> Result<ExitCode> {
    let config = Config::load_default()?;
    let options = InjectOptions {
        paste_delay: Duration::from_millis(config.injection.paste_delay_ms),
        auto_paste: config.injection.auto_paste,
    };
    let backend = inject_text(text, &options)?;
    if backend == PasteBackend::ClipboardOnly {
        notify("语打", "结果已复制到剪贴板，按 Super+V 粘贴");
    }
    Ok(ExitCode::SUCCESS)
}

#[cfg(feature = "sensevoice")]
fn transcribe(args: &[String]) -> Result<ExitCode> {
    let (path, language) = match args {
        [path] => (path, None),
        [path, flag, language] if flag == "--sv-language" => (path, Some(language.as_str())),
        _ => bail!("用法：yuda transcribe <文件.wav> [--sv-language auto|zh|en|yue|ja|ko]"),
    };
    let config = Config::load_default()?;
    let recognizer = match language {
        Some(language) => yuda::sensevoice::SenseVoice::load_with_language(&config, language)?,
        None => yuda::sensevoice::SenseVoice::load(&config)?,
    };
    let started = Instant::now();
    let text = recognizer.transcribe_wav(Path::new(path))?;
    let text = yuda::text::process(&text, &config.text);
    eprintln!("耗时 {} ms", started.elapsed().as_millis());
    println!("{text}");
    Ok(ExitCode::SUCCESS)
}

#[cfg(not(feature = "sensevoice"))]
fn transcribe(_args: &[String]) -> Result<ExitCode> {
    bail!("此构建不含离线引擎；请用 --features daemon 构建")
}

fn config_command(args: &[String]) -> Result<ExitCode> {
    let path = Config::default_path()?;
    match args {
        [action] if action == "path" => println!("{}", path.display()),
        [action, key] if action == "get" => {
            println!("{}", config::get_value(&Config::load(&path)?, key)?);
        }
        [action, key, value] if action == "set" || action == "set-string" => {
            let value = if action == "set-string" {
                toml_string(value)
            } else {
                value.clone()
            };
            config::set_value(&path, key, &value)?;
            // daemon 未运行时修改照样生效于下次启动。
            let _ = ipc::call(Command::Reload, CONTROL_TIMEOUT);
        }
        _ => bail!("用法：yuda config path | get <键> | set <键> <值> | set-string <键> <字符串>"),
    }
    Ok(ExitCode::SUCCESS)
}

/// 生成 TOML 基本字符串字面量，保证密钥一类的值按字符串保存。
fn toml_string(value: &str) -> String {
    let mut quoted = String::from("\"");
    for c in value.chars() {
        match c {
            '"' => quoted.push_str("\\\""),
            '\\' => quoted.push_str("\\\\"),
            c if c.is_control() => quoted.push_str(&format!("\\u{:04X}", c as u32)),
            c => quoted.push(c),
        }
    }
    quoted.push('"');
    quoted
}

/// 通过 Omarchy 的原生选择菜单（omarchy-menu-select）修改设置。
fn menu(which: Option<&str>) -> Result<ExitCode> {
    let path = Config::default_path()?;
    let config = Config::load(&path)?;
    match which {
        Some("engine") => {
            let rows: Vec<String> = EngineMode::ALL
                .iter()
                .map(|mode| {
                    let mark = if *mode == config.engine { "󰄬" } else { " " };
                    format!("{mark}\t{}\t{}", mode.label(), mode.as_str())
                })
                .collect();
            let Some(choice) = menu_select("语打 · 识别引擎", &rows)? else {
                return Ok(ExitCode::SUCCESS);
            };
            let mode: EngineMode = choice.parse()?;
            if mode != EngineMode::Offline && !config.cloud.is_configured() {
                notify(
                    "语打",
                    "云端引擎需要火山引擎 App Key 与 Access Key；已打开配置文件",
                );
                open_editor(&path);
            }
            config::set_value(&path, "engine", mode.as_str())?;
            let _ = ipc::call(Command::Reload, CONTROL_TIMEOUT);
            notify("语打", &format!("识别引擎：{}", mode.label()));
        }
        Some("language") => {
            let rows: Vec<String> = Language::ALL
                .iter()
                .map(|language| {
                    let mark = if *language == config.language {
                        "󰄬"
                    } else {
                        " "
                    };
                    format!("{mark}\t{}\t{}", language.label(), language.as_str())
                })
                .collect();
            let Some(choice) = menu_select("语打 · 识别语言", &rows)? else {
                return Ok(ExitCode::SUCCESS);
            };
            let language: Language = choice.parse()?;
            config::set_value(&path, "language", language.as_str())?;
            let _ = ipc::call(Command::Reload, CONTROL_TIMEOUT);
            notify("语打", &format!("识别语言：{}", language.label()));
        }
        Some("fillers") => {
            let enabled = !config.text.remove_fillers;
            config::set_value(&path, "text.remove_fillers", &enabled.to_string())?;
            let _ = ipc::call(Command::Reload, CONTROL_TIMEOUT);
            notify(
                "语打",
                if enabled {
                    "已开启：去除语气词"
                } else {
                    "已关闭：去除语气词"
                },
            );
        }
        Some("polish") => {
            let enabled = !config.llm.enabled;
            config::set_value(&path, "llm.enabled", &enabled.to_string())?;
            let _ = ipc::call(Command::Reload, CONTROL_TIMEOUT);
            if enabled && config.llm.api_base.trim().is_empty() {
                notify(
                    "语打",
                    "AI 纠错需要 OpenAI 兼容端点；请在 [llm] 填写 api_base 与 api_key",
                );
                open_editor(&path);
            } else {
                notify(
                    "语打",
                    if enabled {
                        "已开启：AI 纠错"
                    } else {
                        "已关闭：AI 纠错"
                    },
                );
            }
        }
        Some("history") => {
            let entries = history::recent(&history::default_path()?, 30)?;
            if entries.is_empty() {
                notify("语打", "还没有识别历史");
                return Ok(ExitCode::SUCCESS);
            }
            let rows: Vec<String> = entries
                .iter()
                .enumerate()
                .map(|(index, entry)| {
                    let label: String = entry.text.replace(['\t', '\n'], " ");
                    format!("󰆏\t{label}\t{} · #{index}", entry.local_time())
                })
                .collect();
            let Some(choice) = menu_select("语打 · 重新上屏", &rows)? else {
                return Ok(ExitCode::SUCCESS);
            };
            let index: usize = choice
                .rsplit('#')
                .next()
                .and_then(|value| value.parse().ok())
                .context("无法识别所选历史条目")?;
            let entry = entries.get(index).context("所选历史条目不存在")?;
            // 等菜单关闭、焦点回到原窗口后再粘贴。
            thread::sleep(Duration::from_millis(250));
            return paste_text(&entry.text);
        }
        _ => bail!("用法：yuda menu <engine|language|fillers|polish|history>"),
    }
    Ok(ExitCode::SUCCESS)
}

fn omarchy_menu(action: Option<&str>) -> Result<ExitCode> {
    let home = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|dir| !dir.is_empty())
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| Path::new(&home).join(".config")))
        .context("无法确定配置目录")?;
    let path = home.join("omarchy/extensions/omarchy-menu.jsonc");
    match action {
        Some("install") => yuda::omarchy::update_file(&path, true)?,
        Some("remove") => yuda::omarchy::update_file(&path, false)?,
        _ => bail!("用法：yuda omarchy-menu <install|remove>"),
    }
    // 菜单 watch 文件变化；显式刷新让变更立即可见。
    let _ = Process::new("omarchy-menu")
        .arg("refresh")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    Ok(ExitCode::SUCCESS)
}

/// 返回所选行的 subtext（第三列）；用户取消时返回 None。
fn menu_select(prompt: &str, rows: &[String]) -> Result<Option<String>> {
    let mut child = Process::new("omarchy-menu-select")
        .arg(prompt)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .context("找不到 omarchy-menu-select；该菜单只在 Omarchy 中可用")?;
    child
        .stdin
        .take()
        .context("无法打开菜单输入")?
        .write_all(rows.join("\n").as_bytes())?;
    let output = child.wait_with_output()?;
    if !output.status.success() {
        return Ok(None);
    }
    let selection = String::from_utf8_lossy(&output.stdout);
    Ok(selection
        .trim_end_matches('\n')
        .split('\t')
        .nth(1)
        .map(str::to_owned))
}

fn notify(title: &str, body: &str) {
    let sent = Process::new("omarchy-notification-send")
        .args([title, body])
        .status()
        .map(|status| status.success())
        .unwrap_or(false);
    if !sent {
        let _ = Process::new("notify-send").args([title, body]).status();
    }
}

fn open_editor(path: &Path) {
    let _ = Process::new("omarchy-launch-editor").arg(path).spawn();
}

fn doctor() -> Result<ExitCode> {
    let mut problems = 0;
    let mut check = |ok: bool, label: &str, hint: &str| {
        if ok {
            println!("  ✓ {label}");
        } else {
            problems += 1;
            println!("  ✗ {label} —— {hint}");
        }
    };
    let note = |ok: bool, label: &str, hint: &str| {
        if ok {
            println!("  ✓ {label}");
        } else {
            println!("  · {label} —— {hint}");
        }
    };
    let config_path = Config::default_path()?;
    let config = Config::load(&config_path);
    check(
        config.is_ok(),
        &format!("配置 {}", config_path.display()),
        &config
            .as_ref()
            .err()
            .map(|error| format!("{error:#}"))
            .unwrap_or_default(),
    );
    let config = config.unwrap_or_default();
    check(
        std::env::var_os("WAYLAND_DISPLAY").is_some(),
        "Wayland 会话",
        "需要在 Hyprland 会话中运行",
    );
    for program in ["wl-copy", "wl-paste"] {
        check(command_exists(program), program, "安装 wl-clipboard");
    }
    check(
        command_exists("hyprctl") || command_exists("wtype"),
        "粘贴后端（hyprctl / wtype）",
        "安装 wtype",
    );
    if config.engine != EngineMode::Cloud {
        let paths = config.offline_model_paths();
        check(
            paths.is_complete(),
            "离线模型 SenseVoice-Small",
            &format!(
                "缺少 {}；运行 omarchy-install-dictation-yuda",
                paths.directory.display()
            ),
        );
        check(
            paths.vad.is_file(),
            "VAD 模型 silero_vad.onnx",
            "缺少时退化为整段识别（无实时预览）",
        );
    }
    match config.engine {
        EngineMode::Cloud => check(
            config.cloud.is_configured(),
            "火山引擎凭据",
            "在 [cloud] 填写 access_key（以及 app_key）",
        ),
        EngineMode::Auto => note(
            config.cloud.is_configured(),
            "火山引擎凭据（可选）",
            "未配置，使用离线识别；填写 [cloud] 后启用豆包大模型",
        ),
        EngineMode::Offline => {}
    }
    check(
        ipc::status(CONTROL_TIMEOUT).is_ok(),
        "daemon 运行中",
        "systemctl --user enable --now yuda",
    );
    if command_exists("omarchy-default-dictation") {
        let backend = Process::new("omarchy-default-dictation")
            .output()
            .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
            .unwrap_or_default();
        check(
            backend == "yuda",
            "Omarchy 默认听写后端为 yuda",
            "在 Omarchy 菜单 Setup › Defaults › Dictation 选择语打",
        );
        check(
            command_exists("omarchy-dictation-yuda"),
            "omarchy-dictation-yuda 适配器",
            "重新安装 yuda 包",
        );
    }
    if problems == 0 {
        println!("一切就绪。");
        Ok(ExitCode::SUCCESS)
    } else {
        println!("{problems} 项需要处理。");
        Ok(ExitCode::FAILURE)
    }
}

fn command_exists(program: &str) -> bool {
    std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).any(|dir| dir.join(program).is_file()))
        .unwrap_or(false)
}
