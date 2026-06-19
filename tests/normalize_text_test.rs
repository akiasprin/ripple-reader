// SPDX-License-Identifier: MIT OR Apache-2.0

//! Integration tests for the `ripple_reader::db` text normalizers.
//!
//! - [`normalize_text`] is the default normalizer: whitespace, CJK/alphanumeric
//!   spacing, and `)`-before-CJK spacing, while leaving quotes untouched.
//! - [`normalize_text_convert_quotes`] additionally converts English/curly double
//!   quotes to CJK corner brackets `「 」`. It is used only for summary and
//!   abstract fields.
//!
//! These tests pin down:
//!
//! 1. Body quotes outside HTML tags are converted to corner brackets only when
//!    using `normalize_text_convert_quotes`.
//! 2. Attribute quotes inside HTML tags (`<...>`) are preserved as ASCII `"`
//!    in both normalizers.
//! 3. A `)` followed by a CJK character gains a separating space.
//!
//! The two private helpers (`replace_double_quotes`,
//! `insert_space_after_close_paren`) cannot be reached from an external
//! test crate, but their behavior is fully observable through the public
//! `normalize_text_convert_quotes` entry point. The original test inputs contain
//! no characters that other passes would transform, so
//! `normalize_text_convert_quotes(x) == expected` holds iff the corresponding
//! helper does the right thing on `x`.

use ripple_reader::db::{normalize_text, normalize_text_convert_quotes};

// --- replace_double_quotes (via normalize_text_convert_quotes) ----------------

#[test]
fn replace_straight_quotes() {
    assert_eq!(
        normalize_text_convert_quotes(r#""compute-optimal""#),
        "\u{300C}compute-optimal\u{300D}"
    );
}

#[test]
fn replace_curly_quotes() {
    assert_eq!(
        normalize_text_convert_quotes("\u{201C}compute-optimal\u{201D}"),
        "\u{300C}compute-optimal\u{300D}"
    );
}

#[test]
fn mixed_quotes() {
    assert_eq!(
        normalize_text_convert_quotes(r#"称"计算最优"策略"#),
        "称\u{300C}计算最优\u{300D}策略"
    );
}

#[test]
fn already_corner_brackets_unchanged() {
    assert_eq!(
        normalize_text_convert_quotes("\u{300C}计算最优\u{300D}"),
        "\u{300C}计算最优\u{300D}"
    );
}

#[test]
fn normalize_text_convert_quotes_replaces_quotes() {
    assert_eq!(
        normalize_text_convert_quotes(r#"称"计算最优"策略"#),
        "称\u{300C}计算最优\u{300D}策略"
    );
}

// --- HTML attribute quote preservation (regression for the bug report) -----

/// The auto-appended insight footer looks like
/// `<div style="text-align:right; ...">（footer）</div>`. When a user re-saves
/// an insight that already contains this footer (via `POST /api/update`),
/// the attribute quotes must stay as ASCII `"` — otherwise the rendered
/// HTML is broken: `style=「text-align:right; …」`.
#[test]
fn html_attribute_quotes_preserved_with_convert_quotes() {
    let input = r#"正文末尾<div style="text-align:right">（footer）</div>继续"#;
    let out = normalize_text_convert_quotes(input);
    assert!(
        out.contains(r#"style="text-align:right""#),
        "attribute quotes must not be replaced with corner brackets, got: {out}"
    );
    assert!(out.contains("继续"), "trailing body text lost: {out}");
}

/// Body quotes outside a tag are still converted to 「 」 per the original
/// contract. Mixed input (tag + body) must apply the rule selectively.
#[test]
fn html_attribute_quotes_preserved_while_body_quotes_convert() {
    let input = r#"作者说"hello"在<code class="hl">var x = 1</code>里"#;
    let out = normalize_text_convert_quotes(input);
    assert!(
        out.contains(r#"class="hl""#),
        "code-tag attribute quotes lost: {out}"
    );
    assert!(
        out.contains('\u{300C}'),
        "body opening quote not converted: {out}"
    );
    assert!(
        out.contains('\u{300D}'),
        "body closing quote not converted: {out}"
    );
}

// --- insert_space_after_close_paren (via normalize_text) ---------------------

#[test]
fn space_after_close_paren_before_cjk() {
    assert_eq!(
        normalize_text("Qwen Team (Alibaba Group)完成了"),
        "Qwen Team (Alibaba Group) 完成了"
    );
}

#[test]
fn no_space_when_already_present() {
    // `insert_space_after_close_paren` must not double-insert when a space
    // already separates `)` from the following CJK char.
    assert_eq!(
        normalize_text("Qwen Team (Alibaba Group) 完成了"),
        "Qwen Team (Alibaba Group) 完成了"
    );
}

#[test]
fn no_space_before_ascii() {
    // `insert_space_after_close_paren` must skip when the next char is ASCII
    // (e.g. Latin word), not CJK.
    assert_eq!(normalize_text("foo (bar) baz"), "foo (bar) baz");
}

// --- normalize_text (no quote conversion) -------------------------------------

#[test]
fn normalize_text_keeps_straight_quotes() {
    assert_eq!(
        normalize_text(r#""compute-optimal""#),
        r#""compute-optimal""#
    );
}

#[test]
fn normalize_text_keeps_curly_quotes() {
    assert_eq!(
        normalize_text("\u{201C}compute-optimal\u{201D}"),
        "\u{201C}compute-optimal\u{201D}"
    );
}

#[test]
fn normalize_text_keeps_mixed_quotes() {
    assert_eq!(normalize_text(r#"称"计算最优"策略"#), r#"称"计算最优"策略"#);
}

#[test]
fn normalize_text_keeps_corner_brackets() {
    assert_eq!(
        normalize_text("\u{300C}计算最优\u{300D}"),
        "\u{300C}计算最优\u{300D}"
    );
}

#[test]
fn normalize_text_still_inserts_space_after_close_paren() {
    assert_eq!(
        normalize_text("Qwen Team (Alibaba Group)完成了"),
        "Qwen Team (Alibaba Group) 完成了"
    );
}

#[test]
fn normalize_text_html_attribute_quotes_unchanged() {
    let input = r#"正文末尾<div style="text-align:right">（footer）</div>继续"#;
    let out = normalize_text(input);
    assert!(
        out.contains(r#"style="text-align:right""#),
        "attribute quotes must stay unchanged, got: {out}"
    );
    assert!(out.contains("继续"), "trailing body text lost: {out}");
}
