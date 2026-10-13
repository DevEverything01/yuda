#!/usr/bin/env python3
"""语打 Yuda 假 daemon：开发 Omarchy shell 插件 yuda.dictation 用。

在 $XDG_RUNTIME_DIR/yuda/yuda.sock 上实现 docs/ipc.md 的协议 v1
（start / stop / toggle / cancel / status / subscribe），只用 Python 标准库，
不录音、不识别、不上屏。真 daemon 正在监听时拒绝启动，不会删除它的 socket。

注意：真 daemon 会录音并把文字粘贴到当前焦点窗口。先停掉它再启动本工具，
用完再启动回来；本工具拒绝启动时，下面的 socat 命令会发给真 daemon。
  systemctl --user stop yuda.service    # 开始前
  systemctl --user start yuda.service   # 结束后

用法：
  # 演示模式（默认）：循环播放脚本化会话——
  #   普通录音 → 识别 → 纠错 → 上屏 → 空闲；离线降级（fallback=true）；
  #   长文本；麦克风错误（error 1.5 s 后回到 idle）。
  python3 fake-daemon.py demo [--once]

  # 交互模式：像真 daemon 一样响应 start/stop/toggle/cancel，
  # 录音时生成模拟电平与逐字增长的文字。
  python3 fake-daemon.py interactive

  # 定格模式：一直保持某个状态（录音中电平持续变化），便于截图。
  python3 fake-daemon.py hold recording --text "今天下午三点在会议室讨论 Python 项目的发布计划"
  python3 fake-daemon.py hold recording --text "" --fallback --engine offline
  python3 fake-daemon.py hold transcribing --text "今天下午三点"
  python3 fake-daemon.py hold error --message "麦克风不可用：设备忙"

  # 手动发命令（socat 已随 Omarchy 安装；确认监听的是本工具而不是真 daemon）：
  echo subscribe | socat - UNIX-CONNECT:"$XDG_RUNTIME_DIR/yuda/yuda.sock"
  echo toggle    | socat - UNIX-CONNECT:"$XDG_RUNTIME_DIR/yuda/yuda.sock"

选项：--socket PATH 改用其他 socket 路径。Ctrl+C 或 SIGTERM 退出并删除 socket。
"""

import argparse
import asyncio
import json
import math
import os
import random
import signal
import socket
import sys
import time

PROTOCOL_VERSION = 1
FRAME_HZ = 30

SHORT_TEXT = "今天下午三点在会议室讨论 Python 项目的发布计划"
LONG_TEXT = (
    "今天下午三点在会议室讨论 Python 项目的发布计划，请大家提前准备好各自负责模块的进度报告，"
    "并在周五之前把测试用例补齐，我们争取下周一发布第一个公开测试版本"
)
OFFLINE_TEXT = "网络断了也能继续听写，识别会自动切换到本地模型"
ERROR_MESSAGE = "麦克风不可用：设备正忙"


def encode(obj):
    """One protocol line: compact UTF-8 JSON and a newline."""
    return (json.dumps(obj, ensure_ascii=False, separators=(",", ":")) + "\n").encode()


def default_socket_path():
    runtime = os.environ.get("XDG_RUNTIME_DIR") or f"/run/user/{os.getuid()}"
    return os.path.join(runtime, "yuda", "yuda.sock")


def partial_steps(text):
    """Split text into partial-result steps: one CJK character or one ASCII word."""
    out, word = [], ""
    for ch in text:
        if ch.isascii() and not ch.isspace():
            word += ch
            continue
        if word:
            out.append(word)
            word = ""
        out.append(ch)
    if word:
        out.append(word)
    return out


def speech_level(t):
    """A speech-like RMS envelope in 0..1: syllables, phrases and short pauses."""
    syllable = abs(math.sin(t * 6.5)) ** 1.5
    phrase = 0.55 + 0.45 * math.sin(t * 1.1)
    pause = 0.15 if (t % 4.0) > 3.4 else 1.0
    noise = random.uniform(-0.05, 0.05)
    return max(0.0, min(1.0, (0.08 + 0.75 * syllable * phrase) * pause + noise))


