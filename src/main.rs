use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyCode},
    terminal::{disable_raw_mode, enable_raw_mode, size},
};
use std::{
    io::{self, Write},
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

mod explainshell;
mod snoop;


// ── Terminal cleanup ──────────────────────────────────────────────────────────

struct TerminalRestorer;
impl Drop for TerminalRestorer {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = write!(io::stdout(), "\x1b[?25h\x1b[0m");
        let _ = io::stdout().flush();
    }
}


fn highlight_cmd_source_by_index(cmd: &str, start: usize, end: usize) -> String {
    let chars: Vec<char> = cmd.chars().collect();
    if start < end && end <= chars.len() {
        let before: String = chars[..start].iter().collect();
        let matched: String = chars[start..end].iter().collect();
        let after: String = chars[end..].iter().collect();
        format!("{}\x1b[31;1;7m{}\x1b[27;0;7m{}", before, matched, after)
    } else {
        cmd.to_string()
    }
}

fn highlight_cmd_source(cmd: &str, source: &str) -> String {
    let clean_source = if let Some(idx) = source.find('(') {
        &source[..idx]
    } else {
        source
    };

    if let Some(pos) = cmd.find(clean_source) {
        let before = &cmd[..pos];
        let matched = &cmd[pos..pos + clean_source.len()];
        let after = &cmd[pos + clean_source.len()..];
        format!("{}\x1b[31;1;7m{}\x1b[27;0;7m{}", before, matched, after)
    } else {
        cmd.to_string()
    }
}

// ── Watch mode (right-pane TUI) ───────────────────────────────────────────────

fn render_watch(
    cmds: &[String],
    selected: usize,
    scroll: usize,
    explanation: &explainshell::Explanation,
    active_match_idx: Option<usize>,
    cols: u16,
    rows: u16,
    stdout: &mut io::Stdout,
) -> Result<()> {
    let mut out = String::with_capacity(8192);
    out.push_str("\x1b[H\x1b[2J\x1b[?25l");

    let header_height = 1;
    let separator_height = 1;
    
    // Calculate list height (40% of rows, min 5, max rows - 6)
    let total_rows = rows as usize;
    let list_height = (total_rows * 40 / 100).max(5).min(total_rows.saturating_sub(6));
    let explanation_height = total_rows.saturating_sub(list_height + header_height + separator_height);

    // 1. Header
    let header = format!(" ◉ flw  {} cmds  (↑↓ scroll · ←→ inspect · q quit) ", cmds.len());
    let header_line: String = header
        .chars()
        .chain(std::iter::repeat(' '))
        .take(cols as usize)
        .collect();
    out.push_str("\x1b[7m");
    out.push_str(&header_line);
    out.push_str("\x1b[0m\r\n");

    // 2. Command List
    for i in 0..list_height {
        let idx = scroll + i;
        if idx < cmds.len() {
            let display_cmd = if idx == selected {
                explanation.formatted_command.as_deref().unwrap_or(&cmds[idx])
            } else {
                &cmds[idx]
            };

            if idx == selected {
                if let Some(match_idx) = active_match_idx {
                    if match_idx < explanation.matches.len() {
                        let m = &explanation.matches[match_idx];
                        let display_text = if explanation.formatted_command.is_some() {
                            highlight_cmd_source_by_index(display_cmd, m.start, m.end)
                        } else {
                            highlight_cmd_source(display_cmd, &m.source)
                        };
                        let raw_len = display_cmd.chars().count();
                        let pad_len = (cols as usize).saturating_sub(raw_len);
                        let line = format!("\x1b[7m{}{}\x1b[0m", display_text, " ".repeat(pad_len));
                        out.push_str(&line);
                        out.push_str("\r\n");
                        continue;
                    }
                }
                
                let line: String = display_cmd
                    .chars()
                    .chain(std::iter::repeat(' '))
                    .take(cols as usize)
                    .collect();
                out.push_str("\x1b[7m");
                out.push_str(&line);
                out.push_str("\x1b[0m");
            } else {
                let line: String = display_cmd
                    .chars()
                    .chain(std::iter::repeat(' '))
                    .take(cols as usize)
                    .collect();
                out.push_str(&line);
            }
        } else {
            out.push_str(&" ".repeat(cols as usize));
        }
        out.push_str("\r\n");
    }

    // 3. Separator
    let sep_title = " 🧭 ExplainShell JSON ";
    let sep_line: String = sep_title
        .chars()
        .chain(std::iter::repeat('─'))
        .take(cols as usize)
        .collect();
    out.push_str("\x1b[36m"); // Cyan color for separator
    out.push_str(&sep_line);
    out.push_str("\x1b[0m\r\n");

    // 4. Explanation (JSON)
    let json_str = explainshell::explanation_to_json(explanation, active_match_idx);
    let exp_lines: Vec<&str> = json_str.lines().collect();
    for i in 0..explanation_height {
        if i < exp_lines.len() {
            let line = exp_lines[i];
            
            // Calculate visual length (excluding ANSI escape sequences)
            let mut visual_len = 0;
            let mut in_escape = false;
            let mut char_count_including_escapes = 0;
            
            for c in line.chars() {
                if c == '\x1b' {
                    in_escape = true;
                }
                
                if in_escape {
                    char_count_including_escapes += 1;
                    if c == 'm' || c == 'K' || c == 'H' || c == 'J' {
                        in_escape = false;
                    }
                } else {
                    visual_len += 1;
                    char_count_including_escapes += 1;
                    if visual_len >= cols as usize {
                        break;
                    }
                }
            }
            
            let truncated: String = line.chars().take(char_count_including_escapes).collect();
            out.push_str(&truncated);
            
            // Safe reset if line ended within red block
            if line.contains("\x1b[31;1m") && !truncated.contains("\x1b[0m") {
                out.push_str("\x1b[0m");
            }
            
            let extra = (cols as usize).saturating_sub(visual_len);
            if extra > 0 {
                out.push_str(&" ".repeat(extra));
            }
        } else {
            out.push_str(&" ".repeat(cols as usize));
        }
        if i < explanation_height - 1 {
            out.push_str("\r\n");
        }
    }

    out.push_str("\x1b[?25h");
    stdout.write_all(out.as_bytes())?;
    stdout.flush()?;
    Ok(())
}

