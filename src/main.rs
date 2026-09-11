mod app;
mod chat;
mod command;
mod config;
mod duration;
mod format;
mod grammar;
mod input;
mod keys;
mod model;
mod reference;
mod rows;
mod store;
mod ui;

use anyhow::Result;
use app::App;
use keys::Action;
use chrono::{DateTime, Local};
use command::{Cmd, Effect};
use reference::Here;
use crossterm::event::{self, Event, KeyEventKind};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::ExecutableCommand;
use model::Thread;
use std::io::{stdout, Stdout};
use std::time::{Duration as StdDuration, Instant};

/// macOS banner. Quietly does nothing elsewhere.
pub fn notify(title: &str, body: &str) {
    if !cfg!(target_os = "macos") {
        return;
    }
    let script = format!(
        "display notification \"{}\" with title \"{}\"",
        body.replace('\\', "\\\\").replace('"', "\\\""),
        title
    );
    let _ = std::process::Command::new("osascript").args(["-e", &script]).status();
}

/// Mark every passed deadline and put a banner up for each. Returns what fired.
fn fire_timers(now: DateTime<Local>) -> Vec<String> {
    let fired = command::check_timers(now).unwrap_or_default();
    for f in &fired {
        notify("t0", f);
    }
    fired
}

const HELP: &str = "t0                      the thread tree, keyboard driven
t0 status [thread]      what is current, as text
t0 cold [thread]        what stopped moving
t0 check                notify on passed deadlines
t0 chat <thread>[: title]
t0 chats <thread>
t0 resume <thread> <file>
t0 <command>            any line of the grammar below

A thread is a path (infra/aws), a folder name, or a title. Inside a thread's
folder, \".\" means that thread.";

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        return run_app();
    }
    let line = args.join(" ");
    let now = Local::now();
    let here = Here::from_cwd();
    store::ensure_root()?;

    let (verb, rest) = line.split_once(' ').unwrap_or((line.as_str(), ""));
    let (verb, rest) = (verb.trim(), rest.trim());

    let result = (|| -> Result<String> {
        match verb {
            "help" | "-h" | "--help" => Ok(format!("{HELP}\n\n{}", grammar::GRAMMAR.join("\n"))),
            // "status" is what is current; "cold" is what stopped moving
            "status" | "cold" => {
                let tree = store::read_tree()?;
                let roots: Vec<&Thread> = if rest.is_empty() {
                    tree.iter().collect()
                } else {
                    vec![reference::resolve(&tree, rest, &here)?]
                };
                let config = config::load();
                let after = Some(config.cools_after);
                let show_cold = verb == "cold";
                let text = ui::status_text(now, &roots, after, show_cold);
                if show_cold {
                    return Ok(text);
                }
                let cold =
                    roots.iter().flat_map(|r| r.walk()).filter(|t| t.is_cold(now, config.cools_after)).count();
                Ok(match cold {
                    0 => text,
                    n => format!("{text}\n\n{n} gone cold, t0 cold to see {}", if n == 1 { "it" } else { "them" }),
                })
            }
            "check" if rest.is_empty() => {
                let fired = fire_timers(now);
                Ok(if fired.is_empty() { "nothing newly due".into() } else { fired.join("\n") })
            }
            "chats" if !rest.is_empty() => {
                let tree = store::read_tree()?;
                let t = reference::resolve(&tree, rest, &here)?;
                if t.chats.is_empty() {
                    return Ok(format!("no chats in {}", t.path));
                }
                Ok(t.chats
                    .iter()
                    .map(|c| format!("{}  {}  ({})", c.file, c.title, c.session))
                    .collect::<Vec<_>>()
                    .join("\n"))
            }
            "chat" if !rest.is_empty() => {
                let (thread, title) = match rest.split_once(':') {
                    Some((r, t)) => (r.trim(), t.trim()),
                    None => (rest, ""),
                };
                let cmd = Cmd::Chat { thread: thread.into(), title: title.into() };
                let outcome = command::apply_cmd(cmd, now, &here)?;
                let Effect::Chat { path, file, session, .. } = outcome.effect else {
                    return Ok(outcome.message);
                };
                if let Err(e) = run_chat(&path, &chat::start_args(&session)) {
                    // no session ever existed, so the file should not linger
                    let _ = store::remove_chat(&path, &file);
                    return Err(e);
                }
                Ok(outcome.message)
            }
            "resume" if !rest.is_empty() => {
                let (thread, file) = rest.rsplit_once(' ').unwrap_or((rest, ""));
                let tree = store::read_tree()?;
                let t = reference::resolve(&tree, thread.trim(), &here)?;
                let c = chat::find(t, file.trim())?;
                run_chat(&t.path, &chat::resume_args(&c.session))?;
                Ok(format!("resumed {}", c.title))
            }
            _ => command::apply(&line, now, &here).map(|o| o.message),
        }
    })();

    match result {
        Ok(message) => {
            println!("{message}");
            Ok(())
        }
        Err(e) => {
            eprintln!("{e:#}");
            std::process::exit(1);
        }
    }
}

