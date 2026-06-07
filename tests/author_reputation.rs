use ripple_reader::author_reputation::compute_author_boost;
use ripple_reader::db::AuthorStats;

#[test]
fn test_compute_author_boost_unknown() {
    let stats = vec![(
        "Unknown Author".to_string(),
        AuthorStats {
            paper_count: 0,
            avg_score: 0.0,
            critical_count: 0,
            citation_count: None,
            h_index: None,
        },
    )];
    let boost = compute_author_boost(&stats, None);
    assert_eq!(boost, 0.0);
}

#[test]
fn test_compute_author_boost_jeff_dean() {
    let stats = vec![(
        "Jeff Dean".to_string(),
        AuthorStats {
            paper_count: 200,
            avg_score: 8.5,
            critical_count: 50,
            citation_count: Some(376000),
            h_index: Some(134),
        },
    )];
    let boost = compute_author_boost(&stats, None);
    assert!(
        boost > 1.0,
        "Jeff Dean should give a strong boost, got {}",
        boost
    );
}

#[test]
fn test_compute_author_boost_with_corresponding() {
    let first_three = vec![
        (
            "First Author".to_string(),
            AuthorStats {
                paper_count: 5,
                avg_score: 7.0,
                critical_count: 1,
                citation_count: Some(500),
                h_index: Some(8),
            },
        ),
        (
            "Second Author".to_string(),
            AuthorStats {
                paper_count: 10,
                avg_score: 7.5,
                critical_count: 2,
                citation_count: Some(2000),
                h_index: Some(15),
            },
        ),
    ];
    let corresponding = (
        "Senior Author".to_string(),
        AuthorStats {
            paper_count: 300,
            avg_score: 8.0,
            critical_count: 30,
            citation_count: Some(50000),
            h_index: Some(80),
        },
    );
    let boost = compute_author_boost(&first_three, Some(&corresponding));
    assert!(
        boost > 2.0,
        "Combined boost should exceed old clamp limit, got {}",
        boost
    );
}
