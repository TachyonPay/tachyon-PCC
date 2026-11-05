//! Load a `.env` file into the process environment without logging values.

use std::fs;
use std::path::{Path, PathBuf};

/// Walk from `start` toward root looking for `.env`. Returns the first existing one.
pub fn find_dotenv(start: &Path) -> Option<PathBuf> {
    let mut dir = start.to_path_buf();
    loop {
        let candidate = dir.join(".env");
        if candidate.is_file() {
            return Some(candidate);
        }
        if !dir.pop() {
            return None;
        }
    }
}

/// Set variables that are not already in the environment. Never prints values.
pub fn load_dotenv(path: &Path) -> Result<usize, String> {
    let text = fs::read_to_string(path).map_err(|_| "could not read .env".to_owned())?;
    let mut n = 0;
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        let key = k.trim();
        if key.is_empty() {
            continue;
        }
        if std::env::var_os(key).is_some() {
            continue;
        }
        let val = v.trim();
        let val = if val.len() >= 2
            && ((val.starts_with('"') && val.ends_with('"')) || (val.starts_with('\'') && val.ends_with('\'')))
        {
            &val[1..val.len() - 1]
        } else {
            val
        };
        std::env::set_var(key, val);
        n += 1;
    }
    Ok(n)
}
