use ripple_reader::mdfmt::parser::{has_layout, parse_images, rebuild_image, ImageRef};

#[test]
fn test_parse_simple() {
    let text = "![desc](figure/foo.png)";
    let imgs = parse_images(text);
    assert_eq!(imgs.len(), 1);
    assert_eq!(imgs[0].alt, "desc");
    assert_eq!(imgs[0].url, "figure/foo.png");
    assert!(!has_layout(&imgs[0]));
}

#[test]
fn test_parse_with_size() {
    let text = "![desc](figure/foo.png =80%)";
    let imgs = parse_images(text);
    assert_eq!(imgs.len(), 1);
    assert_eq!(imgs[0].size.as_deref(), Some("80%"));
    assert!(has_layout(&imgs[0]));
}

#[test]
fn test_parse_with_size_and_align() {
    let text = "![desc](figure/foo.png =80% center)";
    let imgs = parse_images(text);
    assert_eq!(imgs.len(), 1);
    assert_eq!(imgs[0].size.as_deref(), Some("80%"));
    assert_eq!(imgs[0].align.as_deref(), Some("center"));
}

#[test]
fn test_rebuild() {
    let img = ImageRef {
        full: "![desc](figure/foo.png)".into(),
        alt: "desc".into(),
        url: "figure/foo.png".into(),
        size: None,
        align: None,
        offset: 0,
    };
    assert_eq!(
        rebuild_image(&img, Some("50%"), Some("right")),
        "![desc](figure/foo.png =50% right)"
    );
}
