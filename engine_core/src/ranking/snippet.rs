use aho_corasick::AhoCorasick;

pub fn generate_dynamic_snippet(
    content: &str,
    query_terms: &[String],
    target_len: usize,
) -> String {
    if content.is_empty() {
        return String::new();
    }

    let patterns: Vec<&str> = query_terms
        .iter()
        .map(|s| s.as_str())
        .filter(|s| s.len() > 1)
        .collect();
    if patterns.is_empty() {
        let snippet: String = content.chars().take(target_len).collect();
        return if content.len() > target_len {
            format!("{snippet}...")
        } else {
            snippet
        };
    }

    let ac = match AhoCorasick::builder()
        .ascii_case_insensitive(true)
        .build(&patterns)
    {
        Ok(aut) => aut,
        Err(_) => {
            let snippet: String = content.chars().take(target_len).collect();
            return format!("{snippet}...");
        }
    };

    let matches: Vec<(usize, usize)> = ac
        .find_iter(content)
        .map(|m| (m.start(), m.end()))
        .collect();

    if matches.is_empty() {
        let snippet: String = content.chars().take(target_len).collect();
        return if content.len() > target_len {
            format!("{snippet}...")
        } else {
            snippet
        };
    }

    // Locate the start of the window containing the densest cluster of matched tokens
    let mut best_start = matches[0].0;
    let mut max_density = 0;

    for (i, m) in matches.iter().enumerate() {
        let window_end = m.0 + target_len;
        let hits_in_window = matches[i..]
            .iter()
            .take_while(|next_m| next_m.0 < window_end)
            .count();

        if hits_in_window > max_density {
            max_density = hits_in_window;
            best_start = m.0;
        }
    }

    // Expand backwards slightly to avoid cutting off mid-word (safeguarding UTF-8 boundaries)
    let raw_start = best_start.saturating_sub(40);
    let start_offset = content.floor_char_boundary(raw_start);
    let safe_start = if start_offset > 0 {
        content[start_offset..best_start]
            .find(' ')
            .map(|off| start_offset + off + 1)
            .unwrap_or(start_offset)
    } else {
        0
    };

    let raw_end = (safe_start + target_len).min(content.len());
    let rough_end = content.ceil_char_boundary(raw_end);
    let safe_end = if rough_end < content.len() {
        content[rough_end..]
            .find(' ')
            .map(|off| rough_end + off)
            .unwrap_or(rough_end)
    } else {
        content.len()
    };
    let safe_end = safe_end.max(safe_start);

    let prefix = if safe_start > 0 { "..." } else { "" };
    let suffix = if safe_end < content.len() { "..." } else { "" };

    format!(
        "{}{}{}",
        prefix,
        content[safe_start..safe_end].trim(),
        suffix
    )
}
