#!/usr/bin/env python3
"""Compare current MinerU postprocess results against a snapshot.

Usage:
    python3 scripts/compare_mineru_snapshot.py <snapshot_dir> <figures_dir>

Outputs:
    <snapshot_dir>/comparison_report.json
    <snapshot_dir>/comparison_report.txt   (when redirected)
"""

import hashlib
import json
import re
import sys
from collections import defaultdict
from pathlib import Path


CAPTION_RE = re.compile(r"\b(F|f|T|t)[Ii]?[Gg]?[Uu]?[Rr]?[Ee]?[Ss]?[:.\s]*(\d+)\b")
BBOX_THRESHOLD = 0.5


def extract_caption_number(desc: str) -> str | None:
    """Extract a normalised caption key like 'F:1' or 'T:2' from a description."""
    match = CAPTION_RE.search(desc)
    if not match:
        return None
    kind = match.group(1).upper()
    num = int(match.group(2))
    return f"{kind}:{num}"


def bbox_diff(a: list[float], b: list[float]) -> list[float]:
    """Element-wise absolute difference between two bbox arrays."""
    return [abs(x - y) for x, y in zip(a, b)]


def bbox_close(a: list[float], b: list[float], threshold: float = BBOX_THRESHOLD) -> bool:
    """Return True if all coordinate differences are below threshold."""
    if len(a) != len(b):
        return False
    return all(d < threshold for d in bbox_diff(a, b))


def hash_file(path: Path) -> str:
    """Return SHA-256 hex digest of a file's contents."""
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(65536), b""):
            h.update(chunk)
    return h.hexdigest()


def align_entries(old_entries: list[dict], new_entries: list[dict]) -> dict:
    """Align two lists of entries primarily by caption number, fallback to bbox/page."""
    old_by_cap = {}
    old_unmatched = []
    for i, e in enumerate(old_entries):
        cap = e.get("caption")
        if cap and cap not in old_by_cap:
            old_by_cap[cap] = (i, e)
        else:
            old_unmatched.append((i, e))

    new_by_cap = {}
    new_unmatched = []
    for i, e in enumerate(new_entries):
        cap = e.get("caption")
        if cap and cap not in new_by_cap:
            new_by_cap[cap] = (i, e)
        else:
            new_unmatched.append((i, e))

    pairs = []
    used_old = set()
    used_new = set()

    # First pass: caption-number matches
    for cap, (ni, ne) in new_by_cap.items():
        if cap in old_by_cap:
            oi, oe = old_by_cap[cap]
            pairs.append((oe, ne, oi, ni))
            used_old.add(oi)
            used_new.add(ni)

    # Second pass: fallback on (page_idx, bbox[0], bbox[1]) for unmatched
    def fallback_key(entry: dict) -> tuple:
        bbox = entry.get("bbox", [])
        return (entry.get("page_idx", -1), bbox[0] if bbox else -1, bbox[1] if len(bbox) > 1 else -1)

    old_unmatched_remaining = [(i, e) for i, e in old_unmatched if i not in used_old]
    new_unmatched_remaining = [(i, e) for i, e in new_unmatched if i not in used_new]

    old_by_fallback = {fallback_key(e): (i, e) for i, e in old_unmatched_remaining}
    for ni, ne in new_unmatched_remaining:
        key = fallback_key(ne)
        if key in old_by_fallback:
            oi, oe = old_by_fallback.pop(key)
            pairs.append((oe, ne, oi, ni))
            used_old.add(oi)
            used_new.add(ni)

    old_missing = [i for i, _ in enumerate(old_entries) if i not in used_old]
    new_added = [i for i, _ in enumerate(new_entries) if i not in used_new]

    return {
        "pairs": pairs,
        "old_missing": old_missing,
        "new_added": new_added,
    }


def compare_entry(old: dict, new: dict) -> list[dict]:
    """Compare two aligned entries and return a list of issue dicts."""
    issues = []

    old_desc = old.get("description", "").strip()
    new_desc = new.get("description", "").strip()
    if old_desc != new_desc:
        issues.append({
            "type": "description_changed",
            "old": old_desc[:120],
            "new": new_desc[:120],
        })

    if old.get("content_type") != new.get("content_type"):
        issues.append({
            "type": "content_type_changed",
            "old": old.get("content_type"),
            "new": new.get("content_type"),
        })

    if old.get("page_idx") != new.get("page_idx"):
        issues.append({
            "type": "page_changed",
            "old": old.get("page_idx"),
            "new": new.get("page_idx"),
        })

    old_bbox = old.get("bbox", [])
    new_bbox = new.get("bbox", [])
    if not bbox_close(old_bbox, new_bbox):
        issues.append({
            "type": "bbox_changed",
            "old": old_bbox,
            "new": new_bbox,
            "diff": bbox_diff(old_bbox, new_bbox),
        })

    old_body = old.get("body_bbox", [])
    new_body = new.get("body_bbox", [])
    if not bbox_close(old_body, new_body):
        issues.append({
            "type": "body_bbox_changed",
            "old": old_body,
            "new": new_body,
            "diff": bbox_diff(old_body, new_body),
        })

    return issues


