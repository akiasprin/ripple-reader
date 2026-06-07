use ripple_reader::env_editor::{
    read_env_file, read_providers, write_env_file, write_provider, ProviderConfig,
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