class Daemon:
    def __init__(self):
        self.snapshot = {
            "v": PROTOCOL_VERSION,
            "state": "idle",
            "level": 0.0,
            "text": "",
            "engine": "cloud",
            "fallback": False,
            "language": "zh",
            "message": "",
            "elapsed_ms": 0,
        }
        self.subscribers = set()
        self.session = None  # asyncio.Task driving the current state changes

    def line(self):
        return encode(self.snapshot)

    def publish(self, **changes):
        state = changes.get("state")
        if state and state != self.snapshot["state"]:
            # Wall-clock stamps let screenshots and frame grabs be lined up.
            print(f"fake-daemon: {time.time():.3f} {self.snapshot['state']} -> {state}", flush=True)
        self.snapshot.update(changes)
        data = self.line()
        for writer in list(self.subscribers):
            if writer.is_closing():
                self.subscribers.discard(writer)
                continue
            # A subscriber that stops reading must not grow our buffers forever.
            if writer.transport.get_write_buffer_size() > 1 << 20:
                writer.close()
                self.subscribers.discard(writer)
                continue
            writer.write(data)

    # ---------------------------------------------------------- scripted parts

    async def record(self, text, seconds, engine="cloud", fallback=False):
        steps = partial_steps(text)
        start = time.monotonic()
        shown = 0
        self.publish(state="recording", level=0.0, text="", engine=engine,
                     fallback=fallback, message="", elapsed_ms=0)
        while True:
            elapsed = time.monotonic() - start
            if elapsed >= seconds:
                break
            # Partial results lag the audio a little, then catch up.
            want = int(len(steps) * min(1.0, max(0.0, (elapsed - 0.6) / max(0.1, seconds - 1.2))))
            shown = max(shown, want)
            self.publish(level=round(speech_level(elapsed), 3), text="".join(steps[:shown]),
                         elapsed_ms=int(elapsed * 1000))
            await asyncio.sleep(1 / FRAME_HZ)
        self.publish(level=0.0, text=text)

    async def finish(self, text, refine=True):
        self.publish(state="transcribing", level=0.0, text=text)
        await asyncio.sleep(1.2)
        if refine:
            self.publish(state="refining")
            await asyncio.sleep(0.8)
        self.publish(state="injecting")
        await asyncio.sleep(0.4)
        self.publish(state="idle", text="", message="")

    async def fail(self, message):
        self.publish(state="error", level=0.0, text="", message=message)
        await asyncio.sleep(1.5)
        self.publish(state="idle", message="")

    async def demo(self, once):
        while True:
            await asyncio.sleep(1.5)
            await self.record(SHORT_TEXT, 6.0)
            await self.finish(SHORT_TEXT)
            await asyncio.sleep(1.5)
            await self.record(OFFLINE_TEXT, 5.0, engine="offline", fallback=True)
            await self.finish(OFFLINE_TEXT, refine=False)
            await asyncio.sleep(1.5)
            await self.record(LONG_TEXT, 9.0)
            await self.finish(LONG_TEXT)
            await asyncio.sleep(1.5)
            await self.record("", 1.2)
            await self.fail(ERROR_MESSAGE)
            self.publish(engine="cloud", fallback=False)
            if once:
                return

    async def hold(self, state, text, message, engine, fallback):
        self.publish(state=state, text=text, message=message, engine=engine,
                     fallback=fallback, level=0.0)
        start = time.monotonic()
        if state != "recording":
            await asyncio.Event().wait()
        while True:
            elapsed = time.monotonic() - start
            self.publish(level=round(speech_level(elapsed), 3), elapsed_ms=int(elapsed * 1000))
            await asyncio.sleep(1 / FRAME_HZ)

    # ------------------------------------------------------------ commands

    def run_session(self, coro):
        if self.session and not self.session.done():
            self.session.cancel()
        self.session = asyncio.ensure_future(coro)

    async def interactive_record(self):
        steps = partial_steps(SHORT_TEXT)
        start = time.monotonic()
        self.publish(state="recording", level=0.0, text="", message="", elapsed_ms=0)
        while True:
            elapsed = time.monotonic() - start
            shown = min(len(steps), int(max(0.0, elapsed - 0.6) / 0.18))
            self.publish(level=round(speech_level(elapsed), 3), text="".join(steps[:shown]),
                         elapsed_ms=int(elapsed * 1000))
            await asyncio.sleep(1 / FRAME_HZ)

    def command(self, name, interactive):
        state = self.snapshot["state"]
        if not interactive:
            return {"ok": True}
        if name == "toggle":
            name = "stop" if state == "recording" else "start"
        if name == "start":
            if state != "recording":
                self.run_session(self.interactive_record())
        elif name == "stop":
            if state == "recording":
                text = self.snapshot["text"]
                if text:
                    self.run_session(self.finish(text))
                else:
                    self.run_session(self.fail("没有识别到语音"))
        elif name == "cancel":
            if state == "recording":
                self.run_session(asyncio.sleep(0))
                self.publish(state="idle", level=0.0, text="")
        return {"ok": True}

    async def handle(self, reader, writer, interactive):
        try:
            raw = await asyncio.wait_for(reader.readline(), timeout=5)
        except (asyncio.TimeoutError, ConnectionError):
            writer.close()
            return
        name = raw.decode(errors="replace").strip()
        if name == "subscribe":
            writer.write(self.line())
            self.subscribers.add(writer)
            try:
                # Wait for the client to hang up; anything it sends is ignored.
                while await reader.read(4096):
                    pass
            except ConnectionError:
                pass
            finally:
                self.subscribers.discard(writer)
                writer.close()
            return
        if name == "status":
            writer.write(self.line())
        elif name in ("start", "stop", "toggle", "cancel"):
            reply = self.command(name, interactive)
            writer.write(encode(reply))
        else:
            reply = {"ok": False, "error": f"未知命令：{name}"}
            writer.write(encode(reply))
        try:
            await writer.drain()
        except ConnectionError:
            pass
        writer.close()


