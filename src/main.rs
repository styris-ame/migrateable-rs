use std::process::Command;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(|s| s.as_str()) {
        Some("lock") => {
            let overwrite = args.iter().any(|a| a == "--overwrite");
            run_lock(overwrite);
        }
        _ => {
            eprintln!("Usage: migrateable lock [--overwrite]");
            std::process::exit(1);
        }
    }
}

struct LockEntry {
    file: String,
    name: String,
    hash: String,
}

fn run_lock(overwrite: bool) {
    eprintln!("Running cargo test to discover locked hashes...");
    let output = Command::new("cargo")
        .args(["test", "--features", "migrateable/__lock_discover", "--tests", "migrateable_locked_hash", "--", "--nocapture"])
        .output()
        .expect("Failed to run cargo test");

    let combined = format!("{}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr),);

    let entries: Vec<LockEntry> = combined
        .lines()
        .filter_map(|line| {
            let marker_pos = line.find("MIGRATEABLE_LOCK:")?;
            let rest = &line[marker_pos + "MIGRATEABLE_LOCK:".len()..];
            let hash_start = rest.len().checked_sub(34)?;
            if &rest[hash_start - 1..hash_start] != ":" {
                return None;
            }
            let hash = &rest[hash_start..];
            let before = &rest[..hash_start - 1];
            let colon = before.rfind(':')?;
            Some(LockEntry {
                file: before[..colon].to_string(),
                name: before[colon + 1..].to_string(),
                hash: hash.to_string(),
            })
        })
        .collect();

    if entries.is_empty() {
        if !output.status.success() {
            eprintln!("cargo test failed (exit {}):", output.status);
            eprint!("{}", String::from_utf8_lossy(&output.stderr));
        } else {
            eprintln!("No Locked types found.");
        }
        return;
    }

    for entry in &entries {
        match update_file(&entry.file, &entry.name, &entry.hash, overwrite) {
            Ok(true) => eprintln!("Updated `{}` in {}", entry.name, entry.file),
            Ok(false) => eprintln!("Skipped `{}` in {} (already locked, use --overwrite)", entry.name, entry.file),
            Err(e) => eprintln!("Error updating `{}` in {}: {}", entry.name, entry.file, e),
        }
    }
}

fn update_file(path: &str, struct_name: &str, hash: &str, overwrite: bool) -> Result<bool, String> {
    let content = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let lines: Vec<&str> = content.lines().collect();

    let struct_pattern = format!("struct {struct_name}");
    let enum_pattern = format!("enum {struct_name}");
    let struct_idx = lines
        .iter()
        .position(|l| contains_word_boundary(l, &struct_pattern) || contains_word_boundary(l, &enum_pattern))
        .ok_or_else(|| format!("Could not find type `{struct_name}` in {path}"))?;

    let mut locked_line_idx = None;
    for i in (0..struct_idx).rev() {
        let trimmed = lines[i].trim();
        if trimmed.starts_with("#[locked(") && trimmed.ends_with(")]") {
            locked_line_idx = Some(i);
            break;
        }
        if !trimmed.starts_with("#[") && !trimmed.starts_with("//") && !trimmed.is_empty() {
            break;
        }
    }

    let indent: String = lines[struct_idx].chars().take_while(|c| c.is_whitespace()).collect();

    let mut new_lines: Vec<String> = lines.iter().map(|l| l.to_string()).collect();

    if let Some(idx) = locked_line_idx {
        if !overwrite {
            return Ok(false);
        }
        let existing = lines[idx].trim();
        let msg = existing
            .strip_prefix("#[locked(")
            .and_then(|s| s.strip_suffix(")]"))
            .and_then(|inner| inner.find(',').map(|pos| inner[pos..].to_string()));
        let new_attr = match msg {
            Some(msg_part) => format!("{indent}#[locked({hash}{msg_part})]"),
            None => format!("{indent}#[locked({hash})]"),
        };
        new_lines[idx] = new_attr;
    } else {
        new_lines.insert(struct_idx, format!("{indent}#[locked({hash})]"));
    }

    let mut result = new_lines.join("\n");
    if content.ends_with('\n') && !result.ends_with('\n') {
        result.push('\n');
    }
    std::fs::write(path, result).map_err(|e| e.to_string())?;
    Ok(true)
}

fn contains_word_boundary(line: &str, pattern: &str) -> bool {
    if let Some(pos) = line.find(pattern) {
        let after = pos + pattern.len();
        after >= line.len() || {
            let c = line.as_bytes()[after];
            !c.is_ascii_alphanumeric() && c != b'_'
        }
    } else {
        false
    }
}
