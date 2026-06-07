use anyhow::Result;

#[derive(Clone, Debug)]
pub struct Match {
    pub index: usize,
    pub source: String,
    pub explanation: String,
    pub start: usize,
    pub end: usize,
}

#[derive(Clone, Debug)]
pub struct Explanation {
    pub command: String,
    pub formatted_command: Option<String>,
    pub error: Option<String>,
    pub matches: Vec<Match>,
}

/// URL encode a string for query parameters.
fn url_encode(input: &str) -> String {
    let mut encoded = String::new();
    for byte in input.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(byte as char);
            }
            b' ' => {
                encoded.push('+');
            }
            _ => {
                encoded.push_str(&format!("%{:02X}", byte));
            }
        }
    }
    encoded
}

fn is_valid_response(html: &str) -> bool {
    html.contains("help-box") || html.contains("missing man page") || html.contains("parsing error")
}

/// Fetch the HTML content of the explainshell page for a given command.
pub fn fetch_html(cmd: &str) -> Result<String> {
    let encoded = url_encode(cmd);
    let port = std::env::var("EXPLAINSHELL_PORT")
        .ok()
        .and_then(|val| val.parse::<u16>().ok())
        .unwrap_or(5000);
    
    // Try local instance first
    let local_url = format!("http://localhost:{}/explain?cmd={}", port, encoded);
    if let Ok(output) = std::process::Command::new("curl")
        .args(["-s", "-S", "--max-time", "2", &local_url])
        .output()
    {
        if output.status.success() && !output.stdout.is_empty() {
            let html = String::from_utf8_lossy(&output.stdout).into_owned();
            if is_valid_response(&html) {
                return Ok(html);
            }
        }
    }

    // Fallback to explainshell.com
    let remote_url = format!("https://explainshell.com/explain?cmd={}", encoded);
    let output = std::process::Command::new("curl")
        .args(["-s", "-S", "--max-time", "5", &remote_url])
        .output()?;
    
    if output.status.success() {
        let html = String::from_utf8_lossy(&output.stdout).into_owned();
        if is_valid_response(&html) {
            return Ok(html);
        }
    }
    
    anyhow::bail!("Could not retrieve explanation from explainshell (tried localhost:5000 and explainshell.com)")
}

/// Clean up HTML content, stripping tags and restoring formatting (newliness/spaces).
pub fn strip_html_tags(html: &str) -> String {
    let mut output = String::new();
    let mut in_tag = false;
    let mut last_was_space = false;
    
    let preprocessed = html
        .replace("</p>", "\n\n")
        .replace("<br>", "\n")
        .replace("<br/>", "\n")
        .replace("<br />", "\n")
        .replace("</li>", "\n")
        .replace("</td>", " ")
        .replace("</tr>", "\n");

    for c in preprocessed.chars() {
        if c == '<' {
            in_tag = true;
        } else if c == '>' {
            in_tag = false;
        } else if !in_tag {
            if c.is_whitespace() {
                if c == '\n' || c == '\r' {
                    if !output.ends_with('\n') {
                        output.push('\n');
                    }
                    last_was_space = true;
                } else if !last_was_space {
                    output.push(' ');
                    last_was_space = true;
                }
            } else {
                output.push(c);
                last_was_space = false;
            }
        }
    }

    // Clean up multiple newlines/whitespace
    let mut cleaned = String::new();
    for line in output.lines() {
        let trimmed = line.trim();
        if !trimmed.is_empty() {
            cleaned.push_str(trimmed);
            cleaned.push('\n');
        } else if !cleaned.ends_with("\n\n") && !cleaned.is_empty() {
            cleaned.push('\n');
        }
    }
    
    cleaned.trim().to_string()
}

