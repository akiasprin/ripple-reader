use ripple_reader::figure::svg::{extract_svg_clip_bboxes, parse_path_coords, parse_svg_matrix};
use ripple_reader::figure::text_detect::{has_numbered_label, has_structure_label};

/// Sample mutool SVG snippet with a single clipPath containing a rectangle path.
const SAMPLE_SVG: &str = r#"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg">
<defs>
<clipPath id="clip_5">
  <path transform="matrix(.47688,0,0,-.47688,220.34803,436.00239)"
        d="M214.232 159.391H603.284V453.141H214.232Z"/>
</clipPath>
</defs>
</svg>
"#;

#[test]
fn test_extract_svg_clip_bboxes_finds_clip_5() {
    let bboxes = extract_svg_clip_bboxes(SAMPLE_SVG);
    assert!(
        !bboxes.is_empty(),
        "expected at least one clipPath bbox, got none"
    );

    // clip_5 should produce roughly (322.5, 219.9, 508.0, 360.0)
    let bbox = bboxes[0];
    assert!((bbox[0] - 322.5).abs() < 1.0, "x0 mismatch: {}", bbox[0]);
    assert!((bbox[1] - 219.9).abs() < 1.0, "y0 mismatch: {}", bbox[1]);
    assert!((bbox[2] - 508.0).abs() < 1.0, "x1 mismatch: {}", bbox[2]);
    assert!((bbox[3] - 360.0).abs() < 1.0, "y1 mismatch: {}", bbox[3]);
}

#[test]
fn test_parse_svg_matrix() {
    let m = parse_svg_matrix("matrix(.5,0,0,-.5,100,200)").unwrap();
    assert_eq!(m, [0.5, 0.0, 0.0, -0.5, 100.0, 200.0]);
}

#[test]
fn test_parse_path_coords_rect() {
    let coords = parse_path_coords("M0 0H960V540H0Z");
    assert_eq!(coords.len(), 4);
    assert_eq!(coords[0], (0.0, 0.0));
    assert_eq!(coords[1], (960.0, 0.0));
    assert_eq!(coords[2], (960.0, 540.0));
    assert_eq!(coords[3], (0.0, 540.0));
}

// ---- Text-figure detection tests ----

#[test]
fn test_has_structure_label_matches_system_user() {
    assert!(has_structure_label("[System] You are an expert..."));
    assert!(has_structure_label("[User] What is the answer?"));
    assert!(has_structure_label("Generation: The answer is..."));
    assert!(has_structure_label(
        "Output requirement: Think step by step"
    ));
    assert!(has_structure_label("Guidelines: 1. Be concise"));
    assert!(has_structure_label("Criteria for PASS: explicit support"));
    assert!(has_structure_label("Causal Scaffold: This is a test"));
    assert!(has_structure_label("Edit Target: Who founded X?"));
    assert!(has_structure_label(
        "Important task setting: counterfactual"
    ));
    // Without brackets (2605.10889 prompt figures)
    assert!(has_structure_label("SYSTEM You are a helpful assistant."));
    assert!(has_structure_label("USER question with instruction"));
    assert!(has_structure_label(
        "Demonstration This is a correct response"
    ));
    assert!(has_structure_label(
        "Summarized Demonstration condensed by model"
    ));
}

#[test]
fn test_has_structure_label_no_false_positives() {
    assert!(!has_structure_label(
        "This is a regular paragraph about systems."
    ));
    assert!(!has_structure_label("The user experience is important."));
    assert!(!has_structure_label(
        "Output quality depends on many factors."
    ));
    assert!(!has_structure_label("Figure 7 shows the results."));
}

#[test]
fn test_has_numbered_label_matches() {
    assert!(has_numbered_label("1. Baseline (AdaLoRA):"));
    assert!(has_numbered_label("2. CODE (Ours):"));
    assert!(has_numbered_label("1. What is Epistemic Dissonance?"));
    assert!(has_numbered_label("3. Output format"));
}

#[test]
fn test_has_numbered_label_rejects_non_matching() {
    assert!(!has_numbered_label("This is 1. not a label:"));
    assert!(!has_numbered_label("The answer is 42."));
    assert!(!has_numbered_label("[System] You are an expert"));
    assert!(!has_numbered_label("Figure 7 shows the results."));
}
