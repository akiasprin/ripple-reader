use ripple_reader::env_editor::{
    delete_provider, read_env_file, read_providers, remove_env_keys, reorder_providers,
    write_env_file, write_provider, ProviderConfig,
};
use std::collections::HashMap;
use std::io::Write;

#[test]
fn test_read_env_file() {
    let mut tmp = tempfile::NamedTempFile::new().unwrap();
    writeln!(tmp, "# comment").unwrap();
    writeln!(tmp, "KEY1=value1").unwrap();
    writeln!(tmp, "KEY2=\"quoted value\"").unwrap();
    writeln!(tmp, "").unwrap();
    writeln!(tmp, "KEY3='single quoted'").unwrap();

    let map = read_env_file(tmp.path().to_str().unwrap()).unwrap();
    assert_eq!(map.get("KEY1"), Some(&"value1".to_string()));
    assert_eq!(map.get("KEY2"), Some(&"quoted value".to_string()));
    assert_eq!(map.get("KEY3"), Some(&"single quoted".to_string()));
}

#[test]
fn test_write_env_file_preserves_comments() {
    let mut tmp = tempfile::NamedTempFile::new().unwrap();
    writeln!(tmp, "# comment line").unwrap();
    writeln!(tmp, "KEY1=old1").unwrap();
    writeln!(tmp, "").unwrap();
    writeln!(tmp, "KEY2=old2").unwrap();

    let mut updates = HashMap::new();
    updates.insert("KEY1".to_string(), "new1".to_string());
    updates.insert("KEY3".to_string(), "new3".to_string());

    write_env_file(tmp.path().to_str().unwrap(), &updates).unwrap();

    let content = std::fs::read_to_string(tmp.path()).unwrap();
    assert!(content.contains("# comment line"));
    assert!(content.contains("KEY1=new1"));
    assert!(content.contains("KEY2=old2"));
    assert!(content.contains("KEY3=new3"));
}

#[test]
fn test_provider_roundtrip() {
    let mut tmp = tempfile::NamedTempFile::new().unwrap();
    writeln!(tmp, "INSIGHT_PROVIDER_1_NAME=Test").unwrap();
    writeln!(tmp, "INSIGHT_PROVIDER_1_TYPE=openai").unwrap();
    writeln!(tmp, "INSIGHT_PROVIDER_1_BASE_URL=https://api.example.com").unwrap();

    let path = tmp.path().to_str().unwrap();
    let providers = read_providers(path).unwrap();
    assert_eq!(providers.len(), 1);
    assert_eq!(providers[0].1.name, "Test");

    let p = ProviderConfig {
        name: "Updated".to_string(),
        provider_type: "anthropic".to_string(),
        base_url: "https://new.example.com".to_string(),
        model: "claude-3".to_string(),
        ..Default::default()
    };
    let idx = write_provider(path, Some(1), &p).unwrap();
    assert_eq!(idx, 1);

    let providers = read_providers(path).unwrap();
    assert_eq!(providers[0].1.name, "Updated");
    assert_eq!(providers[0].1.provider_type, "anthropic");
}

#[test]
fn test_delete_provider_removes_keys_from_file() {
    let mut tmp = tempfile::NamedTempFile::new().unwrap();
    writeln!(tmp, "# Provider config").unwrap();
    writeln!(tmp, "INSIGHT_PROVIDER_1_NAME=OpenAI").unwrap();
    writeln!(tmp, "INSIGHT_PROVIDER_1_TYPE=openai").unwrap();
    writeln!(tmp, "INSIGHT_PROVIDER_1_BASE_URL=https://api.openai.com").unwrap();
    writeln!(tmp, "INSIGHT_PROVIDER_1_API_KEY=sk-old").unwrap();
    writeln!(tmp, "INSIGHT_PROVIDER_1_API_KEY_1=sk-first").unwrap();
    writeln!(tmp, "INSIGHT_PROVIDER_1_API_KEY_2=sk-second").unwrap();
    writeln!(tmp, "INSIGHT_PROVIDER_1_MODEL=gpt-4").unwrap();
    writeln!(tmp, "INSIGHT_PROVIDER_1_ENABLED=true").unwrap();
    writeln!(tmp, "").unwrap();
    writeln!(tmp, "INSIGHT_PROVIDER_2_NAME=Anthropic").unwrap();
    writeln!(tmp, "INSIGHT_PROVIDER_2_TYPE=anthropic").unwrap();

    let path = tmp.path().to_str().unwrap();
    delete_provider(path, 1).unwrap();

    let content = std::fs::read_to_string(path).unwrap();
    assert!(!content.contains("INSIGHT_PROVIDER_1_NAME="));
    assert!(!content.contains("INSIGHT_PROVIDER_1_API_KEY"));
    assert!(!content.contains("INSIGHT_PROVIDER_1_MODEL="));
    assert!(content.contains("# Provider config"));
    assert!(content.contains("INSIGHT_PROVIDER_2_NAME=Anthropic"));

    let providers = read_providers(path).unwrap();
    assert_eq!(providers.len(), 1);
    assert_eq!(providers[0].1.name, "Anthropic");
}

