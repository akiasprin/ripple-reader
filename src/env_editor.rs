// SPDX-License-Identifier: MIT OR Apache-2.0

//! Read/write `.env` files with in-place value updates (preserving comments and formatting).

use anyhow::{Context, Result};
use std::collections::HashMap;

/// Provider configuration fields.
#[derive(Debug, Clone, Default)]
pub struct ProviderConfig {
    pub name: String,
    pub provider_type: String,
    pub base_url: String,
    pub api_keys: Vec<String>,
    pub model: String,
    pub max_tokens: String,
    pub reasoning_effort: String,
    pub user_agent: String,
    pub is_digest: String,
    pub is_comment: String,
    pub enabled: String,
}

impl ProviderConfig {
    /// Generate all env key-value pairs for this provider.
    pub fn to_env_map(&self, index: usize) -> HashMap<String, String> {
        let mut map = HashMap::new();
        let prefix = format!("INSIGHT_PROVIDER_{}", index);
        if !self.name.is_empty() {
            map.insert(format!("{}_NAME", prefix), self.name.clone());
        }
        if !self.provider_type.is_empty() {
            map.insert(format!("{}_TYPE", prefix), self.provider_type.clone());
        }
        if !self.base_url.is_empty() {
            map.insert(format!("{}_BASE_URL", prefix), self.base_url.clone());
        }
        // Write multiple API keys as _API_KEY_1, _API_KEY_2, ...
        // Always clear the singular _API_KEY to avoid conflicts
        map.insert(format!("{}_API_KEY", prefix), String::new());
        for (ki, key) in self.api_keys.iter().enumerate() {
            if !key.is_empty() {
                map.insert(format!("{}_API_KEY_{}", prefix, ki + 1), key.clone());
            }
        }
        if !self.model.is_empty() {
            map.insert(format!("{}_MODEL", prefix), self.model.clone());
        }
        if !self.max_tokens.is_empty() {
            map.insert(format!("{}_MAX_TOKENS", prefix), self.max_tokens.clone());
        }
        // Always write reasoning_effort (even if empty) so clearing the field
        // in the admin UI actually removes the env key on save.
        map.insert(
            format!("{}_REASONING_EFFORT", prefix),
            self.reasoning_effort.clone(),
        );
        if !self.user_agent.is_empty() {
            map.insert(format!("{}_USER_AGENT", prefix), self.user_agent.clone());
        }
        if !self.is_digest.is_empty() {
            map.insert(format!("{}_IS_DIGEST", prefix), self.is_digest.clone());
        }
        if !self.is_comment.is_empty() {
            map.insert(format!("{}_IS_COMMENT", prefix), self.is_comment.clone());
        }
        if !self.enabled.is_empty() {
            map.insert(format!("{}_ENABLED", prefix), self.enabled.clone());
        }
        map
    }
}

/// Read `.env` file and parse all `KEY=value` pairs.
/// Comment lines and empty lines are ignored.
pub fn read_env_file(path: &str) -> Result<HashMap<String, String>> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read .env file: {}", path))?;
    let mut map = HashMap::new();
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if let Some(eq_pos) = trimmed.find('=') {
            let key = trimmed[..eq_pos].trim().to_string();
            let value = trimmed[eq_pos + 1..].trim().to_string();
            // Strip quotes if value is quoted
            let value = if (value.starts_with('"') && value.ends_with('"'))
                || (value.starts_with('\'') && value.ends_with('\''))
            {
                value[1..value.len() - 1].to_string()
            } else {
                value
            };
            map.insert(key, value);
        }
    }
    Ok(map)
}

/// Quote a value for .env if it contains characters that would break parsing.
/// Values with spaces, `#`, quotes, or other special chars are wrapped in
/// double quotes, with internal `"` and `\` escaped.
fn quote_env_value(value: &str) -> String {
    if value.is_empty() {
        return String::new();
    }
    // Only simple chars don't need quoting
    if value
        .chars()
        .all(|c| c.is_alphanumeric() || c == '_' || c == '-' || c == '.' || c == '/' || c == ':')
    {
        return value.to_string();
    }
    // Double-quote, escaping `\` and `"` inside
    let escaped = value.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{}\"", escaped)
}