/// Hand the terminal to Claude Code, take it back when it exits.
fn run_chat(path: &str, args: &[String]) -> Result<()> {
    ran(chat::command(path, args).status(), "claude")
}

/// Open a file in the user's editor. $VISUAL and $EDITOR may carry flags ("code -w"),
/// so the first word is the program and the rest are arguments.
fn run_editor(path: &std::path::Path) -> Result<()> {
    let editor = std::env::var("VISUAL")
        .or_else(|_| std::env::var("EDITOR"))
        .unwrap_or_else(|_| "vi".to_string());
    let mut words = editor.split_whitespace();
    let Some(program) = words.next() else { anyhow::bail!("EDITOR is set but empty") };
    ran(std::process::Command::new(program).args(words).arg(path).status(), program)
}

fn ran(status: std::io::Result<std::process::ExitStatus>, program: &str) -> Result<()> {
    match status {
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            anyhow::bail!("{program} is not on your PATH")
        }
        Err(e) => Err(e.into()),
    }
}

/// The terminal settings t0 was handed, so it can give exactly those back even if
/// a chat process exited badly and left the tty in some other state.
fn saved_tty() -> Option<libc::termios> {
    unsafe {
        let mut t: libc::termios = std::mem::zeroed();
        if libc::tcgetattr(libc::STDIN_FILENO, &mut t) == 0 {
            Some(t)
        } else {
            None
        }
    }
}

fn restore_tty(saved: &Option<libc::termios>) {
    if let Some(t) = saved {
        unsafe {
            libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, t);
        }
    }
}

fn enter() -> Result<ratatui::Terminal<ratatui::backend::CrosstermBackend<Stdout>>> {
    enable_raw_mode()?;
    stdout().execute(EnterAlternateScreen)?;
    Ok(ratatui::Terminal::new(ratatui::backend::CrosstermBackend::new(stdout()))?)
}

fn leave() -> Result<()> {
    disable_raw_mode()?;
    stdout().execute(LeaveAlternateScreen)?;
    Ok(())
}

fn run_app() -> Result<()> {
    let mut app = App::new()?;
    let saved = saved_tty();

    // a panic must not leave the caller staring at a raw alternate screen
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = leave();
        restore_tty(&saved);
        previous(info);
    }));

    let mut terminal = enter()?;
    let result = loop_app(&mut terminal, &mut app);
    let _ = leave();
    restore_tty(&saved);
    app.save_state();
    result
}

/// Give the terminal to another program and take it back when it exits, whatever
/// state that program left it in.
fn handover(
    terminal: &mut ratatui::Terminal<ratatui::backend::CrosstermBackend<Stdout>>,
    app: &mut App,
    banner: &str,
    run: impl FnOnce() -> Result<()>,
) -> Result<Result<()>> {
    app.save_state();
    leave()?;
    println!("{banner}");
    let outcome = run();
    *terminal = enter()?;
    terminal.clear()?;
    Ok(outcome)
}

fn loop_app(
    terminal: &mut ratatui::Terminal<ratatui::backend::CrosstermBackend<Stdout>>,
    app: &mut App,
) -> Result<()> {
    let mut last_check = Instant::now();
    loop {
        terminal.draw(|f| ui::draw(f, app))?;

        if event::poll(StdDuration::from_millis(250))? {
            if let Event::Key(k) = event::read()? {
                if k.kind != KeyEventKind::Press {
                    continue;
                }
                match app.key(k) {
                    Action::Quit => return Ok(()),
                    Action::RunChat { path, args, label, created } => {
                        let banner = format!("t0 · {label} · {}", store::dir_of(&path).display());
                        match handover(terminal, app, &banner, || run_chat(&path, &args))? {
                            Ok(()) => app.note = Some((true, format!("back from {label}"))),
                            Err(e) => {
                                if let Some(file) = created {
                                    let _ = store::remove_chat(&path, &file);
                                }
                                app.note = Some((false, format!("{e:#}")));
                            }
                        }
                        app.poll_disk();
                    }
                    Action::EditConfig => {
                        let file = config::path();
                        let banner = format!("t0 · config · {}", file.display());
                        match handover(terminal, app, &banner, || run_editor(&file))? {
                            // poll_disk sees the file changed and reloads the colours
                            Ok(()) => app.note = Some((true, "config saved".into())),
                            Err(e) => app.note = Some((false, format!("{e:#}"))),
                        }
                        app.poll_disk();
                    }
                    Action::None => {}
                }
            }
        }

        app.now = Local::now();
        app.poll_disk();
        if last_check.elapsed() > StdDuration::from_secs(60) {
            last_check = Instant::now();
            let fired = fire_timers(app.now);
            if !fired.is_empty() {
                app.note = Some((false, fired.join(" · ")));
                app.poll_disk();
            }
        }
    }
}
