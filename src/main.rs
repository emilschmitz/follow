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


fn highlight_cmd_source_by_index(cmd: &str, start: usize, end: usize, bg_style: &str) -> String {
    let chars: Vec<char> = cmd.chars().collect();
    if start < end && end <= chars.len() {
        let before: String = chars[..start].iter().collect();
        let matched: String = chars[start..end].iter().collect();
        let after: String = chars[end..].iter().collect();
        format!("{}\x1b[1;48;5;160;38;5;231m{}\x1b[0;{}m{}", before, matched, bg_style, after)
    } else {
        cmd.to_string()
    }
}

fn highlight_cmd_source(cmd: &str, source: &str, bg_style: &str) -> String {
    let clean_source = if let Some(idx) = source.find('(') {
        &source[..idx]
    } else {
        source
    };

    if let Some(pos) = cmd.find(clean_source) {
        let before = &cmd[..pos];
        let matched = &cmd[pos..pos + clean_source.len()];
        let after = &cmd[pos + clean_source.len()..];
        format!("{}\x1b[1;48;5;160;38;5;231m{}\x1b[0;{}m{}", before, matched, bg_style, after)
    } else {
        cmd.to_string()
    }
}

// ── Watch mode (right-pane TUI) ───────────────────────────────────────────────

fn render_list(
    cmds: &[snoop::Command],
    selected: usize,
    scroll: usize,
    explanation: &explainshell::Explanation,
    active_match_idx: Option<usize>,
    cols: u16,
    rows: u16,
    stdout: &mut io::Stdout,
) -> Result<()> {
    let mut out = String::with_capacity(8192);
    // Hide cursor, clear screen, clear scrollback
    out.push_str("\x1b[?25l\x1b[2J\x1b[3J");

    let header_height = 2;
    let list_height = (rows as usize).saturating_sub(header_height);

    // 1. Header (Line 1)
    let left_plain = format!("  🦅 FLW  •  {} commands", cmds.len());
    let right_plain = "↑↓ scroll  ←→ inspect  q exit  ";
    let left_len = left_plain.chars().count();
    let right_len = right_plain.chars().count();
    let pad_width = (cols as usize).saturating_sub(left_len + right_len);

    let left_styled = format!("  \x1b[38;5;203m🦅\x1b[0m \x1b[1;38;5;203;1mFLW\x1b[0m  \x1b[38;5;242m•\x1b[0m  \x1b[38;5;250m{} commands\x1b[0m", cmds.len());
    let right_styled = "\x1b[38;5;242m↑↓ scroll  ←→ inspect  q exit\x1b[0m  ";
    
    out.push_str("\x1b[1;1H"); // Move to row 1, col 1
    out.push_str(&left_styled);
    out.push_str(&" ".repeat(pad_width));
    out.push_str(&right_styled);

    // 1. Header (Line 2: Separator)
    out.push_str("\x1b[2;1H"); // Move to row 2, col 1
    let sep_chars = (cols as usize).saturating_sub(4);
    out.push_str(&format!("  \x1b[38;5;238m{}\x1b[0m", "─".repeat(sep_chars)));

    // 2. Command List
    for i in 0..list_height {
        let row_pos = i + 3;
        out.push_str(&format!("\x1b[{};1H", row_pos)); // Move to row_pos, col 1

        let idx = scroll + i;
        if idx < cmds.len() {
            let cmd_text = &cmds[idx].text;
            let display_cmd = if idx == selected {
                explanation.formatted_command.as_deref().unwrap_or(cmd_text)
            } else {
                cmd_text
            };

            if idx == selected {
                let avail_width = (cols as usize).saturating_sub(5);
                let truncated_cmd: String = display_cmd.chars().take(avail_width).collect();
                let bg_style = "48;5;236";
                
                let display_text = if let Some(match_idx) = active_match_idx {
                    if match_idx < explanation.matches.len() {
                        let m = &explanation.matches[match_idx];
                        let start = m.start.min(truncated_cmd.chars().count());
                        let end = m.end.min(truncated_cmd.chars().count());
                        if explanation.formatted_command.is_some() {
                            highlight_cmd_source_by_index(&truncated_cmd, start, end, bg_style)
                        } else {
                            highlight_cmd_source(&truncated_cmd, &m.source, bg_style)
                        }
                    } else {
                        truncated_cmd
                    }
                } else {
                    truncated_cmd
                };

                let status_indicator_sel = match cmds[idx].status {
                    Some(0) => "\x1b[38;5;119;1m●\x1b[0;48;5;236m",
                    Some(_) => "\x1b[38;5;203;1m●\x1b[0;48;5;236m",
                    None => "\x1b[38;5;75;1m●\x1b[0;48;5;236m",
                };

                let left_indicator = if active_match_idx.is_some() {
                    " "
                } else {
                    "\x1b[38;5;203;1m▎\x1b[0;\x1b[48;5;236m"
                };

                let line = format!(
                    "\x1b[48;5;236m{} {} {}\x1b[K\x1b[0m",
                    left_indicator,
                    status_indicator_sel,
                    display_text
                );
                out.push_str(&line);
            } else {
                let avail_width = (cols as usize).saturating_sub(5);
                let truncated_cmd: String = display_cmd.chars().take(avail_width).collect();
                
                let status_indicator_unsel = match cmds[idx].status {
                    Some(0) => "\x1b[38;5;119m●\x1b[0m",
                    Some(_) => "\x1b[38;5;203m●\x1b[0m",
                    None => "\x1b[38;5;244m●\x1b[0m",
                };

                let line = format!(
                    "  {} \x1b[38;5;250m{}\x1b[K\x1b[0m",
                    status_indicator_unsel,
                    truncated_cmd
                );
                out.push_str(&line);
            }
        } else {
            out.push_str("\x1b[K");
        }
    }

    out.push_str("\x1b[?25h");
    stdout.write_all(out.as_bytes())?;
    stdout.flush()?;
    Ok(())
}