/// Escape text characters for JSON formatting.
fn escape_json(s: &str) -> String {
    let mut escaped = String::new();
    for c in s.chars() {
        match c {
            '"' => escaped.push_str("\\\""),
            '\\' => escaped.push_str("\\\\"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            _ => escaped.push(c),
        }
    }
    escaped
}

fn parse_command_div(html: &str) -> (String, Vec<(usize, usize, String)>) {
    let mut formatted = String::new();
    let mut spans = Vec::new();
    
    let mut active_help_id = None;
    let mut active_start = 0;
    
    let mut i = 0;
    let chars: Vec<char> = html.chars().collect();
    
    while i < chars.len() {
        if html[i..].starts_with("<ul class=\"dropdown-menu\"") {
            if let Some(end) = html[i..].find("</ul>") {
                i += end + 5;
                continue;
            }
        }
        
        if html[i..].starts_with("<span") {
            if let Some(end) = html[i..].find('>') {
                let tag = &html[i..=i+end];
                if let Some(help_idx) = tag.find("helpref=\"") {
                    let start_quote = help_idx + 9;
                    if let Some(end_quote) = tag[start_quote..].find('"') {
                        let help_id = tag[start_quote..start_quote+end_quote].to_string();
                        active_help_id = Some(help_id);
                        active_start = formatted.chars().count();
                    }
                } else if tag.contains("class=\"") && tag.contains("unknown") {
                    let help_id = format!("unknown-{}", formatted.chars().count());
                    active_help_id = Some(help_id);
                    active_start = formatted.chars().count();
                }
                i += end + 1;
                continue;
            }
        }
        
        if html[i..].starts_with("</span") {
            if let Some(end) = html[i..].find('>') {
                if let Some(help_id) = active_help_id.take() {
                    spans.push((active_start, formatted.chars().count(), help_id));
                }
                i += end + 1;
                continue;
            }
        }
        
        if html[i..].starts_with('<') {
            if let Some(end) = html[i..].find('>') {
                i += end + 1;
                continue;
            }
        }
        
        if html[i..].starts_with("&amp;") { formatted.push('&'); i += 5; continue; }
        if html[i..].starts_with("&lt;") { formatted.push('<'); i += 4; continue; }
        if html[i..].starts_with("&gt;") { formatted.push('>'); i += 4; continue; }
        if html[i..].starts_with("&quot;") { formatted.push('"'); i += 6; continue; }
        if html[i..].starts_with("&#39;") { formatted.push('\''); i += 5; continue; }
        
        let c = chars[i];
        if c == '\n' || c == '\r' || c.is_whitespace() {
            if !formatted.is_empty() && !formatted.ends_with(' ') {
                formatted.push(' ');
            }
        } else {
            formatted.push(c);
        }
        i += 1;
    }
    
    (formatted.trim().to_string(), spans)
}

/// Parse the explainshell HTML into a clean structured Explanation.
pub fn parse_html(cmd: &str, html: &str) -> Explanation {
    // Check for common error pages first
    if html.contains("missing man page") {
        return Explanation {
            command: cmd.to_string(),
            formatted_command: None,
            error: Some("missing man page".to_string()),
            matches: Vec::new(),
        };
    }
    if html.contains("parsing error") {
        return Explanation {
            command: cmd.to_string(),
            formatted_command: None,
            error: Some("parsing error".to_string()),
            matches: Vec::new(),
        };
    }

    // 1. Parse formatted command and spans from <div id="command">...</div>
    let mut formatted_command = None;
    let mut formatted_spans = Vec::new();
    if let Some(cmd_start) = html.find("id=\"command\"") {
        if let Some(cmd_fragment_start) = html[cmd_start..].find('>') {
            let abs_start = cmd_start + cmd_fragment_start + 1;
            if let Some(cmd_end) = html[abs_start..].find("</div>") {
                let fragment = &html[abs_start..abs_start + cmd_end];
                let (f, s) = parse_command_div(fragment);
                if !f.is_empty() {
                    formatted_command = Some(f);
                    formatted_spans = s;
                }
            }
        }
    }

    // 2. Parse help boxes: <div class="help-box" id="help-X">...</div>
    let mut help_boxes = Vec::new();
    let mut search_idx = 0;
    while let Some(div_start) = html[search_idx..].find("<div class=\"help-box\" id=\"") {
        let start_pos = search_idx + div_start + 26;
        if let Some(id_end) = html[start_pos..].find('"') {
            let id = &html[start_pos..start_pos + id_end];
            if let Some(tag_end) = html[start_pos + id_end..].find('>') {
                let content_start = start_pos + id_end + tag_end + 1;
                if let Some(div_end) = html[content_start..].find("</div>") {
                    let inner_html = &html[content_start..content_start + div_end];
                    help_boxes.push((id.to_string(), inner_html.to_string()));
                    search_idx = content_start + div_end + 6;
                    continue;
                }
            }
        }
        search_idx += div_start + 26;
    }

    // 3. Assemble matches
    let mut matches = Vec::new();
    let mut idx = 0;
    for (start, end, ref_id) in &formatted_spans {
        let help_text_opt = if ref_id.starts_with("unknown-") {
            Some("Unknown option / argument (no explanation available in explainshell)".to_string())
        } else {
            help_boxes.iter()
                .find(|(h_id, _)| h_id == ref_id)
                .map(|(_, help_text)| help_text.clone())
        };

        if let Some(help_text) = help_text_opt {
            let source: String = if let Some(ref f) = formatted_command {
                f.chars().skip(*start).take(end - start).collect()
            } else {
                ref_id.clone()
            };
            matches.push(Match {
                index: idx,
                source,
                explanation: help_text,
                start: *start,
                end: *end,
            });
            idx += 1;
        }
    }

    // If we couldn't match spans (e.g. different HTML structure), just list the help boxes directly!
    if matches.is_empty() {
        for (id, help_text) in &help_boxes {
            matches.push(Match {
                index: idx,
                source: id.clone(),
                explanation: help_text.clone(),
                start: 0,
                end: 0,
            });
            idx += 1;
        }
    }

    Explanation {
        command: cmd.to_string(),
        formatted_command,
        error: None,
        matches,
    }
}

pub fn explanation_to_json(
    exp: &Explanation,
    active_match_idx: Option<usize>,
    expanded_match_idx: Option<usize>,
    expanded_scroll: usize,
    status: Option<i32>,
    pid: Option<u32>,
) -> String {
    let mut meta_json = String::new();
    if let Some(p) = pid {
        if p > 0 {
            meta_json.push_str(&format!(r#",
  "pid": {}"#, p));
        }
    }
    if let Some(code) = status {
        meta_json.push_str(&format!(r#",
  "status": {}"#, code));
    }

    let active_str = match active_match_idx {
        Some(idx) => idx.to_string(),
        None => "null".to_string(),
    };
    meta_json.push_str(&format!(r#",
  "active_match_idx": {}"#, active_str));

    let expanded_str = match expanded_match_idx {
        Some(idx) => idx.to_string(),
        None => "null".to_string(),
    };
    meta_json.push_str(&format!(r#",
  "expanded_match_idx": {}"#, expanded_str));

    meta_json.push_str(&format!(r#",
  "expanded_scroll": {}"#, expanded_scroll));

    if let Some(err) = &exp.error {
        return format!(
            r#"{{
  "command": "{}"{},
  "error": "{}"
}}"#,
            escape_json(&exp.command),
            meta_json,
            escape_json(err)
        );
    }

    let mut matches_json = Vec::new();
    for m in &exp.matches {
        let is_highlighted = Some(m.index) == active_match_idx;
        let escaped_source = escape_json(&m.source);
        let escaped_help = escape_json(&m.explanation);
        
        let block = format!(
            r#"    {{
      "index": {},
      "source": "{}",
      "explanation": "{}"
    }}"#,
            m.index, escaped_source, escaped_help
        );

        if is_highlighted {
            // Light it up in red (\x1b[31;1m ... \x1b[0m)
            matches_json.push(format!("\x1b[31;1m{}\x1b[0m", block));
        } else {
            matches_json.push(block);
        }
    }

    format!(
        r#"{{
  "command": "{}"{},
  "matches": [
{}
  ]
}}"#,
        escape_json(&exp.command),
        meta_json,
        matches_json.join(",\n")
    )
}