fn watch_mode(trace_path: String) -> Result<()> {
    let commands: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    snoop::start_from_file(trace_path, Arc::clone(&commands));

    enable_raw_mode()?;
    let _restorer = TerminalRestorer;
    let mut stdout = io::stdout();

    let mut selected: usize = 0;
    let mut scroll: usize = 0;
    let mut follow = true;
    let mut active_match_idx: Option<usize> = None;

    // Caching explainshell Explanation results
    let cache = Arc::new(Mutex::new(std::collections::HashMap::<String, explainshell::Explanation>::new()));
    let mut last_selected = usize::MAX;
    let mut last_active_match_idx: Option<usize> = None;
    let mut last_cache_status: Option<String> = None;
    let mut last_count = usize::MAX;

    loop {
        let (cols, rows) = size()?;
        let visible_list_rows = {
            let total_rows = rows as usize;
            (total_rows * 40 / 100).max(5).min(total_rows.saturating_sub(6))
        };

        let mut should_redraw = false;
        let current_explanation: explainshell::Explanation;
        let current_cache_status: String;

        let count = {
            let cmds = commands.lock().unwrap();
            cmds.len()
        };

        if count != last_count {
            should_redraw = true;
            if follow && count > 0 {
                scroll = count.saturating_sub(visible_list_rows);
                selected = count - 1;
            }
            last_count = count;
        }

        let selected_cmd = {
            let cmds = commands.lock().unwrap();
            if selected < cmds.len() {
                Some(cmds[selected].clone())
            } else {
                None
            }
        };

        if let Some(cmd_text) = selected_cmd {
            let cached_opt = {
                let cache_lock = cache.lock().unwrap();
                cache_lock.get(&cmd_text).cloned()
            };

            match cached_opt {
                Some(cached) => {
                    if let Some(err) = &cached.error {
                        current_cache_status = format!("error: {}", err);
                    } else {
                        current_cache_status = format!("loaded: {}", cached.matches.len());
                    }
                    current_explanation = cached;
                }
                None => {
                    let loading_exp = explainshell::Explanation {
                        command: cmd_text.clone(),
                        error: Some("Loading explanation...".to_string()),
                        formatted_command: None,
                        matches: Vec::new(),
                    };
                    {
                        let mut cache_lock = cache.lock().unwrap();
                        cache_lock.insert(cmd_text.clone(), loading_exp.clone());
                    }
                    current_explanation = loading_exp;
                    current_cache_status = "loading".to_string();
                    should_redraw = true;

                    let cache_clone = Arc::clone(&cache);
                    let cmd_to_fetch = cmd_text.clone();
                    std::thread::spawn(move || {
                        let result = match explainshell::fetch_html(&cmd_to_fetch) {
                            Ok(html) => explainshell::parse_html(&cmd_to_fetch, &html),
                            Err(e) => explainshell::Explanation {
                                command: cmd_to_fetch.clone(),
                                error: Some(format!("Error: {}", e)),
                                formatted_command: None,
                                matches: Vec::new(),
                            },
                        };
                        cache_clone.lock().unwrap().insert(cmd_to_fetch, result);
                    });
                }
            }
        } else {
            let empty_exp = explainshell::Explanation {
                command: String::new(),
                error: Some("No commands recorded yet".to_string()),
                formatted_command: None,
                matches: Vec::new(),
            };
            current_explanation = empty_exp;
            current_cache_status = "empty".to_string();
        }

        if selected != last_selected 
            || active_match_idx != last_active_match_idx 
            || Some(&current_cache_status) != last_cache_status.as_ref() 
            || should_redraw 
        {
            let cmds = commands.lock().unwrap();
            render_watch(&cmds, selected, scroll, &current_explanation, active_match_idx, cols, rows, &mut stdout)?;
            last_selected = selected;
            last_active_match_idx = active_match_idx;
            last_cache_status = Some(current_cache_status);
        }

        if event::poll(Duration::from_millis(50))? {
            match event::read()? {
                Event::Key(key) => match key.code {
                    KeyCode::Up | KeyCode::Char('k') => {
                        active_match_idx = None;
                        follow = false;
                        selected = selected.saturating_sub(1);
                        scroll = scroll.min(selected);
                    }
                    KeyCode::Down | KeyCode::Char('j') => {
                        active_match_idx = None;
                        let count = commands.lock().unwrap().len();
                        if selected + 1 < count {
                            selected += 1;
                        }
                        if selected >= scroll + visible_list_rows {
                            scroll = selected + 1 - visible_list_rows;
                        }
                        follow = selected + 1 == count;
                    }
                    KeyCode::Left | KeyCode::Char('h') => {
                        if let Some(m_idx) = active_match_idx {
                            if m_idx > 0 {
                                active_match_idx = Some(m_idx - 1);
                            } else {
                                active_match_idx = None;
                            }
                        }
                    }
                    KeyCode::Right | KeyCode::Char('l') => {
                        if active_match_idx.is_none() {
                            if !current_explanation.matches.is_empty() {
                                active_match_idx = Some(0);
                            }
                        } else if let Some(m_idx) = active_match_idx {
                            if m_idx + 1 < current_explanation.matches.len() {
                                active_match_idx = Some(m_idx + 1);
                            }
                        }
                    }
                    KeyCode::Esc => {
                        if active_match_idx.is_some() {
                            active_match_idx = None;
                        } else {
                            break;
                        }
                    }
                    KeyCode::Char('q') => break,
                    _ => {}
                },
                Event::Resize(c, r) => {
                    let cmds = commands.lock().unwrap();
                    render_watch(&cmds, selected, scroll, &current_explanation, active_match_idx, c, r, &mut stdout)?;
                    last_count = cmds.len();
                }
                _ => {}
            }
        }
    }

    write!(stdout, "\x1b[2J\x1b[H")?;
    stdout.flush()?;
    Ok(())
}