def build_entries(data: dict) -> list[dict]:
    """Build a list of comparable entries from a CacheMeta dict."""
    entries = []
    for i, desc in enumerate(data.get("image_descriptions", [])):
        entry = {
            "index": i,
            "description": desc,
            "caption": extract_caption_number(desc),
            "content_type": data.get("image_bboxes", [])[i].get("content_type") if i < len(data.get("image_bboxes", [])) else None,
            "page_idx": data.get("image_bboxes", [])[i].get("page_idx") if i < len(data.get("image_bboxes", [])) else None,
            "bbox": data.get("image_bboxes", [])[i].get("bbox") if i < len(data.get("image_bboxes", [])) else None,
            "body_bbox": data.get("body_bboxes", [])[i].get("bbox") if i < len(data.get("body_bboxes", [])) else None,
        }
        entries.append(entry)
    return entries


def load_snapshot_hashes(snapshot_dir: Path) -> dict[str, set[str]]:
    """Load old image hashes grouped by paper id from image_hashes.txt."""
    hashes_path = snapshot_dir / "image_hashes.txt"
    by_paper: dict[str, set[str]] = defaultdict(set)
    if not hashes_path.exists():
        return by_paper
    with open(hashes_path) as f:
        for line in f:
            parts = line.strip().split()
            if len(parts) < 2:
                continue
            h = parts[0]
            rel_path = parts[1]
            # rel_path looks like figures/arxiv/<paper_id>/image.jpg
            segs = rel_path.split("/")
            if len(segs) >= 3:
                paper_id = segs[2]
                by_paper[paper_id].add(h)
    return by_paper


def load_new_hashes(figures_dir: Path, paper_id: str) -> set[str]:
    """Compute SHA-256 hashes for all .jpg/.png under the given paper directory."""
    paper_dir = figures_dir / "arxiv" / paper_id
    hashes = set()
    if not paper_dir.exists():
        return hashes
    for path in paper_dir.rglob("*"):
        if path.is_file() and path.suffix.lower() in {".jpg", ".jpeg", ".png"}:
            hashes.add(hash_file(path))
    return hashes


def log_issue_counts(log_path: Path) -> dict[str, int]:
    """Count actual log-level failure markers in mineru.log.

    Avoids false positives from caption text containing words like
    'error' (e.g. 'Training error') by matching line-level markers.
    """
    counts = {"failed": 0, "error": 0, "warn": 0}
    if not log_path.exists():
        return counts
    with open(log_path, "r", errors="ignore") as f:
        for line in f:
            lower = line.lower()
            # Match log-level markers such as [ERROR], ERROR:, or leading ERROR.
            if "[error]" in lower or " error:" in lower or lower.lstrip().startswith("error "):
                counts["error"] += 1
            if "[warn]" in lower or " warn:" in lower or lower.lstrip().startswith("warn "):
                counts["warn"] += 1
            if "[failed]" in lower or " failed:" in lower or lower.lstrip().startswith("failed "):
                counts["failed"] += 1
    return counts


def classify_paper(paper_report: dict) -> str:
    """Classify a single paper as identical, improved, regressed, or neutral."""
    issues = paper_report.get("issues", [])
    old_count = paper_report.get("old_entry_count", 0)
    new_count = paper_report.get("new_entry_count", 0)
    old_img_count = paper_report.get("old_image_count", 0)
    new_img_count = paper_report.get("new_image_count", 0)

    has_regression = any(
        i["type"] in {"content_type_changed", "page_changed", "bbox_changed", "body_bbox_changed", "description_changed"}
        for i in issues
    )
    has_missing = new_count < old_count or new_img_count < old_img_count
    has_added = new_count > old_count or new_img_count > old_img_count

    if not issues and old_count == new_count and old_img_count == new_img_count:
        return "identical"
    if (has_added or not has_regression) and not has_missing:
        if has_regression or has_added:
            return "improved"
        return "neutral"
    if has_regression or has_missing:
        return "regressed"
    return "neutral"


