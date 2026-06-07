use ripple_reader::web::insight_html::render_insight_html;

#[test]
fn test_render_insight_html_preserves_algorithm_content() {
    let insight = r#"## 标题

\begin{algorithm}
\caption{Deep Q-learning with Experience Replay}
\begin{algorithmic}
\STATE Initialize replay memory $\mathcal{D}$ to capacity $N$
\STATE Initialize action-value function $Q$ with random weights
\end{algorithmic}
\end{algorithm}

一些正文。"#;

    let resp = render_insight_html("arxiv", "test", insight);
    println!("HTML:\n{}", resp.html);

    assert!(
        resp.html.contains(r"\begin{algorithm}"),
        "should contain backslash begin algorithm"
    );
    assert!(
        resp.html.contains(r"\end{algorithm}"),
        "should contain backslash end algorithm"
    );
    assert!(resp.html.contains("caption"), "should contain caption");
    assert!(resp.html.contains("STATE"), "should contain STATE");
    assert!(resp.html.contains(r"$\mathcal{D}$"), "should contain math");
}

#[test]
fn test_render_insight_html_empty_input() {
    let resp = render_insight_html("arxiv", "test", "");
    assert!(
        resp.html.is_empty(),
        "empty insight should produce empty html"
    );
    assert!(
        resp.toc.is_empty(),
        "empty insight should produce empty toc"
    );
}

#[test]
fn test_render_insight_html_math_deferred() {
    let insight = r#"行内公式 $E=mc^2$ 和显示公式

$$\int_0^\infty e^{-x} dx = 1$$

结束。"#;

    let resp = render_insight_html("arxiv", "test", insight);
    println!("HTML:\n{}", resp.html);

    assert!(
        resp.html.contains(r#"class="math-deferred""#),
        "should produce math-deferred elements"
    );
    assert!(
        resp.html.contains(r#"data-latex="E=mc^2""#),
        "inline math should be preserved in data-latex"
    );
    assert!(
        resp.html.contains(r#"data-display="true""#),
        "display math should have data-display=true"
    );
    assert!(
        resp.html.contains(r#"data-display="false""#),
        "inline math should have data-display=false"
    );
}

#[test]
fn test_render_insight_html_heading_numbers_and_toc() {
    let insight = r#"# 一级标题

第一段。

## 二级标题 A

第二段。

### 三级标题

第三段。

## 二级标题 B

第四段。"#;

    let resp = render_insight_html("arxiv", "test", insight);
    println!("HTML:\n{}", resp.html);
    println!("TOC len: {}", resp.toc.len());

    assert!(
        resp.html.contains(r#"data-heading-num="1""#),
        "h1 should have numbering"
    );
    assert!(
        resp.html.contains(r#"data-heading-num="1.1""#),
        "first h2 should have numbering 1.1"
    );
    assert!(
        resp.html.contains(r#"data-heading-num="1.1.1""#),
        "h3 should have numbering 1.1.1"
    );
    assert!(
        resp.html.contains(r#"data-heading-num="1.2""#),
        "second h2 should have numbering 1.2"
    );

    assert_eq!(resp.toc.len(), 4, "toc should have 4 items");
    assert_eq!(resp.toc[0].num, "1");
    assert_eq!(resp.toc[0].level, 1);
    assert_eq!(resp.toc[1].num, "1.1");
    assert_eq!(resp.toc[1].level, 2);
    assert_eq!(resp.toc[2].num, "1.1.1");
    assert_eq!(resp.toc[2].level, 3);
    assert_eq!(resp.toc[3].num, "1.2");
    assert_eq!(resp.toc[3].level, 2);
}