fn strip_shopt_wrapper(cmd: &str) -> String {
    // We are looking to strip the prefix:
    // shopt -u promptvars nullglob extglob nocaseglob dotglob; ( 
    // And potentially trailing background boilerplate like:
    // )
    // __code=$?; pgrep -g 0 ...
    
    let mut cleaned = cmd.trim();
    
    let prefix = "shopt -u promptvars nullglob extglob nocaseglob dotglob; (";
    if cleaned.starts_with(prefix) {
        cleaned = &cleaned[prefix.len()..];
        cleaned = cleaned.trim_start();
        
        // Now try to strip the closing parenthesis and everything after it.
        // We look for a line or section starting with `) \n __code=$?` or just `)` at the end
        if let Some(idx) = cleaned.rfind(')') {
            // Check if what follows the ')' is mostly just the pgrep exit code tracking noise
            let after_paren = cleaned[idx + 1..].trim();
            if after_paren.is_empty() || after_paren.starts_with("__code=$?") || after_paren.starts_with("; __code=$?") || after_paren.starts_with("\n__code=$?") {
                cleaned = &cleaned[..idx];
                cleaned = cleaned.trim_end();
            }
        }
    }
    
    cleaned.to_string()
}

fn watch_list_mode(trace_path: String, json_path: String) -> Result<()> {
    let _ = std::fs::write(&json_path, ""); // Ensure file exists to reset missing_count
    
    let commands: Arc<Mutex<Vec<snoop::Command>>> = Arc::new(Mutex::new(Vec::new()));
    snoop::start_from_file(trace_path, Arc::clone(&commands));

    enable_raw_mode()?;
    let _restorer = TerminalRestorer;
    let mut stdout = io::stdout();
    write!(stdout, "\x1b[?1049h")?; // Enter alternate screen
    stdout.flush()?;

    // Allow tmux window layout/resize to settle before first render query
    std::thread::sleep(Duration::from_millis(100));
    write!(stdout, "\x1b[H\x1b[2J\x1b[3J")?;
    stdout.flush()?;

    let mut selected: usize = 0;
    let mut scroll: usize = 0;
    let mut follow = true;
    let mut active_match_idx: Option<usize> = None;
    let mut expanded_match_idx: Option<usize> = None;
    let mut expanded_scroll: usize = 0;
    let mut force_redraw = false;

    // Caching explainshell Explanation results
    let cache = Arc::new(Mutex::new(std::collections::HashMap::<String, explainshell::Explanation>::new()));
    let mut last_selected = usize::MAX;
    let mut last_active_match_idx: Option<usize> = None;
    let mut last_expanded_match_idx: Option<usize> = None;
    let mut last_expanded_scroll = usize::MAX;
    let mut last_cache_status: Option<String> = None;
    let mut last_count = usize::MAX;

    loop {
        let (cols, rows) = size()?;
        let visible_list_rows = (rows as usize).saturating_sub(2);

        let count = {
            let cmds = commands.lock().unwrap();
            cmds.len()
        };

        // Keep selected visible within scroll margins
        if count > 0 && visible_list_rows > 2 {
            let top_margin = 1;
            let bottom_margin = 2;
            if selected.saturating_sub(top_margin) < scroll {
                scroll = selected.saturating_sub(top_margin);
            }
            if selected + bottom_margin >= scroll + visible_list_rows {
                scroll = (selected + bottom_margin).saturating_sub(visible_list_rows);
            }
        }

        // Clamp expanded_scroll using the .max_scroll file written by watch_json_mode
        if expanded_match_idx.is_some() {
            let max_scroll_path = json_path.replace(".json", ".max_scroll");
            if let Ok(s) = std::fs::read_to_string(&max_scroll_path) {
                if let Ok(max_scroll) = s.trim().parse::<usize>() {
                    if expanded_scroll > max_scroll {
                        expanded_scroll = max_scroll;
                    }
                }
            }
        }

        let mut should_redraw = force_redraw;
        force_redraw = false;
        let current_explanation: explainshell::Explanation;
        let current_cache_status: String;

        if count != last_count {
            should_redraw = true;
            if follow && count > 0 && expanded_match_idx.is_none() {
                scroll = count.saturating_sub(visible_list_rows);
                selected = count - 1;
            }
            last_count = count;
        }

        let selected_cmd = {
            let cmds = commands.lock().unwrap();
            if selected < cmds.len() {
                Some(cmds[selected].text.clone())
            } else {
                None
            }
        };

        let selected_status = {
            let cmds = commands.lock().unwrap();
            if selected < cmds.len() {
                cmds[selected].status
            } else {
                None
            }
        };

        let current_status_str = selected_status.map_or("running".to_string(), |s| s.to_string());

        if let Some(cmd_text) = selected_cmd {
            let cmd_to_fetch = strip_shopt_wrapper(&cmd_text);
            let cached_opt = {
                let cache_lock = cache.lock().unwrap();
                cache_lock.get(&cmd_to_fetch).cloned()
            };

            match cached_opt {
                Some(cached) => {
                    if let Some(err) = &cached.error {
                        current_cache_status = format!("error: {} ({})", err, current_status_str);
                    } else {
                        current_cache_status = format!("loaded: {} ({})", cached.matches.len(), current_status_str);
                    }
                    current_explanation = cached;
                }
                None => {
                    let loading_exp = explainshell::Explanation {
                        command: cmd_to_fetch.clone(),
                        error: Some("Loading explanation...".to_string()),
                        formatted_command: None,
                        matches: Vec::new(),
                    };
                    {
                        let mut cache_lock = cache.lock().unwrap();
                        cache_lock.insert(cmd_to_fetch.clone(), loading_exp.clone());
                    }
                    current_explanation = loading_exp;
                    current_cache_status = format!("loading ({})", current_status_str);
                    should_redraw = true;

                    let cache_clone = Arc::clone(&cache);
                    let fetch_cmd = cmd_to_fetch.clone();
                    std::thread::spawn(move || {
                        let result = match explainshell::fetch_html(&fetch_cmd) {
                            Ok(html) => explainshell::parse_html(&fetch_cmd, &html),
                            Err(e) => explainshell::Explanation {
                                command: fetch_cmd.clone(),
                                error: Some(format!("Error: {}", e)),
                                formatted_command: None,
                                matches: Vec::new(),
                            },
                        };
                        cache_clone.lock().unwrap().insert(fetch_cmd, result);
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
            || expanded_match_idx != last_expanded_match_idx
            || expanded_scroll != last_expanded_scroll
            || Some(&current_cache_status) != last_cache_status.as_ref() 
            || should_redraw 
        {
            let cmds = commands.lock().unwrap();
            
            // Generate JSON and write to file for the bottom pane
            let active_status = cmds.get(selected).and_then(|c| c.status);
            let active_pid = cmds.get(selected).map(|c| c.id);
            let json_str = explainshell::explanation_to_json(
                &current_explanation, 
                active_match_idx, 
                expanded_match_idx, 
                expanded_scroll,
                active_status, 
                active_pid
            );
            
            let temp_json_path = format!("{}.tmp", json_path);
            let write_res = std::fs::write(&temp_json_path, &json_str)
                .and_then(|_| std::fs::rename(&temp_json_path, &json_path));
            if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open("/tmp/flw_debug.log") {
                let _ = writeln!(f, "Redraw triggered. active_match_idx: {:?}, expanded_match_idx: {:?}, write_success: {}", active_match_idx, expanded_match_idx, write_res.is_ok());
            }

            render_list(&cmds, selected, scroll, &current_explanation, active_match_idx, cols, rows, &mut stdout)?;
            last_selected = selected;
            last_active_match_idx = active_match_idx;
            last_expanded_match_idx = expanded_match_idx;
            last_expanded_scroll = expanded_scroll;
            last_cache_status = Some(current_cache_status);
        }

        if event::poll(Duration::from_millis(50))? {
            match event::read()? {
                Event::Key(key) => {
                    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open("/tmp/flw_debug.log") {
                        let _ = writeln!(f, "Key pressed: {:?}", key.code);
                    }
                    match key.code {
                    KeyCode::Up | KeyCode::Char('k') => {
                        if expanded_match_idx.is_some() {
                            expanded_scroll = expanded_scroll.saturating_sub(1);
                        } else {
                            active_match_idx = None;
                            expanded_match_idx = None;
                            follow = false;
                            selected = selected.saturating_sub(1);
                            
                            // Scroll up margin check: keep selected at least at scroll + 1
                            let margin = 1;
                            if selected.saturating_sub(margin) < scroll {
                                scroll = selected.saturating_sub(margin);
                            }
                        }
                    }
                    KeyCode::Down | KeyCode::Char('j') => {
                        if expanded_match_idx.is_some() {
                            let max_scroll_path = json_path.replace(".json", ".max_scroll");
                            let max_scroll = std::fs::read_to_string(&max_scroll_path)
                                .ok()
                                .and_then(|s| s.trim().parse::<usize>().ok())
                                .unwrap_or(usize::MAX);
                            if expanded_scroll < max_scroll {
                                expanded_scroll += 1;
                            }
                        } else {
                            active_match_idx = None;
                            expanded_match_idx = None;
                            let count = commands.lock().unwrap().len();
                            if selected + 1 < count {
                                selected += 1;
                            }
                            
                            // Scroll down margin check: keep selected at most at scroll + visible_list_rows - 2
                            let margin = 2; // Selected row must be <= scroll + visible_list_rows - margin
                            if selected + margin >= scroll + visible_list_rows {
                                scroll = (selected + margin).saturating_sub(visible_list_rows);
                            }
                            follow = selected + 1 == count;
                        }
                    }
                    KeyCode::Left | KeyCode::Char('h') => {
                        if expanded_match_idx.is_none() {
                            if let Some(m_idx) = active_match_idx {
                                if m_idx > 0 {
                                    active_match_idx = Some(m_idx - 1);
                                    expanded_match_idx = None;
                                } else {
                                    active_match_idx = None;
                                    expanded_match_idx = None;
                                }
                            }
                        }
                    }
                    KeyCode::Right | KeyCode::Char('l') => {
                        if expanded_match_idx.is_none() {
                            if active_match_idx.is_none() {
                                if !current_explanation.matches.is_empty() {
                                    active_match_idx = Some(0);
                                    expanded_match_idx = None;
                                }
                            } else if let Some(m_idx) = active_match_idx {
                                if m_idx + 1 < current_explanation.matches.len() {
                                    active_match_idx = Some(m_idx + 1);
                                    expanded_match_idx = None;
                                }
                            }
                        }
                    }
                    KeyCode::Enter => {
                        if let Some(m_idx) = active_match_idx {
                            if expanded_match_idx == Some(m_idx) {
                                expanded_match_idx = None;
                                expanded_scroll = 0;
                            } else {
                                expanded_match_idx = Some(m_idx);
                                expanded_scroll = 0;
                            }
                        } else if !current_explanation.matches.is_empty() {
                            active_match_idx = Some(0);
                            expanded_match_idx = Some(0);
                            expanded_scroll = 0;
                        }
                    }
                    KeyCode::Esc => {
                        if expanded_match_idx.is_some() {
                            expanded_match_idx = None;
                            expanded_scroll = 0;
                        } else if active_match_idx.is_some() {
                            active_match_idx = None;
                            expanded_match_idx = None;
                        } else {
                            break;
                        }
                    }
                    KeyCode::Char('q') => break,
                    _ => {}
                    }
                }
                Event::Resize(..) => {
                    force_redraw = true;
                }
                _ => {}
            }
        }
    }

    write!(stdout, "\x1b[2J\x1b[H")?;
    stdout.flush()?;
    let _ = std::fs::remove_file(&json_path);
    let _ = std::fs::remove_file(json_path.replace(".json", ".max_scroll"));
    Ok(())
}

struct ParsedMatch {
    index: usize,
    source: String,
    explanation: String,
}

struct ParsedJson {
    _command: String,
    error: Option<String>,
    active_match_idx: Option<usize>,
    expanded_match_idx: Option<usize>,
    expanded_scroll: usize,
    matches: Vec<ParsedMatch>,
}

fn unescape_json(s: &str) -> String {
    let mut result = String::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            if let Some(next) = chars.next() {
                match next {
                    '"' => result.push('"'),
                    '\\' => result.push('\\'),
                    'n' => result.push('\n'),
                    'r' => result.push('\r'),
                    't' => result.push('\t'),
                    _ => {
                        result.push('\\');
                        result.push(next);
                    }
                }
            } else {
                result.push('\\');
            }
        } else {
            result.push(c);
        }
    }
    result
}

fn parse_usize_field(json: &str, key: &str) -> Option<usize> {
    if let Some(pos) = json.find(key) {
        let after = &json[pos + key.len()..];
        let mut chars = after.chars().peekable();
        while let Some(&c) = chars.peek() {
            if c == ':' || c.is_whitespace() {
                chars.next();
            } else {
                break;
            }
        }
        let mut val_str = String::new();
        while let Some(&c) = chars.peek() {
            if c.is_digit(10) {
                val_str.push(c);
                chars.next();
            } else {
                break;
            }
        }
        if !val_str.is_empty() {
            return val_str.parse().ok();
        }
    }
    None
}

fn parse_string_field(json: &str, key: &str) -> Option<String> {
    if let Some(pos) = json.find(key) {
        let after = &json[pos + key.len()..];
        if let Some(start_quote) = after.find('"') {
            let str_start = start_quote + 1;
            let mut escaped = false;
            let mut end_quote = None;
            let chars: Vec<(usize, char)> = after[str_start..].char_indices().collect();
            for (idx, c) in chars {
                if escaped {
                    escaped = false;
                } else if c == '\\' {
                    escaped = true;
                } else if c == '"' {
                    end_quote = Some(str_start + idx);
                    break;
                }
            }
            if let Some(end) = end_quote {
                let raw_str = &after[str_start..end];
                return Some(unescape_json(raw_str));
            }
        }
    }
    None
}

fn parse_matches(json: &str) -> Vec<ParsedMatch> {
    let mut matches = Vec::new();
    if let Some(matches_start) = json.find("\"matches\":") {
        let after_matches = &json[matches_start..];
        if let Some(arr_start) = after_matches.find('[') {
            let mut arr_content = &after_matches[arr_start + 1..];
            while let Some(obj_start) = arr_content.find('{') {
                let mut brace_depth = 0;
                let mut in_string = false;
                let mut escaped = false;
                let mut obj_end = None;
                let chars: Vec<(usize, char)> = arr_content[obj_start..].char_indices().collect();
                for (idx, c) in chars {
                    if in_string {
                        if escaped {
                            escaped = false;
                        } else if c == '\\' {
                            escaped = true;
                        } else if c == '"' {
                            in_string = false;
                        }
                    } else {
                        if c == '"' {
                            in_string = true;
                        } else if c == '{' {
                            brace_depth += 1;
                        } else if c == '}' {
                            brace_depth -= 1;
                            if brace_depth == 0 {
                                obj_end = Some(obj_start + idx);
                                break;
                            }
                        }
                    }
                }
                
                if let Some(end) = obj_end {
                    let obj_str = &arr_content[obj_start..=end];
                    let index = parse_usize_field(obj_str, "\"index\"").unwrap_or(0);
                    let source = parse_string_field(obj_str, "\"source\"").unwrap_or_default();
                    let explanation = parse_string_field(obj_str, "\"explanation\"").unwrap_or_default();
                    matches.push(ParsedMatch { index, source, explanation });
                    
                    arr_content = &arr_content[end + 1..];
                } else {
                    break;
                }
            }
        }
    }
    matches
}

fn parse_json(json: &str) -> ParsedJson {
    let clean_json = json
        .replace("\x1b[31;1m", "")
        .replace("\x1b[0m", "");
    
    let command = parse_string_field(&clean_json, "\"command\"").unwrap_or_default();
    let error = parse_string_field(&clean_json, "\"error\"");
    let active_match_idx = parse_usize_field(&clean_json, "\"active_match_idx\"");
    let expanded_match_idx = parse_usize_field(&clean_json, "\"expanded_match_idx\"");
    let expanded_scroll = parse_usize_field(&clean_json, "\"expanded_scroll\"").unwrap_or(0);
    let matches = parse_matches(&clean_json);

    ParsedJson {
        _command: command,
        error,
        active_match_idx,
        expanded_match_idx,
        expanded_scroll,
        matches,
    }
}

fn clean_explanation(explanation: &str) -> String {
    let mut s = explanation.trim();
    loop {
        let lower = s.to_lowercase();
        if lower.starts_with("[source]\n") {
            s = s["[source]\n".len()..].trim();
        } else if lower.starts_with("[source]\r\n") {
            s = s["[source]\r\n".len()..].trim();
        } else if lower.starts_with("[soruce]\n") {
            s = s["[soruce]\n".len()..].trim();
        } else if lower.starts_with("[soruce]\r\n") {
            s = s["[soruce]\r\n".len()..].trim();
        } else {
            break;
        }
    }
    s.to_string()
}



fn render_html(html_or_text: &str, cols: usize) -> String {
    use std::io::Write;
    let child = std::process::Command::new("w3m")
        .args(["-dump", "-T", "text/html", "-cols", &cols.to_string()])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok();
        
    if let Some(mut child) = child {
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(html_or_text.as_bytes());
        }
        if let Ok(output) = child.wait_with_output() {
            if output.status.success() {
                return String::from_utf8_lossy(&output.stdout).trim().to_string();
            }
        }
    }
    explainshell::strip_html_tags(html_or_text)
}

fn get_first_description_line(text: &str) -> String {
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if (trimmed.starts_with('-') && trimmed.len() <= 6) || trimmed.len() <= 3 {
            continue;
        }
        return trimmed.to_string();
    }
    for line in text.lines() {
        let trimmed = line.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    String::new()
}

fn truncate_to_line(s: &str, max_len: usize) -> String {
    let flat: String = s.replace('\n', " ").replace('\r', " ");
    let char_count = flat.chars().count();
    if char_count <= max_len {
        flat
    } else {
        let truncated: String = flat.chars().take(max_len.saturating_sub(3)).collect();
        format!("{}...", truncated)
    }
}

fn format_lines(content: &str, raw_json: bool, cols: u16, rows: u16) -> (Vec<String>, usize) {
    if raw_json {
        let lines: Vec<String> = content.lines().map(|s| s.to_string()).collect();
        return (lines, 0);
    }
    
    let trimmed = content.trim();
    if trimmed.is_empty() {
        return (vec!["\x1b[37mNo command selected or explanation loaded yet.\x1b[0m".to_string()], 0);
    }
    
    let parsed = parse_json(content);
    
    if let Some(err) = parsed.error {
        return (vec![format!("\x1b[37m{}\x1b[0m", err)], 0);
    }
    
    if parsed.matches.is_empty() {
        return (vec!["\x1b[37mNo explanation matches found.\x1b[0m".to_string()], 0);
    }
    
    if let Some(idx) = parsed.expanded_match_idx {
        if let Some(m) = parsed.matches.iter().find(|m| m.index == idx) {
            let mut lines = Vec::new();
            lines.push(format!("\x1b[31;1m{}\x1b[0m", m.source));
            lines.push(String::new());
            
            let cleaned_raw = clean_explanation(&m.explanation);
            let wrap_width = (cols as usize).saturating_sub(4);
            let rendered = render_html(&cleaned_raw, wrap_width);
            
            let desc_lines: Vec<&str> = rendered.lines().collect();
            let max_lines = (rows as usize).saturating_sub(1);
            let visible_desc_lines = max_lines.saturating_sub(2);
            let max_scroll = desc_lines.len().saturating_sub(visible_desc_lines);
            let scroll = parsed.expanded_scroll.min(max_scroll);
            
            for line in desc_lines.iter().skip(scroll) {
                lines.push(format!("  \x1b[38;5;250m{}\x1b[0m", line));
            }
            return (lines, max_scroll);
        }
    }
    
    let mut lines = Vec::new();
    let max_source_len = parsed.matches.iter()
        .map(|m| m.source.chars().count())
        .max()
        .unwrap_or(0);
        
    for m in &parsed.matches {
        let source_len = m.source.chars().count();
        let pad_len = max_source_len.saturating_sub(source_len);
        let padding = " ".repeat(pad_len);
        let is_active = parsed.active_match_idx == Some(m.index);
        
        let left_width = max_source_len + 2;
        let avail_width = (cols as usize).saturating_sub(left_width + 2);
        
        let cleaned_raw = clean_explanation(&m.explanation);
        let stripped = explainshell::strip_html_tags(&cleaned_raw);
        
        let desc = get_first_description_line(&stripped);
        let truncated_exp = truncate_to_line(&desc, avail_width);
        
        if is_active {
            lines.push(format!("\x1b[31;1m▸ {}{}\x1b[0m  \x1b[38;5;250m{}\x1b[0m", m.source, padding, truncated_exp));
        } else {
            lines.push(format!("\x1b[33m  {}{}\x1b[0m  \x1b[38;5;250m{}\x1b[0m", m.source, padding, truncated_exp));
        }
    }
    
    (lines, 0)
}

fn watch_json_mode(json_path: String, raw_json: bool) -> Result<()> {
    let mut last_content = String::new();
    let mut missing_count = 0;
    let mut last_cols = 0;
    let mut last_rows = 0;
    
    // Enter alternate screen
    print!("\x1b[?1049h");
    io::stdout().flush()?;

    // Allow tmux window layout/resize to settle before first render query
    std::thread::sleep(Duration::from_millis(100));
    print!("\x1b[H\x1b[2J\x1b[3J");
    io::stdout().flush()?;
    
    loop {
        let (cols, rows) = match size() {
            Ok((c, r)) => (c, r),
            Err(_) => (80, 24),
        };
        let max_lines = (rows as usize).saturating_sub(1); // Leave 1 line safety margin
        let size_changed = cols != last_cols || rows != last_rows;

        match std::fs::read_to_string(&json_path) {
            Ok(content) => {
                missing_count = 0;
                if content != last_content || size_changed {
                    let (lines, max_scroll) = format_lines(&content, raw_json, cols, rows);
                    let max_scroll_path = json_path.replace(".json", ".max_scroll");
                    let _ = std::fs::write(&max_scroll_path, max_scroll.to_string());
                    if lines.len() <= max_lines {
                        for (idx, line) in lines.iter().enumerate() {
                            print!("\x1b[{};1H{}\x1b[K", idx + 1, line);
                        }
                        let next_row = lines.len() + 1;
                        if next_row <= rows as usize {
                            print!("\x1b[{};1H\x1b[J", next_row);
                        }
                    } else {
                        let mut highlight_line_idx = None;
                        for (idx, line) in lines.iter().enumerate() {
                            if line.contains("\x1b[1;38;5;203m") {
                                highlight_line_idx = Some(idx);
                                break;
                            }
                        }

                        let start_line = if let Some(h_idx) = highlight_line_idx {
                            h_idx.saturating_sub(max_lines / 2)
                        } else {
                            0
                        };

                        let end_line = (start_line + max_lines).min(lines.len());
                        let start_line = end_line.saturating_sub(max_lines);

                        for i in start_line..end_line {
                            let row_pos = i - start_line + 1;
                            print!("\x1b[{};1H{}\x1b[K", row_pos, lines[i]);
                        }
                        let next_row = (end_line - start_line) + 1;
                        if next_row <= rows as usize {
                            print!("\x1b[{};1H\x1b[J", next_row);
                        }
                    }
                    io::stdout().flush()?;
                    last_content = content;
                    last_cols = cols;
                    last_rows = rows;
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                missing_count += 1;
                if missing_count > 10 {
                    break;
                }
            }
            Err(_) => {}
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let _ = std::fs::remove_file(json_path.replace(".json", ".max_scroll"));
    print!("\x1b[?1049l"); // Leave alternate screen
    let _ = io::stdout().flush();
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

/// Write a tiny wrapper script that logs the -c argument then runs the real shell, capturing status.
fn write_wrapper(path: PathBuf, real: &PathBuf, trace: &PathBuf) -> Result<()> {
    let script = format!(
        "#!/bin/sh\nif [ \"$1\" = \"-c\" ] && [ -n \"$2\" ]; then\n  PID=$$\n  CMD=$(printf '%s' \"$2\" | tr '\\n' ' ')\n  printf 'START\\t%d\\t%s\\n' \"$PID\" \"$CMD\" >> {} 2>/dev/null\n  {} \"$@\"\n  STATUS=$?\n  printf 'END\\t%d\\t%d\\n' \"$PID\" \"$STATUS\" >> {} 2>/dev/null\n  exit $STATUS\nelse\n  exec {} \"$@\"\nfi\n",
        sq(&trace.to_string_lossy()),
        sq(&real.to_string_lossy()),
        sq(&trace.to_string_lossy()),
        sq(&real.to_string_lossy()),
    );
    std::fs::write(&path, &script)?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))?;
    Ok(())
}

fn get_explainshell_port() -> u16 {
    std::env::var("EXPLAINSHELL_PORT")
        .ok()
        .and_then(|val| val.parse().ok())
        .unwrap_or(5000)
}

fn is_explainshell_running(port: u16) -> bool {
    let addr_str = format!("127.0.0.1:{}", port);
    if let Ok(addr) = addr_str.parse() {
        std::net::TcpStream::connect_timeout(&addr, Duration::from_millis(200)).is_ok()
    } else {
        false
    }
}

fn find_explainshell_dir() -> Option<PathBuf> {
    let home = std::env::var("HOME").ok().map(PathBuf::from)?;
    let candidates = [
        home.join("scratch/explainshell"),
        home.join("explainshell"),
        home.join("projects/explainshell"),
    ];
    for path in &candidates {
        if path.join("runserver.py").exists() {
            return Some(path.clone());
        }
    }
    None
}

fn start_explainshell(dir: &PathBuf, port: u16) {
    let python_path = if dir.join(".venv/bin/python").exists() {
        dir.join(".venv/bin/python")
    } else {
        PathBuf::from("python")
    };
    
    let _ = std::process::Command::new(python_path)
        .arg("runserver.py")
        .current_dir(dir)
        .env("DB_PATH", "explainshell.db")
        .env("PORT", port.to_string())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

fn launch_mode(agent_name: String, extra_args: Vec<String>, raw_json: bool) -> Result<()> {
    let port = get_explainshell_port();
    if !is_explainshell_running(port) {
        if let Some(dir) = find_explainshell_dir() {
            println!("\x1b[33mStarting local explainshell server on localhost:{}...\x1b[0m", port);
            start_explainshell(&dir, port);
            std::thread::sleep(Duration::from_millis(800));
        }
    }

    let agent_path = which::which(&agent_name)
        .unwrap_or_else(|_| panic!("'{}' not found in PATH", agent_name));

    let pid = std::process::id();
    let wrapper_dir = PathBuf::from(format!("/tmp/flw_{}_bin", pid));
    let trace_path = PathBuf::from(format!("/tmp/flw_{}.trace", pid));
    let json_path = PathBuf::from(format!("/tmp/flw_{}.json", pid));
    let flw_bin = std::env::current_exe()?;
    let in_tmux = std::env::var("TMUX").is_ok();
    let window_name = format!("flw-{}", agent_name);

    // Create bash and sh wrappers that log -c commands to the trace file
    std::fs::create_dir_all(&wrapper_dir)?;
    write_wrapper(wrapper_dir.join("bash"), &real_shell("bash"), &trace_path)?;
    write_wrapper(wrapper_dir.join("sh"), &real_shell("sh"), &trace_path)?;

    // Create an empty json file initially so the tail loop doesn't fail
    std::fs::write(&json_path, "")?;

    // Left pane: agent runs normally, just with our wrapper dir prepended to PATH
    let extra: String = extra_args.iter().map(|a| format!(" {}", sq(a))).collect();

    // Right pane top: flw --watch-list <trace> <json>
    let right_top_cmd = format!(
        "{} --watch-list {} {}",
        sq(&flw_bin.to_string_lossy()),
        sq(&trace_path.to_string_lossy()),
        sq(&json_path.to_string_lossy()),
    );

    // Right pane bottom: flw --watch-json [--raw-json] <json>
    let right_bottom_cmd = if raw_json {
        format!(
            "{} --watch-json --raw-json {}",
            sq(&flw_bin.to_string_lossy()),
            sq(&json_path.to_string_lossy()),
        )
    } else {
        format!(
            "{} --watch-json {}",
            sq(&flw_bin.to_string_lossy()),
            sq(&json_path.to_string_lossy()),
        )
    };

    let toggle_script_path = PathBuf::from(format!("/tmp/flw_{}_toggle.sh", pid));
    let toggle_key = std::env::var("FLW_KEY").unwrap_or_else(|_| "C-f".to_string());

    // Also clean up the binding when agi exits
    let cleanup = format!(
        "; tmux unbind-key -n {} 2>/dev/null; rm -rf {} {} {} {}; tmux kill-window -t \"$TMUX_PANE\" 2>/dev/null || tmux kill-window 2>/dev/null",
        sq(&toggle_key),
        sq(&wrapper_dir.to_string_lossy()),
        sq(&trace_path.to_string_lossy()),
        sq(&json_path.to_string_lossy()),
        sq(&toggle_script_path.to_string_lossy()),
    );
    let left_cmd = format!(
        "PATH={}:$PATH {}{}{}",
        sq(&wrapper_dir.to_string_lossy()),
        sq(&agent_path.to_string_lossy()),
        extra,
        cleanup,
    );

    let (agent_pane, agent_window) = if in_tmux {
        // Create a new window in the existing session (no nesting)
        let output = std::process::Command::new("tmux")
            .args(["new-window", "-P", "-F", "#{pane_id} #{window_id}", "-n", &window_name, &left_cmd])
            .output()?;
        let out_str = String::from_utf8_lossy(&output.stdout);
        let mut parts = out_str.trim().split_whitespace();
        let pane = parts.next().map(|s| s.to_string()).unwrap_or_else(|| "%0".to_string());
        let window = parts.next().map(|s| s.to_string()).unwrap_or_else(|| "@0".to_string());

        // Style pane borders for the newly created window so they disappear/blend in
        let _ = std::process::Command::new("tmux")
            .args(["set-option", "-t", &window, "pane-border-lines", "hidden"])
            .status();
        let _ = std::process::Command::new("tmux")
            .args(["set-option", "-t", &window, "pane-border-style", "fg=black,bg=default"])
            .status();
        let _ = std::process::Command::new("tmux")
            .args(["set-option", "-t", &window, "pane-active-border-style", "fg=black,bg=default"])
            .status();
        (pane, window)
    } else {
        // Not in tmux — create a session and attach
        let session = format!("flw_{}", pid);
        let output = std::process::Command::new("tmux")
            .args(["new-session", "-d", "-s", &session, "-P", "-F", "#{pane_id} #{window_id}", &left_cmd])
            .output()?;
        let out_str = String::from_utf8_lossy(&output.stdout);
        let mut parts = out_str.trim().split_whitespace();
        let pane = parts.next().map(|s| s.to_string()).unwrap_or_else(|| "%0".to_string());
        let window = parts.next().map(|s| s.to_string()).unwrap_or_else(|| "@0".to_string());

        let _ = std::process::Command::new("tmux")
            .args(["set-option", "-t", &session, "status", "off"])
            .status();
        // Style pane borders for the session so they disappear/blend in
        let _ = std::process::Command::new("tmux")
            .args(["set-option", "-t", &window, "pane-border-lines", "hidden"])
            .status();
        let _ = std::process::Command::new("tmux")
            .args(["set-option", "-t", &window, "pane-border-style", "fg=black,bg=default"])
            .status();
        let _ = std::process::Command::new("tmux")
            .args(["set-option", "-t", &window, "pane-active-border-style", "fg=black,bg=default"])
            .status();
        let _ = std::process::Command::new("tmux")
            .args(["select-pane", "-t", &pane])
            .status();
        (pane, window)
    };

    let toggle_script = format!(
        "#!/bin/sh\n\
        active_window=$(tmux display-message -p '#{{window_id}}')\n\
        if [ \"$active_window\" != {} ]; then\n\
          exit 0\n\
        fi\n\
        num_panes=$(tmux list-panes -t {} | wc -l)\n\
        if [ \"$num_panes\" -eq 1 ]; then\n\
          right_pane=$(tmux split-window -h -P -F \"#{{pane_id}}\" -t {} {})\n\
          if [ -n \"$right_pane\" ]; then\n\
            tmux split-window -v -l 62% -d -t \"$right_pane\" {}\n\
          fi\n\
        else\n\
          tmux kill-pane -a -t {}\n\
        fi\n",
        sq(&agent_window),
        sq(&agent_pane),
        sq(&agent_pane),
        sq(&right_top_cmd),
        sq(&right_bottom_cmd),
        sq(&agent_pane)
    );
    std::fs::write(&toggle_script_path, &toggle_script)?;
    std::fs::set_permissions(&toggle_script_path, std::fs::Permissions::from_mode(0o755))?;

    // Toggle key binding using a dedicated shell script to avoid escaping hell
    std::process::Command::new("tmux")
        .args([
            "bind-key", "-n", &toggle_key,
            "run-shell",
            &toggle_script_path.to_string_lossy(),
        ])
        .status()?;

    if !in_tmux {
        let session = format!("flw_{}", pid);
        std::process::Command::new("tmux")
            .args(["attach-session", "-t", &session])
            .status()?;
    }

    Ok(())
}

// ── Entry point ───────────────────────────────────────────────────────────────

fn main() -> Result<()> {
    let mut argv: Vec<String> = std::env::args().skip(1).collect();

    if argv.first().map(String::as_str) == Some("--watch-list") {
        let trace = argv.get(1).cloned().expect("flw --watch-list <trace_file> <json_file>");
        let json = argv.get(2).cloned().expect("flw --watch-list <trace_file> <json_file>");
        return watch_list_mode(trace, json);
    }
    
    if argv.first().map(String::as_str) == Some("--watch-json") {
        let has_raw = argv.get(1).map(String::as_str) == Some("--raw-json");
        let json = if has_raw {
            argv.get(2).cloned().expect("flw --watch-json --raw-json <json_file>")
        } else {
            argv.get(1).cloned().expect("flw --watch-json <json_file>")
        };
        return watch_json_mode(json, has_raw);
    }

    let mut raw_json = false;
    if let Some(pos) = argv.iter().position(|arg| arg == "--raw-json") {
        raw_json = true;
        argv.remove(pos);
    }

    let agent_name = argv
        .first()
        .cloned()
        .expect("Usage: flw <agent>  e.g.  flw agi");
    let extra_args = argv[1..].to_vec();

    launch_mode(agent_name, extra_args, raw_json)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_clean_explanation() {
        assert_eq!(clean_explanation("[source]\nHello World"), "Hello World");
        assert_eq!(clean_explanation("[soruce]\r\nHello World"), "Hello World");
        assert_eq!(clean_explanation("[SOURCE]\nHello World"), "Hello World");
        assert_eq!(clean_explanation("Hello World"), "Hello World");
        assert_eq!(clean_explanation("  [soruce]\n  foo \n  "), "foo");
    }

    #[test]
    fn test_parse_json() {
        let json = r#"{
  "command": "ls -la",
  "active_match_idx": 0,
  "expanded_match_idx": null,
  "matches": [
    {
      "index": 0,
      "source": "ls",
      "explanation": "[source]\nlist directory contents"
    }
  ]
}"#;
        let parsed = parse_json(json);
        assert_eq!(parsed.active_match_idx, Some(0));
        assert_eq!(parsed.expanded_match_idx, None);
        assert_eq!(parsed.matches.len(), 1);
        assert_eq!(parsed.matches[0].index, 0);
        assert_eq!(parsed.matches[0].source, "ls");
        assert_eq!(parsed.matches[0].explanation, "[source]\nlist directory contents");
    }

    #[test]
    fn test_format_lines() {
        let json = r#"{
  "command": "ls -la --color=always",
  "active_match_idx": 1,
  "expanded_match_idx": null,
  "matches": [
    {
      "index": 0,
      "source": "ls",
      "explanation": "[source]\nlist directory contents."
    },
    {
      "index": 1,
      "source": "-la",
      "explanation": "[soruce]\ndo not ignore entries starting with . and use a long listing format"
    }
  ]
}"#;
        let (lines, _max_scroll) = format_lines(json, false, 80, 24);
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("  ls"));
        assert!(lines[0].contains("list directory contents."));
        assert!(!lines[0].contains("[source]"));
        
        assert!(lines[1].contains("▸ -la"));
        assert!(lines[1].contains("do not ignore entries starting with . and use a long listing format"));
        assert!(!lines[1].contains("[soruce]"));
    }

    #[test]
    fn test_get_first_description_line() {
        let exp = "  -l\n  List in long format, giving mode...";
        assert_eq!(get_first_description_line(exp), "List in long format, giving mode...");
        
        let exp2 = "Just a plain description sentence.";
        assert_eq!(get_first_description_line(exp2), "Just a plain description sentence.");
    }



    #[test]
    fn test_strip_shopt_wrapper() {
        // 1. Unwrapped command
        assert_eq!(strip_shopt_wrapper("ls -la"), "ls -la");

        // 2. Wrapped command with no extra trailing noise (just the parenthesis)
        assert_eq!(
            strip_shopt_wrapper("shopt -u promptvars nullglob extglob nocaseglob dotglob; ( echo hello )"),
            "echo hello"
        );

        // 3. Wrapped command with __code exiting noise
        let complex = r#"shopt -u promptvars nullglob extglob nocaseglob dotglob; ( ls -R && touch complex_demo_file.txt && echo "success." ) __code=$?; pgrep -g 0 >/tmp/gemini-shell-Neupw6/pgrep.tmp 2>&1; exit $__code;"#;
        assert_eq!(
            strip_shopt_wrapper(complex),
            r#"ls -R && touch complex_demo_file.txt && echo "success.""#
        );

        // 4. Wrapped command with a subshell inside the command itself
        let nested = r#"shopt -u promptvars nullglob extglob nocaseglob dotglob; ( (echo "nested") ) __code=$?;"#;
        assert_eq!(
            strip_shopt_wrapper(nested),
            r#"(echo "nested")"#
        );
        
        // 5. With newline variations in the trailing noise
        let newlines = "shopt -u promptvars nullglob extglob nocaseglob dotglob; ( echo multiline )\n__code=$?; pgrep";
        assert_eq!(
            strip_shopt_wrapper(newlines),
            "echo multiline"
        );
    }
}
