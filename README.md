# t0

A terminal outliner for the open loops in your work. Every thread is a folder under `~/.t0`, sub-threads are sub-folders, as deep as you like. Each folder holds a plain-text log and the Claude Code chats you started from it.

```
~/.t0/
  CLAUDE.md                 what Claude reads when a chat starts in any thread
  infra/
    thread.md
    aws/
      thread.md             # title, one event per line, then free notes
      chats/
        20260908-064356-draft-the-quota-request.md
      io2-migration/
        thread.md
```

## Build

Needs a Rust toolchain. Chats need the `claude` CLI on your PATH, and deadline notifications are macOS only.

```
cargo install --path .      puts `t0` in ~/.cargo/bin
t0                          the tree
```

## Keys

| Key | Does |
|---|---|
| `j k` or `↓ ↑` | move |
| `l h` or `→ ←` | open or close a thread, or step in and out |
| `space` | toggle open |
| `enter` | edit the title or the event under the cursor, or resume a chat |
| `n` / `N` | new thread beside this one / inside it |
| `a` | add an event, end with `, wait 1d` to start waiting |
| `w` | wait for a duration, or with no deadline |
| `d` | mark done |
| `c` | new Claude Code chat in this thread's folder |
| `tab` / `shift+tab` | move under the thread above / out one level |
| `J` / `K` | move down or up among its siblings, also `shift+↓ ↑` |
| `v` / `V` | next or previous view |
| `x` | archive a thread; on an event or a chat, delete it |
| `X` | delete a thread into the trash, after typing `delete` |
| `u` | undo the last command, then the one before |
| `A` / `T` | the archive / the trash |
| `/` | filter |
| `:` | type a command |
| `g` / `G` | first / last |
| `?` | the key list and the grammar, `j k` to scroll |
| `q` | quit |

The app opens on its wordmark, which resolves out of static and rolls up into the header in under two seconds. The tree is there underneath from the first frame, and any key cuts the banner short.

Typing happens on one line: prompts open at the bottom, the filter at the top. `ctrl+w` and `ctrl+u` rub out. A command that fails keeps what you typed so you can fix it. The box beside a title is empty until the thread is closed, half-filled once you have done something about it, and filled when it is done. Its colour is white for your move, amber while you are on it, blue while waiting, red past a deadline, grey when closed. A folder takes the loudest state of anything inside it, so a parent cannot look calm while a sub-thread is overdue.

## One grammar, two doors

The keys and the CLI build the same lines, so anything you can do in the app you can do from a shell, a script, or a Claude Code chat:

```
new <title>[: <problem>[, wait [1d]]]
new <parent>/<title>[: <problem>]
<thread>: <text>[, wait [1d]]
<thread> wait [2h]
done <thread>[: <text>]
edit <thread> #3: <text>
rename <thread>: <title>
move <thread> under <parent> | top | up | down
<thread> wait +1d | -2h
archive <thread>
delete <thread> | delete <thread> #3
restore <thread> | purge <thread>
```

Threads sit oldest first until you move one with `J` or `K`, after which that level keeps the order you gave it. The position is stored in an HTML comment in `thread.md`, invisible when the file is rendered.

A thread is a path like `infra/aws`, a folder name, or a title when it is unique, and path segments may be titles. Inside a thread's folder, `.` means that thread and `./x` means one inside it. `w` on a thread that is already waiting takes `+2h` to push the deadline out or `-30m` to pull it in, and will not pull it back before the wait began. Durations read as `30m`, `2h`, `1 day`, `2 weeks`, up to ten years. A bare `wait` waits with no deadline and never fires. Text that merely ends in "wait for their reply" stays text; a `, wait` clause only becomes a deadline when what follows is a duration. `#3` is the event's place in the log. Any event on a closed thread reopens it. Archiving or deleting a thread takes its folder and everything under it along.

Other commands: `t0 status [thread]`, `t0 check`, `t0 chat <thread>[: title]`, `t0 chats <thread>`, `t0 resume <thread> <file>`, `t0 help`.

## Views

`v` cycles what the tree shows: the current page, only what is open, only your move, only what you have started, only what you are waiting on, only what is overdue, only what is closed, and what has gone cold. `V` goes back. The view name and the counts in the header always describe what is actually on screen. `/` filters on top of whichever view is on, and a filter reaches what has gone cold too.

A thread that has not moved in three weeks, and has nothing moving under it, goes cold and leaves the current view. Nothing is deleted and nothing asks you anything: a page you have stopped writing on turns itself. `cools_after` in the config sets the span, and `t0 cold` prints them. `m` swipes a highlighter over the line under the cursor, again for the next pen, once more to take it off; it keeps as `mark=` in the file.

## The archive, the trash, and undo

Nothing leaves `~/.t0` until you purge it. `x` moves a thread and everything inside it into `.archive`: finished with, kept to look back on. It does not ask, because `u` takes it back. `X` moves it into `.trash` instead, and makes you type `delete` first, because a wrong `x` costs one keypress and a wrong `X` should not happen at all. `A` opens the archive and `T` the trash, newest first. In either, `enter` or `r` puts a thread back where it came from, and `X` purges it for good after you type `purge`. The same key returns to the tree. From a shell: `t0 "archive <thread>"`, `t0 "delete <thread>"`, `t0 "restore <thread>"`, `t0 "purge <thread>"`.

`u` takes the last command back, then the one before, up to fifty deep. An event added, edited or deleted goes back to the old text; an archive, delete or restore goes back to where it was; a move goes back under its old parent; a new thread goes into the trash. A purge and a chat cannot be undone. Undo only touches what the command wrote: if a chat or the CLI has changed that file since, `u` says so and leaves it alone. The stack lives in memory, so it is empty when the app starts.

## Deadlines

`t0 check` marks every passed deadline once and posts a macOS notification. The app runs it every minute while it is open. For deadlines that fire when the app is closed, add a cron line:

```
*/5 * * * * ~/.cargo/bin/t0 check
```

## Chats

`c` writes `chats/<stamp>-<title>.md` with a fresh session id, then hands this terminal to `claude --session-id <id>` running in the thread's folder. Quit Claude and the tree comes back. `enter` on a chat row resumes it. Because Claude starts inside the folder, it sees that thread's log, its sub-threads, and the root `CLAUDE.md`, which explains the layout and how to append events with the CLI.

## Layout

- `store.rs` reads and writes the folders, the archive and the trash. `format.rs` is the `thread.md` format, hand-editable and forgiving: a line it cannot read stays as notes.
- `model.rs` holds the tree and the status rules. `duration.rs` parses and prints spans. `reference.rs` turns what you typed into the thread you meant.
- `grammar.rs` parses a line into a command. `command.rs` runs it, and records how to undo it.
- `app.rs` is the state, `rows.rs` flattens the tree into what is on screen, `keys.rs` maps each keypress, `ui.rs` draws, `intro.rs` is the banner. `input.rs` is the one text field.
- `config.rs` reads `~/.t0/config`. `chat.rs` writes chat files and builds the `claude` command. `main.rs` runs the loop and owns the terminal handoff.

`T0_ROOT` points the whole thing at another folder, which is how the test runs.
