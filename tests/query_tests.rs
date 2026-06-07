use ripple_reader::query::build_query;

#[test]
fn test_build_query_with_explicit_query() {
    let q = build_query("cat:cs.AI", &[], &[]);
    assert_eq!(q, "(cat:cs.AI)");
}

#[test]
fn test_build_query_with_categories() {
    let q = build_query("", &["cs.AI".to_string(), "cs.CL".to_string()], &[]);
    assert_eq!(q, "cat:(cs.AI OR cs.CL)");
}

#[test]
fn test_build_query_with_keywords() {
    let q = build_query(
        "",
        &[],
        &[
            "transformer".to_string(),
            "large language model".to_string(),
        ],
    );
    assert_eq!(q, "(all:transformer OR all:\"large language model\")");
}

#[test]
fn test_build_query_with_categories_and_keywords() {
    let q = build_query("", &["cs.AI".to_string()], &["llm".to_string()]);
    assert_eq!(q, "cat:(cs.AI) AND (all:llm)");
}

#[test]
fn test_build_query_explicit_query_overrides_categories() {
    let q = build_query(
        "cat:cs.AI OR cat:cs.CL",
        &["cs.LG".to_string()],
        &["transformer".to_string()],
    );
    assert_eq!(q, "(cat:cs.AI OR cat:cs.CL) AND (all:transformer)");
}

#[test]
fn test_build_query_empty() {
    let q = build_query("", &[], &[]);
    assert_eq!(q, "");
}
