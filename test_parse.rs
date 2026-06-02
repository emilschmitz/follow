fn decode_html(s: &str) -> String {
    s.replace("&amp;", "&").replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&#39;", "'")
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

fn main() {
    let html = r#"<div id="command">
                    <span class="dropdown">
                    <span style="word-spacing: 0px;">
                    <b class="caret" data-toggle="dropdown"></b>
                    <span class="command0 simplecommandstart" helpref="help-3"><a href="/explain/1/gnutrue">true(1)</a></span>
                    <ul class="dropdown-menu" role="menu" aria-labelledby="dropdownMenu">
                      <li>other</li>
                    </ul>
                    </span>
                </span> <span class="shell" helpref="help-0">&amp;&amp;</span> <span class="shell" helpref="help-1">{</span>
    "#;
    let (f, spans) = parse_command_div(html);
    println!("Formatted: '{}'", f);
    for s in spans {
        println!("- span: {}-{}, help_id: {}, text: '{}'", s.0, s.1, s.2, &f.chars().skip(s.0).take(s.1 - s.0).collect::<String>());
    }
}