/// Write `.env` file, updating specified key values.
/// Preserves original comments, blank lines, and order of unchanged keys.
/// New keys are appended to the end of the file.
pub fn write_env_file(path: &str, updates: &HashMap<String, String>) -> Result<()> {
    let content = std::fs::read_to_string(path).unwrap_or_default();

    let mut new_lines: Vec<String> = Vec::new();
    let mut updated_keys = std::collections::HashSet::new();

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            new_lines.push(line.to_string());
            continue;
        }
        if let Some(eq_pos) = trimmed.find('=') {
            let key = trimmed[..eq_pos].trim();
            if let Some(new_value) = updates.get(key) {
                new_lines.push(format!("{}={}", key, quote_env_value(new_value)));
                updated_keys.insert(key.to_string());
            } else {
                new_lines.push(line.to_string());
            }
        } else {
            new_lines.push(line.to_string());
        }
    }

    // Append new keys not present in the file
    for (key, value) in updates {
        if !updated_keys.contains(key) {
            new_lines.push(format!("{}={}", key, quote_env_value(value)));
        }
    }

    let output = new_lines.join("\n") + "\n";
    std::fs::write(path, output).with_context(|| format!("Failed to write .env file: {}", path))?;
    Ok(())
}

/// Read all provider configs from .env.
pub fn read_providers(env_path: &str) -> Result<Vec<(usize, ProviderConfig)>> {
    let env = read_env_file(env_path)?;
    let mut providers = Vec::new();

    for i in 1..=10 {
        let name_key = format!("INSIGHT_PROVIDER_{}_NAME", i);
        if let Some(name) = env.get(&name_key) {
            if name.trim().is_empty() {
                continue;
            }
            let prefix = format!("INSIGHT_PROVIDER_{}", i);
            let get = |suffix: &str| -> String {
                env.get(&format!("{}_{}", prefix, suffix))
                    .cloned()
                    .unwrap_or_default()
            };
            // Collect multiple API keys.
            // Prefer numbered keys (API_KEY_1..100) and fall back to the singular
            // API_KEY only when no numbered keys exist. This matches config.rs.
            let mut api_keys = Vec::new();
            for ki in 1..=100 {
                let key = get(&format!("API_KEY_{}", ki));
                if !key.is_empty() {
                    api_keys.push(key);
                }
            }
            if api_keys.is_empty() {
                let single = get("API_KEY");
                if !single.is_empty() {
                    api_keys.push(single);
                }
            }
            providers.push((
                i,
                ProviderConfig {
                    name: name.clone(),
                    provider_type: get("TYPE"),
                    base_url: get("BASE_URL"),
                    api_keys,
                    model: get("MODEL"),
                    max_tokens: get("MAX_TOKENS"),
                    reasoning_effort: get("REASONING_EFFORT"),
                    user_agent: get("USER_AGENT"),
                    is_digest: get("IS_DIGEST"),
                    is_comment: get("IS_COMMENT"),
                    enabled: get("ENABLED"),
                },
            ));
        }
    }

    Ok(providers)
}

/// Add or update a provider.
/// For new providers, finds the first empty slot (1-10).
/// For updates, uses the provided index.
pub fn write_provider(
    env_path: &str,
    index: Option<usize>,
    provider: &ProviderConfig,
) -> Result<usize> {
    let idx = match index {
        Some(i) => i,
        None => {
            // Find first empty slot
            let env = read_env_file(env_path).unwrap_or_default();
            let mut found = None;
            for i in 1..=10 {
                let key = format!("INSIGHT_PROVIDER_{}_NAME", i);
                if !env.contains_key(&key)
                    || env.get(&key).map(|s| s.trim().is_empty()).unwrap_or(true)
                {
                    found = Some(i);
                    break;
                }
            }
            found.ok_or_else(|| anyhow::anyhow!("No available provider slot (max 10)"))?
        }
    };

    let updates = provider.to_env_map(idx);
    write_env_file(env_path, &updates)?;
    Ok(idx)
}

