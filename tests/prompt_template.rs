use ripple_reader::processor::{parse_prompt_file, PromptTemplate};

// ========================================================================
// PromptTemplate::render
// ========================================================================

#[test]
fn render_replaces_single_placeholder() {
    let template = "Hello, {name}!";
    let result = PromptTemplate::render(template, &[("name", "World")]);
    assert_eq!(result, "Hello, World!");
}

#[test]
fn render_replaces_multiple_placeholders() {
    let template = "Title: {title}\n\nContent:\n{content}";
    let result = PromptTemplate::render(template, &[("title", "Paper"), ("content", "Body")]);
    assert_eq!(result, "Title: Paper\n\nContent:\nBody");
}

#[test]
fn render_leaves_unknown_placeholders_intact() {
    let template = "Known: {known}, Unknown: {unknown}";
    let result = PromptTemplate::render(template, &[("known", "value")]);
    assert_eq!(result, "Known: value, Unknown: {unknown}");
}

#[test]
fn render_empty_template_returns_empty() {
    let result = PromptTemplate::render("", &[("key", "value")]);
    assert_eq!(result, "");
}

#[test]
fn render_no_placeholders_returns_unchanged() {
    let template = "Plain text without placeholders.";
    let result = PromptTemplate::render(template, &[("key", "value")]);
    assert_eq!(result, template);
}

#[test]
fn render_replaces_repeated_placeholder() {
    let template = "{x} and {x} again";
    let result = PromptTemplate::render(template, &[("x", "hi")]);
    assert_eq!(result, "hi and hi again");
}

// ========================================================================
// parse_prompt_file
// ========================================================================

#[test]
fn parse_basic_system_and_user() {
    let content = "[SYSTEM]\nBe helpful.\n\n[USER]\nQuestion: {question}";
    let pt = parse_prompt_file(content);

    assert_eq!(pt.system, "Be helpful.");
    assert_eq!(pt.user, "Question: {question}");
    assert!(pt.user_images.is_none());
    assert!(pt.image_system.is_none());
    assert!(pt.image_user.is_none());
}

#[test]
fn parse_all_sections() {
    let content = concat!(
        "[SYSTEM]\nSystem text.\n",
        "[USER]\nUser text.\n",
        "[USER_IMAGES]\nUser images text.\n",
        "[IMAGE_SYSTEM]\nImage system text.\n",
        "[IMAGE_USER]\nImage user text."
    );
    let pt = parse_prompt_file(content);

    assert_eq!(pt.system, "System text.");
    assert_eq!(pt.user, "User text.");
    assert_eq!(pt.user_images, Some("User images text.".to_string()));
    assert_eq!(pt.image_system, Some("Image system text.".to_string()));
    assert_eq!(pt.image_user, Some("Image user text.".to_string()));
}

#[test]
fn parse_empty_content_returns_empty() {
    let pt = parse_prompt_file("");
    assert_eq!(pt.system, "");
    assert_eq!(pt.user, "");
    assert!(pt.user_images.is_none());
}

#[test]
fn parse_no_markers_returns_empty_system() {
    // Content without any section markers should not populate any field
    let pt = parse_prompt_file("Just some random text.");
    assert_eq!(pt.system, "");
    assert_eq!(pt.user, "");
}

#[test]
fn parse_preserves_indentation_within_section() {
    let content = "[SYSTEM]\n  Indented line 1\n  Indented line 2\n\n[USER]\n{query}";
    let pt = parse_prompt_file(content);

    assert_eq!(pt.system, "Indented line 1\n  Indented line 2");
    assert_eq!(pt.user, "{query}");
}

#[test]
fn parse_only_system_section() {
    let content = "[SYSTEM]\nOnly system.";
    let pt = parse_prompt_file(content);

    assert_eq!(pt.system, "Only system.");
    assert_eq!(pt.user, "");
}

// ========================================================================
// Integration: parse then render
// ========================================================================

#[test]
fn parse_and_render_roundtrip() {
    let content = "[SYSTEM]\nYou are a {role}.\n\n[USER]\nTell me about {topic}.";
    let pt = parse_prompt_file(content);

    let system = PromptTemplate::render(&pt.system, &[("role", "scientist")]);
    let user = PromptTemplate::render(&pt.user, &[("topic", "physics")]);

    assert_eq!(system, "You are a scientist.");
    assert_eq!(user, "Tell me about physics.");
}

#[test]
fn parse_multiline_system_with_blank_lines() {
    let content = "[SYSTEM]\nLine one.\n\nLine two after blank.\n\n[USER]\nAsk: {q}";
    let pt = parse_prompt_file(content);

    assert_eq!(pt.system, "Line one.\n\nLine two after blank.");
    assert_eq!(pt.user, "Ask: {q}");
}
