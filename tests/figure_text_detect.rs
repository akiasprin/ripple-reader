use ripple_reader::figure::text_detect::infer_text_figure_body;
use ripple_reader::mineru::LayoutParaBlock;

fn make_block_from_json(json: &str) -> LayoutParaBlock {
    serde_json::from_str(json).unwrap()
}

/// Simulate page 18 of 2605.28303 where two qualitative-comparison galleries
/// (Figure 8 and Figure 9) sit on the same page with a ~30 pt gap between
/// them.  The old code put both into one cluster, so Figure 9's body
/// swallowed Figure 8.
#[test]
fn test_infer_text_figure_body_splits_multiple_figures_on_same_page() {
    let blocks: Vec<LayoutParaBlock> = vec![
        // Figure 8 content (y ≈ 81–359)
        make_block_from_json(
            r#"{
                "type": "text",
                "bbox": [79, 81, 512, 359],
                "blocks": [{
                    "type": "text",
                    "bbox": [79, 81, 512, 359],
                    "lines": [
                        {"bbox": [80, 81, 395, 92], "spans": [{"type": "text", "content": "Gallery title"}]},
                        {"bbox": [80, 100, 347, 130], "spans": [{"type": "text", "content": "Edit target 1"}]},
                        {"bbox": [79, 143, 512, 185], "spans": [{"type": "text", "content": "Baseline"}]},
                        {"bbox": [79, 196, 512, 217], "spans": [{"type": "text", "content": "CODE"}]},
                        {"bbox": [79, 236, 512, 257], "spans": [{"type": "text", "content": "More text"}]},
                        {"bbox": [79, 276, 512, 317], "spans": [{"type": "text", "content": "Even more"}]},
                        {"bbox": [79, 328, 276, 338], "spans": [{"type": "text", "content": "Final line"}]},
                        {"bbox": [79, 338, 512, 359], "spans": [{"type": "text", "content": "Last content"}]}
                    ]
                }]
            }"#,
        ),
        // Figure 8 caption (y ≈ 383–420)
        make_block_from_json(
            r#"{
                "type": "text",
                "bbox": [67, 383, 525, 420],
                "blocks": [{
                    "type": "text",
                    "bbox": [67, 383, 525, 420],
                    "lines": [
                        {"bbox": [67, 383, 525, 420], "spans": [{"type": "text", "content": "Figure 8: Qualitative Comparison"}]}
                    ]
                }]
            }"#,
        ),
        // Figure 9 content (y ≈ 449–701)
        make_block_from_json(
            r#"{
                "type": "text",
                "bbox": [79, 449, 512, 701],
                "blocks": [{
                    "type": "text",
                    "bbox": [79, 449, 512, 701],
                    "lines": [
                        {"bbox": [80, 449, 353, 460], "spans": [{"type": "text", "content": "Gallery title 2"}]},
                        {"bbox": [80, 468, 437, 480], "spans": [{"type": "text", "content": "Edit target 2"}]},
                        {"bbox": [80, 501, 129, 510], "spans": [{"type": "text", "content": "Generation:"}]},
                        {"bbox": [79, 511, 512, 532], "spans": [{"type": "text", "content": "The answer is James Henry Breasted."}]},
                        {"bbox": [79, 532, 512, 551], "spans": [{"type": "text", "content": "1. James Henry Breasted: He was an American Egyptologist."}]},
                        {"bbox": [79, 551, 512, 561], "spans": [{"type": "text", "content": "2. Singularity University (SU): This organization was founded in 2008."}]},
                        {"bbox": [79, 561, 512, 581], "spans": [{"type": "text", "content": "3. Peter Diamandis: He is an engineer, physician."}]},
                        {"bbox": [79, 581, 512, 601], "spans": [{"type": "text", "content": "4. Ray Kurzweil: He is an American author, inventor."}]},
                        {"bbox": [79, 601, 510, 612], "spans": [{"type": "text", "content": "Therefore, the correct answer is that Singularity University."}]},
                        {"bbox": [80, 617, 434, 629], "spans": [{"type": "text", "content": "Edit Target: What is the country of citizenship of Kelly McGillis?"}]},
                        {"bbox": [80, 630, 397, 642], "spans": [{"type": "text", "content": "Prompt: What is the country of citizenship of Kelly McGillis?"}]},
                        {"bbox": [80, 650, 129, 660], "spans": [{"type": "text", "content": "Generation:"}]},
                        {"bbox": [79, 661, 397, 671], "spans": [{"type": "text", "content": "The answer is Austria. However, this is incorrect."}]},
                        {"bbox": [79, 671, 512, 691], "spans": [{"type": "text", "content": "1. Kelly McGills is an American actress. She was born on March 25, 1951."}]},
                        {"bbox": [79, 691, 367, 701], "spans": [{"type": "text", "content": "Therefore, the country of citizenship of Kelly McGillis is the United States."}]}
                    ]
                }]
            }"#,
        ),
        // Figure 9 caption (y ≈ 725–761)
        make_block_from_json(
            r#"{
                "type": "text",
                "bbox": [67, 725, 525, 761],
                "blocks": [{
                    "type": "text",
                    "bbox": [67, 725, 525, 761],
                    "lines": [
                        {"bbox": [67, 725, 525, 761], "spans": [{"type": "text", "content": "Figure 9: Examples"}]}
                    ]
                }]
            }"#,
        ),
    ];

    let page_blocks: Vec<&LayoutParaBlock> = blocks.iter().collect();
    let page_width = 595.0;
    let fill_bboxes: Vec<[f32; 4]> = vec![];

    // ---- Figure 8 ----
    let caption8 = [67.0, 383.0, 525.0, 420.0];
    let (body8, _, reason8) = infer_text_figure_body(
        caption8,
        "Figure 8: ...",
        &page_blocks,
        page_width,
        &fill_bboxes,
    )
    .expect("Figure 8 should be detected");
    assert!(
        body8[3] < 449.0,
        "Figure 8 body bottom {} should be below Figure 9 content top 449 (reason={})",
        body8[3],
        reason8
    );

    // ---- Figure 9 ----
    let caption9 = [67.0, 725.0, 525.0, 761.0];
    let (body9, _, reason9) = infer_text_figure_body(
        caption9,
        "Figure 9: ...",
        &page_blocks,
        page_width,
        &fill_bboxes,
    )
    .expect("Figure 9 should be detected");
    assert!(
        body9[1] > 420.0,
        "Figure 9 body top {} should be above Figure 8 caption bottom 420 (reason={})",
        body9[1],
        reason9
    );
}