/// Delete a provider (clear all its fields).
pub fn delete_provider(env_path: &str, index: usize) -> Result<()> {
    let env = read_env_file(env_path).unwrap_or_default();
    let prefix = format!("INSIGHT_PROVIDER_{}", index);
    let mut updates = HashMap::new();
    let mut suffixes: Vec<String> = [
        "NAME",
        "TYPE",
        "BASE_URL",
        "API_KEY",
        "MODEL",
        "MAX_TOKENS",
        "REASONING_EFFORT",
        "USER_AGENT",
        "IS_DIGEST",
        "IS_COMMENT",
        "ENABLED",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    for ki in 1..=100 {
        suffixes.push(format!("API_KEY_{}", ki));
    }
    for suffix in suffixes {
        let key = format!("{}_{}", prefix, suffix);
        if env.contains_key(&key) {
            updates.insert(key, String::new());
        }
    }
    write_env_file(env_path, &updates)?;
    Ok(())
}

/// Reorder providers.
/// `order` is a list of original indices in the new desired order.
/// E.g. order = [3, 1, 2] means original provider 3 goes first, 1 second, 2 third.
pub fn reorder_providers(env_path: &str, order: &[usize]) -> Result<()> {
    let mut providers = read_providers(env_path)?;
    if order.len() != providers.len() {
        return Err(anyhow::anyhow!(
            "Order length ({}) does not match provider count ({})",
            order.len(),
            providers.len()
        ));
    }
    // Validate all indices in order exist
    let valid_indices: std::collections::HashSet<usize> =
        providers.iter().map(|(idx, _)| *idx).collect();
    for idx in order {
        if !valid_indices.contains(idx) {
            return Err(anyhow::anyhow!("Invalid provider index in order: {}", idx));
        }
    }

    // Build ordered list
    let mut ordered: Vec<(usize, ProviderConfig)> = Vec::with_capacity(providers.len());
    for idx in order {
        if let Some(pos) = providers.iter().position(|(i, _)| i == idx) {
            ordered.push(providers.remove(pos));
        }
    }

    // Step 1: clear all existing provider keys
    let env = read_env_file(env_path).unwrap_or_default();
    let mut clear_updates = HashMap::new();
    for i in 1..=10 {
        let prefix = format!("INSIGHT_PROVIDER_{}", i);
        let suffixes = [
            "NAME",
            "TYPE",
            "BASE_URL",
            "API_KEY",
            "MODEL",
            "MAX_TOKENS",
            "REASONING_EFFORT",
            "USER_AGENT",
            "IS_DIGEST",
            "IS_COMMENT",
            "ENABLED",
        ];
        for suffix in &suffixes {
            let key = format!("{}_{}", prefix, suffix);
            if env.contains_key(&key) {
                clear_updates.insert(key, String::new());
            }
        }
        for ki in 1..=100 {
            let key = format!("{}_API_KEY_{}", prefix, ki);
            if env.contains_key(&key) {
                clear_updates.insert(key, String::new());
            }
        }
    }
    if !clear_updates.is_empty() {
        write_env_file(env_path, &clear_updates)?;
    }

    // Step 2: write providers in new order
    for (new_idx, (_, provider)) in ordered.into_iter().enumerate() {
        let slot = new_idx + 1;
        let updates = provider.to_env_map(slot);
        write_env_file(env_path, &updates)?;
    }

    Ok(())
}

/// Get `.env` file path.
/// Returns `.env` if it exists, otherwise `.env.example` (read-only scenario).
pub fn env_path() -> String {
    if std::path::Path::new(".env").exists() {
        ".env".to_string()
    } else {
        ".env.example".to_string()
    }
}