// ── Launch mode ───────────────────────────────────────────────────────────────

/// Single-quote a shell argument safely.
fn sq(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Find the real path of a shell binary, ignoring our wrapper dir.
/// Looks in /bin, /usr/bin only.
fn real_shell(name: &str) -> PathBuf {
    for dir in ["/bin", "/usr/bin", "/usr/local/bin"] {
        let p = PathBuf::from(dir).join(name);
        if p.exists() {
            return p;
        }
    }
    PathBuf::from(format!("/bin/{}", name))
}

/// Write a tiny wrapper script that logs the -c argument then exec's the real shell.
fn write_wrapper(path: PathBuf, real: &PathBuf, trace: &PathBuf) -> Result<()> {
    // Using printf instead of echo to avoid issues with special chars.
    // The wrapper is deliberately tiny — no bashisms, pure POSIX sh.
    let script = format!(
        "#!/bin/sh\nif [ \"$1\" = \"-c\" ] && [ -n \"$2\" ]; then printf '%s\\n' \"$2\" >> {} 2>/dev/null; fi\nexec {} \"$@\"\n",
        sq(&trace.to_string_lossy()),
        sq(&real.to_string_lossy()),
    );
    std::fs::write(&path, &script)?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))?;
    Ok(())
}

fn launch_mode(agent_name: String, extra_args: Vec<String>) -> Result<()> {
    let agent_path = which::which(&agent_name)
        .unwrap_or_else(|_| panic!("'{}' not found in PATH", agent_name));

    let pid = std::process::id();
    let wrapper_dir = PathBuf::from(format!("/tmp/flw_{}_bin", pid));
    let trace_path = PathBuf::from(format!("/tmp/flw_{}.trace", pid));
    let flw_bin = std::env::current_exe()?;
    let in_tmux = std::env::var("TMUX").is_ok();
    let window_name = format!("flw-{}", agent_name);

    // Create bash and sh wrappers that log -c commands to the trace file
    std::fs::create_dir_all(&wrapper_dir)?;
    write_wrapper(wrapper_dir.join("bash"), &real_shell("bash"), &trace_path)?;
    write_wrapper(wrapper_dir.join("sh"), &real_shell("sh"), &trace_path)?;

    // Left pane: agent runs normally, just with our wrapper dir prepended to PATH
    let extra: String = extra_args.iter().map(|a| format!(" {}", sq(a))).collect();


    // Right pane: flw --watch <trace>
    let right_cmd = format!(
        "{} --watch {}",
        sq(&flw_bin.to_string_lossy()),
        sq(&trace_path.to_string_lossy()),
    );

    // Ctrl+F toggle using tmux's native if-shell command.
    // if-shell runs a shell condition, then dispatches one of two tmux commands.
    // Unlike run-shell, it produces no output in the pane.
    let open_pane = format!("split-window -h {}", sq(&right_cmd));
    std::process::Command::new("tmux")
        .args([
            "bind-key", "-n", "C-f",
            "if-shell", "[ $(tmux list-panes | wc -l) -eq 1 ]",
            &open_pane,
            "kill-pane -t :.1",
        ])
        .status()?;

    // Also clean up the C-f binding when agi exits
    let cleanup = format!(
        "; tmux unbind-key -n C-f 2>/dev/null; rm -rf {} {}",
        sq(&wrapper_dir.to_string_lossy()),
        sq(&trace_path.to_string_lossy()),
    );
    let left_cmd = format!(
        "PATH={}:$PATH {}{}{}",
        sq(&wrapper_dir.to_string_lossy()),
        sq(&agent_path.to_string_lossy()),
        extra,
        cleanup,
    );

    if in_tmux {
        // Create a new window in the existing session (no nesting)
        std::process::Command::new("tmux")
            .args(["new-window", "-n", &window_name, &left_cmd])
            .status()?;
        // Focus is already on the new window; flw exits here
    } else {
        // Not in tmux — create a session and attach
        let session = format!("flw_{}", pid);
        std::process::Command::new("tmux")
            .args(["new-session", "-d", "-s", &session, &left_cmd])
            .status()?;
        std::process::Command::new("tmux")
            .args(["select-pane", "-t", &format!("{}:0.0", session)])
            .status()?;
        std::process::Command::new("tmux")
            .args(["attach-session", "-t", &session])
            .status()?;
    }

    Ok(())
}

// ── Entry point ───────────────────────────────────────────────────────────────

fn main() -> Result<()> {
    let argv: Vec<String> = std::env::args().skip(1).collect();

    if argv.first().map(String::as_str) == Some("--watch") {
        let trace = argv.get(1).cloned().expect("flw --watch <trace_file>");
        return watch_mode(trace);
    }

    let agent_name = argv
        .first()
        .cloned()
        .expect("Usage: flw <agent>  e.g.  flw agi");
    let extra_args = argv[1..].to_vec();

    launch_mode(agent_name, extra_args)
}
