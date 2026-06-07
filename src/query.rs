// SPDX-License-Identifier: MIT OR Apache-2.0

use tracing::info;

pub fn build_query(arxiv_query: &str, categories: &[String], keywords: &[String]) -> String {
    let base_query = if !arxiv_query.is_empty() {
        info!("[query] Using ARXIV_QUERY as base: '{}'", arxiv_query);
        format!("({})", arxiv_query)
    } else if !categories.is_empty() {
        info!(
            "[query] Building query from ARXIV_CATEGORIES: {:?}",
            categories
        );
        format!("cat:({})", categories.join(" OR "))
    } else {
        info!("[query] No base query categories provided");
        String::new()
    };
    let kw_part = if keywords.is_empty() {
        info!("[query] No keywords provided");
        String::new()
    } else {
        info!("[query] Building keyword query from: {:?}", keywords);
        let kw_terms: Vec<String> = keywords
            .iter()
            .map(|k| {
                if k.contains(' ') {
                    format!("all:\"{}\"", k)
                } else {
                    format!("all:{}", k)
                }
            })
            .collect();
        format!("({})", kw_terms.join(" OR "))
    };
    if base_query.is_empty() {
        kw_part
    } else if kw_part.is_empty() {
        base_query
    } else {
        format!("{} AND {}", base_query, kw_part)
    }
}