def main() -> int:
    if len(sys.argv) != 3:
        print(__doc__, file=sys.stderr)
        return 2

    snapshot_dir = Path(sys.argv[1])
    figures_dir = Path(sys.argv[2])

    if not snapshot_dir.is_dir():
        print(f"Snapshot directory not found: {snapshot_dir}", file=sys.stderr)
        return 1
    if not figures_dir.is_dir():
        print(f"Figures directory not found: {figures_dir}", file=sys.stderr)
        return 1

    old_hashes_by_paper = load_snapshot_hashes(snapshot_dir)

    paper_reports = []
    summary = {"identical": 0, "improved": 0, "regressed": 0, "neutral": 0, "failed": 0}

    json_files = sorted(snapshot_dir.glob("*.json"))
    for json_path in json_files:
        if json_path.name in {"comparison_report.json"}:
            continue
        paper_id = json_path.stem
        new_json_path = figures_dir / "arxiv" / paper_id / "mineru.json"

        report = {
            "paper_id": paper_id,
            "old_entry_count": 0,
            "new_entry_count": 0,
            "old_image_count": 0,
            "new_image_count": 0,
            "image_hash_diff": {"added": [], "removed": []},
            "log_counts": {"old": {}, "new": {}},
            "issues": [],
        }

        with open(json_path) as f:
            old_data = json.load(f)

        if not new_json_path.exists():
            report["issues"].append({"type": "missing_new_json"})
            report["classification"] = "regressed"
            paper_reports.append(report)
            summary["regressed"] += 1
            continue

        with open(new_json_path) as f:
            new_data = json.load(f)

        old_entries = build_entries(old_data)
        new_entries = build_entries(new_data)
        report["old_entry_count"] = len(old_entries)
        report["new_entry_count"] = len(new_entries)

        alignment = align_entries(old_entries, new_entries)

        for old_e, new_e, old_idx, new_idx in alignment["pairs"]:
            entry_issues = compare_entry(old_e, new_e)
            if entry_issues:
                report["issues"].append({
                    "type": "entry_changed",
                    "old_index": old_idx,
                    "new_index": new_idx,
                    "caption": old_e.get("caption") or new_e.get("caption"),
                    "issues": entry_issues,
                })

        for idx in alignment["old_missing"]:
            report["issues"].append({
                "type": "entry_removed",
                "old_index": idx,
                "caption": old_entries[idx].get("caption"),
            })

        for idx in alignment["new_added"]:
            report["issues"].append({
                "type": "entry_added",
                "new_index": idx,
                "caption": new_entries[idx].get("caption"),
            })

        # Image hash comparison
        old_hashes = old_hashes_by_paper.get(paper_id, set())
        new_hashes = load_new_hashes(figures_dir, paper_id)
        report["old_image_count"] = len(old_hashes)
        report["new_image_count"] = len(new_hashes)
        report["image_hash_diff"]["added"] = sorted(new_hashes - old_hashes)
        report["image_hash_diff"]["removed"] = sorted(old_hashes - new_hashes)
        if report["image_hash_diff"]["added"] or report["image_hash_diff"]["removed"]:
            report["issues"].append({
                "type": "image_hash_changed",
                "added_count": len(report["image_hash_diff"]["added"]),
                "removed_count": len(report["image_hash_diff"]["removed"]),
            })

        # Log keyword comparison
        old_log_path = snapshot_dir / f"{paper_id}.log"
        new_log_path = figures_dir / "arxiv" / paper_id / "mineru.log"
        report["log_counts"]["old"] = log_issue_counts(old_log_path)
        report["log_counts"]["new"] = log_issue_counts(new_log_path)
        for key in ("failed", "error", "warn"):
            old_n = report["log_counts"]["old"].get(key, 0)
            new_n = report["log_counts"]["new"].get(key, 0)
            if new_n > old_n:
                report["issues"].append({
                    "type": "log_increase",
                    "keyword": key,
                    "old_count": old_n,
                    "new_count": new_n,
                })

        report["classification"] = classify_paper(report)
        summary[report["classification"]] += 1
        paper_reports.append(report)

    # Detect papers present in figures but missing from snapshot (unlikely)
    for new_json in sorted((figures_dir / "arxiv").glob("*/mineru.json")):
        paper_id = new_json.parent.name
        if not (snapshot_dir / f"{paper_id}.json").exists():
            report = {
                "paper_id": paper_id,
                "issues": [{"type": "missing_from_snapshot"}],
                "classification": "improved",
            }
            paper_reports.append(report)
            summary["improved"] += 1

    report_data = {
        "snapshot_dir": str(snapshot_dir),
        "figures_dir": str(figures_dir),
        "total_papers": len(paper_reports),
        "summary": summary,
        "papers": paper_reports,
    }

    report_json_path = snapshot_dir / "comparison_report.json"
    with open(report_json_path, "w") as f:
        json.dump(report_data, f, indent=2)

    print(f"Snapshot: {snapshot_dir}")
    print(f"Figures:  {figures_dir}")
    print(f"Total papers: {len(paper_reports)}")
    print(
        f"identical={summary['identical']} improved={summary['improved']} "
        f"regressed={summary['regressed']} neutral={summary['neutral']} failed={summary['failed']}"
    )
    print(f"Detailed report: {report_json_path}")

    # Print regressed / improved papers
    for cls in ("regressed", "improved"):
        papers = [p for p in paper_reports if p["classification"] == cls]
        if papers:
            print(f"\n{cls.upper()} papers ({len(papers)}):")
            for p in papers:
                issue_summary = ", ".join(
                    sorted({i["type"] for i in p.get("issues", [])})
                )
                print(f"  {p['paper_id']}: {issue_summary}")

    return 0


if __name__ == "__main__":
    sys.exit(main())