def claim_socket(path):
    """Refuse to replace a live daemon; remove only a stale socket file."""
    directory = os.path.dirname(path)
    os.makedirs(directory, mode=0o700, exist_ok=True)
    os.chmod(directory, 0o700)
    if not os.path.exists(path):
        return
    probe = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    try:
        probe.connect(path)
    except OSError:
        os.unlink(path)
        return
    finally:
        probe.close()
    sys.exit(f"fake-daemon: {path} 已有进程在监听（真 daemon？），拒绝启动")


async def main(args):
    daemon = Daemon()
    path = args.socket
    claim_socket(path)
    interactive = args.mode == "interactive"
    server = await asyncio.start_unix_server(
        lambda r, w: daemon.handle(r, w, interactive), path=path)
    os.chmod(path, 0o600)
    print(f"fake-daemon: listening on {path} ({args.mode})", flush=True)

    stop = asyncio.Event()
    loop = asyncio.get_running_loop()
    for sig in (signal.SIGINT, signal.SIGTERM):
        loop.add_signal_handler(sig, stop.set)

    if args.mode == "demo":
        daemon.run_session(daemon.demo(args.once))
    elif args.mode == "hold":
        daemon.run_session(daemon.hold(args.state, args.text, args.message,
                                       args.engine, args.fallback))

    waiters = [asyncio.ensure_future(stop.wait())]
    if args.mode == "demo" and args.once:
        waiters.append(daemon.session)
    await asyncio.wait(waiters, return_when=asyncio.FIRST_COMPLETED)

    server.close()
    for writer in list(daemon.subscribers):
        writer.close()
    try:
        os.unlink(path)
    except FileNotFoundError:
        pass


def parse_args(argv):
    parser = argparse.ArgumentParser(description="语打 Yuda 假 daemon（插件开发用）")
    parser.add_argument("--socket", default=default_socket_path(), help="socket 路径")
    modes = parser.add_subparsers(dest="mode")

    demo = modes.add_parser("demo", help="循环播放脚本化会话（默认）")
    demo.add_argument("--once", action="store_true", help="只播放一轮后退出")

    modes.add_parser("interactive", help="响应 start/stop/toggle/cancel")

    hold = modes.add_parser("hold", help="定格在某个状态，便于截图")
    hold.add_argument("state", choices=["idle", "recording", "transcribing", "refining",
                                        "injecting", "error"])
    hold.add_argument("--text", default=SHORT_TEXT)
    hold.add_argument("--message", default="")
    hold.add_argument("--engine", choices=["cloud", "offline"], default="cloud")
    hold.add_argument("--fallback", action="store_true")

    args = parser.parse_args(argv)
    if args.mode is None:
        args.mode = "demo"
        args.once = False
    if args.mode == "hold" and args.state == "error" and not args.message:
        args.message = ERROR_MESSAGE
    if args.mode == "hold" and args.fallback:
        args.engine = "offline"
    return args


if __name__ == "__main__":
    try:
        asyncio.run(main(parse_args(sys.argv[1:])))
    except KeyboardInterrupt:
        pass
