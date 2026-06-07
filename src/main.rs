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
    out.push_str("\x1b[H\x1b[2J\x1b[?25l");

    let header_height = 1;
    let list_height = (rows as usize).saturating_sub(header_height);

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
            let cmd_text = &cmds[idx].text;
            let display_cmd = if idx == selected {
                explanation.formatted_command.as_deref().unwrap_or(cmd_text)
            } else {
                cmd_text
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
        if i < list_height - 1 {
            out.push_str("\r\n");
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

    let mut selected: usize = 0;
    let mut scroll: usize = 0;
    let mut follow = true;
    let mut active_match_idx: Option<usize> = None;
    let mut expanded_match_idx: Option<usize> = None;

    // Caching explainshell Explanation results
    let cache = Arc::new(Mutex::new(std::collections::HashMap::<String, explainshell::Explanation>::new()));
    let mut last_selected = usize::MAX;
    let mut last_active_match_idx: Option<usize> = None;
    let mut last_expanded_match_idx: Option<usize> = None;
    let mut last_cache_status: Option<String> = None;
    let mut last_count = usize::MAX;

    loop {
        let (cols, rows) = size()?;
        let visible_list_rows = (rows as usize).saturating_sub(1);

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
            || Some(&current_cache_status) != last_cache_status.as_ref() 
            || should_redraw 
        {
            let cmds = commands.lock().unwrap();
            
            // Generate JSON and write to file for the bottom pane
            let active_status = cmds.get(selected).and_then(|c| c.status);
            let active_pid = cmds.get(selected).map(|c| c.id);
            let json_str = explainshell::explanation_to_json(&current_explanation, active_match_idx, expanded_match_idx, active_status, active_pid);
            
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
                        active_match_idx = None;
                        expanded_match_idx = None;
                        follow = false;
                        selected = selected.saturating_sub(1);
                        scroll = scroll.min(selected);
                    }
                    KeyCode::Down | KeyCode::Char('j') => {
                        active_match_idx = None;
                        expanded_match_idx = None;
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
                                expanded_match_idx = None;
                            } else {
                                active_match_idx = None;
                                expanded_match_idx = None;
                            }
                        }
                    }
                    KeyCode::Right | KeyCode::Char('l') => {
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
                    KeyCode::Enter => {
                        if let Some(m_idx) = active_match_idx {
                            if expanded_match_idx == Some(m_idx) {
                                expanded_match_idx = None;
                            } else {
                                expanded_match_idx = Some(m_idx);
                            }
                        }
                    }
                    KeyCode::Esc => {
                        if active_match_idx.is_some() {
                            active_match_idx = None;
                            expanded_match_idx = None;
                        } else {
                            break;
                        }
                    }
                    KeyCode::Char('q') => break,
                    _ => {}
                }
            },
            Event::Resize(c, r) => {
                    let cmds = commands.lock().unwrap();
                    render_list(&cmds, selected, scroll, &current_explanation, active_match_idx, c, r, &mut stdout)?;
                    last_count = cmds.len();
                }
                _ => {}
            }
        }
    }

    write!(stdout, "\x1b[2J\x1b[H")?;
    stdout.flush()?;
    let _ = std::fs::remove_file(&json_path);
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
    let matches = parse_matches(&clean_json);

    ParsedJson {
        _command: command,
        error,
        active_match_idx,
        expanded_match_idx,
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

fn format_lines(content: &str, raw_json: bool, cols: u16) -> Vec<String> {
    if raw_json {
        return content.lines().map(|s| s.to_string()).collect();
    }
    
    let trimmed = content.trim();
    if trimmed.is_empty() {
        return vec!["\x1b[37mNo command selected or explanation loaded yet.\x1b[0m".to_string()];
    }
    
    let parsed = parse_json(content);
    
    if let Some(err) = parsed.error {
        return vec![format!("\x1b[37m{}\x1b[0m", err)];
    }
    
    if parsed.matches.is_empty() {
        return vec!["\x1b[37mNo explanation matches found.\x1b[0m".to_string()];
    }
    
    if let Some(idx) = parsed.expanded_match_idx {
        if let Some(m) = parsed.matches.iter().find(|m| m.index == idx) {
            let mut lines = Vec::new();
            lines.push(format!("\x1b[31;1m{}\x1b[0m", m.source));
            lines.push(String::new());
            
            let cleaned_raw = clean_explanation(&m.explanation);
            let wrap_width = (cols as usize).saturating_sub(4);
            let rendered = render_html(&cleaned_raw, wrap_width);
            for line in rendered.lines() {
                lines.push(format!("  \x1b[38;5;250m{}\x1b[0m", line));
            }
            return lines;
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
        let rendered = render_html(&cleaned_raw, 500);
        
        let desc = get_first_description_line(&rendered);
        let truncated_exp = truncate_to_line(&desc, avail_width);
        
        if is_active {
            lines.push(format!("\x1b[31;1m▸ {}{}\x1b[0m  \x1b[38;5;250m{}\x1b[0m", m.source, padding, truncated_exp));
        } else {
            lines.push(format!("\x1b[33m  {}{}\x1b[0m  \x1b[38;5;250m{}\x1b[0m", m.source, padding, truncated_exp));
        }
    }
    
    lines
}

fn watch_json_mode(json_path: String, raw_json: bool) -> Result<()> {
    let mut last_content = String::new();
    let mut missing_count = 0;
    
    // Clear screen on start
    print!("\x1b[H\x1b[2J");
    io::stdout().flush()?;
    
    loop {
        let (cols, rows) = match size() {
            Ok((c, r)) => (c, r),
            Err(_) => (80, 24),
        };
        let max_lines = (rows as usize).saturating_sub(1); // Leave 1 line safety margin

        match std::fs::read_to_string(&json_path) {
            Ok(content) => {
                missing_count = 0;
                if content != last_content {
                    let lines = format_lines(&content, raw_json, cols);
                    if lines.len() <= max_lines {
                        print!("\x1b[H"); // Overwrite from top-left without blanking out screen
                        for (idx, line) in lines.iter().enumerate() {
                            if idx > 0 {
                                print!("\r\n");
                            }
                            print!("{}\x1b[K", line); // Clear line to prevent trailing leftovers
                        }
                        print!("\x1b[J"); // Clear remaining old lines below
                    } else {
                        // Find the line containing the highlight marker \x1b[31;1m
                        let mut highlight_line_idx = None;
                        for (idx, line) in lines.iter().enumerate() {
                            if line.contains("\x1b[31;1m") {
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

                        print!("\x1b[H"); // Overwrite from top-left without blanking out screen
                        for i in start_line..end_line {
                            if i > start_line {
                                print!("\r\n");
                            }
                            print!("{}\x1b[K", lines[i]); // Clear line to prevent trailing leftovers
                        }
                        print!("\x1b[J"); // Clear remaining old lines below
                    }
                    io::stdout().flush()?;
                    last_content = content;
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

fn launch_mode(agent_name: String, extra_args: Vec<String>, raw_json: bool) -> Result<()> {
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
    let toggle_script = format!(
        "#!/bin/sh\nif [ \"$(tmux list-panes | wc -l)\" -eq 1 ]; then\n  tmux split-window -h {}\n  tmux split-window -v -d {}\nelse\n  tmux kill-pane -a -t 0\nfi\n",
        sq(&right_top_cmd),
        sq(&right_bottom_cmd)
    );
    std::fs::write(&toggle_script_path, &toggle_script)?;
    std::fs::set_permissions(&toggle_script_path, std::fs::Permissions::from_mode(0o755))?;

    let toggle_key = std::env::var("FLW_KEY").unwrap_or_else(|_| "C-f".to_string());

    // Toggle key binding using a dedicated shell script to avoid escaping hell
    std::process::Command::new("tmux")
        .args([
            "bind-key", "-n", &toggle_key,
            "run-shell",
            &toggle_script_path.to_string_lossy(),
        ])
        .status()?;

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
        let lines = format_lines(json, false, 80);
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
