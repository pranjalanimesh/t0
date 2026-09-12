#!/usr/bin/env python3
"""Renders docs/t0.gif: seeds a throwaway tree, drives t0 through a pseudo-terminal
with timed keys, and hands the recording to agg (brew install agg). Run from the
repo root after `cargo build --release`:

    python3 docs/record.py
"""
import codecs, datetime as dt, fcntl, json, os, pty, select, signal, struct, subprocess, sys, tempfile, termios, time

COLS, ROWS = 92, 20
OUT = "docs/t0.gif"
BIN = "target/release/t0"


def seed(root):
    now = dt.datetime.now().astimezone()
    ts = lambda hours_ago: (now - dt.timedelta(hours=hours_ago)).isoformat(timespec="seconds")
    due = lambda hours: (now + dt.timedelta(hours=hours)).isoformat(timespec="seconds")
    wait = lambda ago, span, until: f"- {ts(ago)} wait {span} | due={due(until)}"
    threads = {
        "infra": ("infra", [f"- {ts(300)} t0 keep the lights on"]),
        "infra/dns": ("dns", [f"- {ts(120)} t0 move the zones off the old registrar",
                              f"- {ts(26)} action drafted the cutover plan"]),
        "infra/aws": ("aws", [f"- {ts(70)} t0 EBS throttling on the prod db",
                              f"- {ts(48)} action asked the vendor for a quota bump",
                              wait(48, "2d", -4)]),
        "hiring": ("hiring", [f"- {ts(216)} t0 backfill for the platform role",
                              f"- {ts(50)} action mailed two candidates",
                              wait(50, "1w", 118)]),
        "blog-post": ("blog post", [f"- {ts(144)} t0 write up the outage",
                                    f"- {ts(20)} done published"]),
        "tax-return": ("tax return", [f"- {ts(30)} t0 accountant has the documents",
                                      wait(30, "3d", 42)]),
        "offsite": ("book the offsite", [f"- {ts(3)} t0 venue, dates, and who is coming"]),
    }
    for path, (title, lines) in threads.items():
        d = os.path.join(root, path)
        os.makedirs(d, exist_ok=True)
        with open(os.path.join(d, "thread.md"), "w") as f:
            f.write(f"# {title}\n\n" + "\n".join(lines) + "\n")


# what the recording does, as (seconds to wait, then keys to send)
DOWN = "\x1b[B"
SCRIPT = [
    (2.9, "l"),            # the banner plays, then open infra
    (0.8, DOWN), (0.4, DOWN), (0.4, DOWN),
    (0.7, "l"),            # open aws: its log, and the wait that ran out
    (1.2, "a"),
    (0.6, "they approved the bump, wait 1d"),
    (0.5, "\r"),           # aws goes from red to blue
    (1.8, "v"),            # the open view
    (1.3, "v"),            # only my move
    (1.3, "V"), (0.3, "V"),
    (0.9, "\x1b"),        # clear the note
    (2.5, ""),             # hold on the tree; the recording ends here, not on a bare shell
]


def record(home):
    pid, fd = pty.fork()
    if pid == 0:
        os.environ["HOME"] = home
        os.environ["TERM"] = "xterm-256color"
        os.execv(BIN, [BIN])
    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", ROWS, COLS, 0, 0))
    decoder = codecs.getincrementaldecoder("utf-8")("replace")
    events, start = [], time.monotonic()

    def drain(until):
        while time.monotonic() < until:
            r, _, _ = select.select([fd], [], [], 0.02)
            if not r:
                continue
            try:
                data = os.read(fd, 65536)
            except OSError:
                return
            events.append([round(time.monotonic() - start, 4), "o", decoder.decode(data)])

    for pause, keys in SCRIPT:
        drain(time.monotonic() + pause)
        for ch in keys if len(keys) > 1 and keys != DOWN else [keys] if keys else []:
            os.write(fd, ch.encode())
            drain(time.monotonic() + 0.045)
    drain(time.monotonic() + 0.5)
    try:
        os.kill(pid, signal.SIGKILL)
    except ProcessLookupError:
        pass
    os.waitpid(pid, 0)
    return events


def main():
    if not os.path.exists(BIN):
        sys.exit(f"{BIN} missing: cargo build --release first")
    with tempfile.TemporaryDirectory() as home:
        seed(os.path.join(home, ".t0"))
        events = record(home)
        cast = os.path.join(home, "t0.cast")
        with open(cast, "w") as f:
            f.write(json.dumps({"version": 2, "width": COLS, "height": ROWS, "env": {"TERM": "xterm-256color"}}) + "\n")
            for e in events:
                f.write(json.dumps(e) + "\n")
        theme = "101216,e6e9ee,101216,ff6b6b,6fc48b,e0a458,7aa7ff,c8a2ff,7ad0d0,e6e9ee,6b7280,ff6b6b,6fc48b,e0a458,7aa7ff,c8a2ff,7ad0d0,ffffff"
        subprocess.run(["agg", "--font-size", "20", "--line-height", "1.3", "--theme", theme, "--fps-cap", "30", cast, OUT], check=True)
    print(f"wrote {OUT}")


if __name__ == "__main__":
    main()
