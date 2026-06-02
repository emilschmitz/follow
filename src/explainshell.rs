use anyhow::Result;


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
/// Tries the local instance at localhost:5000 first, and falls back to explainshell.com.
pub fn fetch_html(cmd: &str) -> Result<String> {
    let encoded = url_encode(cmd);
    
    // Try localhost:5000 first
    let local_url = format!("http://localhost:5000/explain?cmd={}", encoded);
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
fn strip_html_tags(html: &str) -> String {
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

/// Parse the explainshell HTML into a clean, pretty-printed JSON string.
pub fn parse_html_to_json(cmd: &str, html: &str) -> String {
    // Check for common error pages first
    if html.contains("missing man page") {
        return format!(
            r#"{{
  "command": "{}",
  "error": "missing man page"
}}"#,
            escape_json(cmd)
        );
    }
    if html.contains("parsing error") {
        return format!(
            r#"{{
  "command": "{}",
  "error": "parsing error"
}}"#,
            escape_json(cmd)
        );
    }

    // 1. Parse spans from <div id="command">...</div>
    let mut spans = Vec::new();
    if let Some(cmd_start) = html.find("id=\"command\"") {
        if let Some(cmd_fragment_start) = html[cmd_start..].find('>') {
            let abs_start = cmd_start + cmd_fragment_start + 1;
            if let Some(cmd_end) = html[abs_start..].find("</div>") {
                let fragment = &html[abs_start..abs_start + cmd_end];
                let mut search_idx = 0;
                while let Some(span_start) = fragment[search_idx..].find("helpref=\"") {
                    let start_pos = search_idx + span_start + 9;
                    if let Some(ref_end) = fragment[start_pos..].find('"') {
                        let ref_id = &fragment[start_pos..start_pos + ref_end];
                        if let Some(tag_end) = fragment[start_pos + ref_end..].find('>') {
                            let content_start = start_pos + ref_end + tag_end + 1;
                            if let Some(span_end) = fragment[content_start..].find("</span>") {
                                let inner_html = &fragment[content_start..content_start + span_end];
                                let clean_text = strip_html_tags(inner_html);
                                spans.push((clean_text, ref_id.to_string()));
                                search_idx = content_start + span_end + 7;
                                continue;
                            }
                        }
                    }
                    search_idx += span_start + 9;
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
                    let clean_text = strip_html_tags(inner_html);
                    help_boxes.push((id.to_string(), clean_text));
                    search_idx = content_start + div_end + 6;
                    continue;
                }
            }
        }
        search_idx += div_start + 26;
    }

    // 3. Assemble JSON manually
    let mut matches_json = Vec::new();
    for (span_text, ref_id) in &spans {
        if let Some((_, help_text)) = help_boxes.iter().find(|(h_id, _)| h_id == ref_id) {
            let escaped_span = escape_json(span_text);
            let escaped_help = escape_json(help_text);
            matches_json.push(format!(
                r#"    {{
      "source": "{}",
      "explanation": "{}"
    }}"#,
                escaped_span, escaped_help
            ));
        }
    }

    // If we couldn't match spans (e.g. different HTML structure), just list the help boxes!
    if matches_json.is_empty() {
        for (id, help_text) in &help_boxes {
            let escaped_id = escape_json(id);
            let escaped_help = escape_json(help_text);
            matches_json.push(format!(
                r#"    {{
      "source": "{}",
      "explanation": "{}"
    }}"#,
                escaped_id, escaped_help
            ));
        }
    }

    format!(
        r#"{{
  "command": "{}",
  "matches": [
{}
  ]
}}"#,
        escape_json(cmd),
        matches_json.join(",\n")
    )
}
