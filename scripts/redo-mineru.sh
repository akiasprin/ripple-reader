#!/usr/bin/env bash
set -euo pipefail

PROJECT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
cd "$PROJECT_DIR"

usage() {
    echo "Usage: $0 [options]"
    echo
    echo "Re-extract figures/tables from mineru.zip using the MinerU API,"
    echo "then regenerate hires PNG screenshots."
    echo
    echo "Options:"
    echo "  --paper ID     Process only this paper (must have figures/ID/mineru.zip)"
    echo "  --offset N     Skip first N papers (default: 0)"
    echo "  --limit N      Process at most N papers (default: all)"
    echo "  --workers N    Parallel workers (default: CPU count)"
    echo "  --no-img       Skip image extraction (JSON metadata only)"
    echo "  --hires ID     Generate hires screenshots for a single paper only"
    echo "  --help         Show this help"
    echo
    echo "Examples:"
    echo "  $0                              # Reprocess all papers"
    echo "  $0 --paper 2106.04554           # Reprocess one paper"
    echo "  $0 --offset 10 --limit 5        # Process papers 10-14"
    echo "  $0 --hires 2106.04554           # Regenerate hires PNGs only"
    exit 0
}

ARGS=()
while [[ $# -gt 0 ]]; do
    case "$1" in
        --help|-h) usage ;;
        --paper|--offset|--limit|--workers|--hires)
            ARGS+=("$1" "$2")
            shift 2
            ;;
        --no-img)
            ARGS+=("$1")
            shift
            ;;
        *)
            echo "Unknown option: $1"
            usage
            ;;
    esac
done

echo "→ Building redo_mineru..."
cargo build --release --bin redo_mineru 2>&1 | tail -1

echo "→ Running redo_mineru..."
RUST_LOG=info ./target/release/redo_mineru "${ARGS[@]:-}"