#[test]
fn test_delete_nonexistent_provider_is_noop() {
    let mut tmp = tempfile::NamedTempFile::new().unwrap();
    writeln!(tmp, "INSIGHT_PROVIDER_1_NAME=OpenAI").unwrap();
    writeln!(tmp, "INSIGHT_PROVIDER_1_API_KEY=sk-secret").unwrap();

    let path = tmp.path().to_str().unwrap();
    delete_provider(path, 99).unwrap();

    let content = std::fs::read_to_string(path).unwrap();
    assert!(content.contains("INSIGHT_PROVIDER_1_NAME=OpenAI"));
    assert!(content.contains("INSIGHT_PROVIDER_1_API_KEY=sk-secret"));
}

#[test]
fn test_remove_env_keys_preserves_comments_and_blank_lines() {
    let mut tmp = tempfile::NamedTempFile::new().unwrap();
    writeln!(tmp, "# comment").unwrap();
    writeln!(tmp, "KEY1=value1").unwrap();
    writeln!(tmp, "").unwrap();
    writeln!(tmp, "KEY2=value2").unwrap();
    writeln!(tmp, "# another comment").unwrap();
    writeln!(tmp, "KEY3=value3").unwrap();

    let path = tmp.path().to_str().unwrap();
    remove_env_keys(path, &["KEY2".to_string()]).unwrap();

    let content = std::fs::read_to_string(path).unwrap();
    assert!(content.contains("# comment"));
    assert!(content.contains("KEY1=value1"));
    assert!(!content.contains("KEY2=value2"));
    assert!(content.contains("# another comment"));
    assert!(content.contains("KEY3=value3"));
    assert!(content.lines().any(|l| l.trim().is_empty()));
}

#[test]
fn test_write_provider_removes_stale_numbered_api_keys() {
    // Simulate: provider has 3 API keys, user deletes the middle one and saves.
    // The old API_KEY_3 must be removed, not left behind.
    let mut tmp = tempfile::NamedTempFile::new().unwrap();
    writeln!(tmp, "INSIGHT_PROVIDER_1_NAME=OpenAI").unwrap();
    writeln!(tmp, "INSIGHT_PROVIDER_1_TYPE=openai").unwrap();
    writeln!(tmp, "INSIGHT_PROVIDER_1_BASE_URL=https://api.openai.com").unwrap();
    writeln!(tmp, "INSIGHT_PROVIDER_1_API_KEY=").unwrap();
    writeln!(tmp, "INSIGHT_PROVIDER_1_API_KEY_1=sk-first").unwrap();
    writeln!(tmp, "INSIGHT_PROVIDER_1_API_KEY_2=sk-second").unwrap();
    writeln!(tmp, "INSIGHT_PROVIDER_1_API_KEY_3=sk-third").unwrap();
    writeln!(tmp, "INSIGHT_PROVIDER_1_MODEL=gpt-4").unwrap();
    writeln!(tmp, "INSIGHT_PROVIDER_1_ENABLED=true").unwrap();

    let path = tmp.path().to_str().unwrap();

    // User deleted sk-second; only 2 keys remain.
    let p = ProviderConfig {
        name: "OpenAI".to_string(),
        provider_type: "openai".to_string(),
        base_url: "https://api.openai.com".to_string(),
        api_keys: vec!["sk-first".to_string(), "sk-third".to_string()],
        model: "gpt-4".to_string(),
        enabled: "true".to_string(),
        ..Default::default()
    };
    write_provider(path, Some(1), &p).unwrap();

    let providers = read_providers(path).unwrap();
    assert_eq!(providers.len(), 1);
    // Should have exactly 2 keys — the stale API_KEY_3 must be gone.
    assert_eq!(providers[0].1.api_keys, vec!["sk-first", "sk-third"]);

    let content = std::fs::read_to_string(path).unwrap();
    assert!(
        !content.contains("API_KEY_3="),
        "stale API_KEY_3 should be removed"
    );
}

#[test]
fn test_reorder_providers_removes_old_keys_and_reorders() {
    let mut tmp = tempfile::NamedTempFile::new().unwrap();
    writeln!(tmp, "INSIGHT_PROVIDER_1_NAME=First").unwrap();
    writeln!(tmp, "INSIGHT_PROVIDER_1_TYPE=openai").unwrap();
    writeln!(tmp, "INSIGHT_PROVIDER_1_API_KEY_1=sk-first").unwrap();
    writeln!(tmp, "INSIGHT_PROVIDER_2_NAME=Second").unwrap();
    writeln!(tmp, "INSIGHT_PROVIDER_2_TYPE=anthropic").unwrap();
    writeln!(tmp, "INSIGHT_PROVIDER_2_API_KEY_1=sk-second").unwrap();

    let path = tmp.path().to_str().unwrap();
    // Move original provider 2 to slot 1, original provider 1 to slot 2.
    reorder_providers(path, &[2, 1]).unwrap();

    let content = std::fs::read_to_string(path).unwrap();
    assert!(!content.contains("INSIGHT_PROVIDER_1_NAME=First"));
    assert!(!content.contains("INSIGHT_PROVIDER_2_NAME=Second"));

    let providers = read_providers(path).unwrap();
    assert_eq!(providers.len(), 2);
    assert_eq!(providers[0].0, 1);
    assert_eq!(providers[0].1.name, "Second");
    assert_eq!(providers[1].0, 2);
    assert_eq!(providers[1].1.name, "First");
}
