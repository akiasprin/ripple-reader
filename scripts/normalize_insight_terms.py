#!/usr/bin/env python3
"""Normalize algorithm/proper-noun capitalization in paper_insights.

Only affects well-known algorithm names, model names, architecture names,
dataset names, framework names, and clear proper nouns. Common technical
terms (bias, token, embedding, attention, etc.) and all-caps abbreviations
(SOTA, BERT, etc.) are left unchanged.

Usage:
    python3 scripts/normalize_insight_terms.py          # preview changes
    python3 scripts/normalize_insight_terms.py --apply   # write to DB

Requires: psql on PATH, DATABASE_URL in .env or default connection.
"""
import argparse
import csv
import os
import re
import subprocess
import sys
from pathlib import Path

# ---------------------------------------------------------------------------
# Mapping: original phrase (case-insensitive) -> normalized form
# ---------------------------------------------------------------------------
OBVIOUS_MAPPINGS: dict[str, str] = {
    # --- Normalization layers ---
    "batch normalization": "Batch Normalization",
    "layer normalization": "Layer Normalization",
    "instance normalization": "Instance Normalization",
    "group normalization": "Group Normalization",
    "weight normalization": "Weight Normalization",
    "batch norm": "Batch Norm",
    "layer norm": "Layer Norm",
    "batch-normalized network": "Batch-Normalized Network",

    # --- Activation functions ---
    "relu": "ReLU",
    "leaky relu": "Leaky ReLU",
    "parametric relu": "Parametric ReLU",
    "gelu": "GELU",
    "silu": "SiLU",
    "swish": "Swish",
    "swiglu": "SwiGLU",
    "mish": "Mish",

    # --- Architectures / models ---
    "transformer": "Transformer",
    "transformers": "Transformers",
    "document transformer": "Document Transformer",
    "decoder-only transformer": "Decoder-Only Transformer",
    "encoder-only transformer": "Encoder-Only Transformer",
    "encoder-decoder transformer": "Encoder-Decoder Transformer",
    "bert": "BERT",
    "gpt": "GPT",
    "gpt-1": "GPT-1",
    "gpt-2": "GPT-2",
    "gpt-3": "GPT-3",
    "gpt-4": "GPT-4",
    "gpt-4o": "GPT-4o",
    "llama": "LLaMA",
    "llama-2": "LLaMA 2",
    "llama-3": "LLaMA 3",
    "palm": "PaLM",
    "palm-2": "PaLM 2",
    "qwen": "Qwen",
    "qwen2": "Qwen2",
    "qwen3": "Qwen3",
    "vae": "VAE",
    "gaussian vae": "Gaussian VAE",
    "full-covariance gaussian vae": "Full-Covariance Gaussian VAE",
    "lstm": "LSTM",
    "rnn": "RNN",
    "resnet": "ResNet",
    "resnet-18": "ResNet-18",
    "resnet-34": "ResNet-34",
    "resnet-50": "ResNet-50",
    "resnet-101": "ResNet-101",
    "resnet-152": "ResNet-152",
    "cnn": "CNN",
    "mlp": "MLP",
    "ffn": "FFN",
    "bart": "BART",
    "t5": "T5",
    "vit": "ViT",
    "clip": "CLIP",
    "llada": "LLaDA",
    "mistral": "Mistral",
    "mixtral": "Mixtral",
    "roformer": "RoFormer",
    "stable diffusion": "Stable Diffusion",

    # --- Optimizers ---
    "adam": "Adam",
    "sgd": "SGD",
    "adamw": "AdamW",
    "rmsprop": "RMSprop",
    "adagrad": "Adagrad",
    "adadelta": "Adadelta",
    "adamax": "Adamax",

    # --- Datasets / benchmarks ---
    "mnist": "MNIST",
    "frey face": "Frey Face",
    "imagenet": "ImageNet",
    "cifar": "CIFAR",
    "cifar-10": "CIFAR-10",
    "cifar-100": "CIFAR-100",
    "squad": "SQuAD",
    "glue": "GLUE",
    "gsm8k": "GSM8K",
    "mmlu": "MMLU",
    "human-eval": "HumanEval",
    "winogrande": "WinoGrande",
    "hellaswag": "HellaSwag",
    "arc": "ARC",
    "piqa": "PIQA",
    "openbookqa": "OpenBookQA",

    # --- Frameworks / libraries ---
    "tensorflow": "TensorFlow",
    "pytorch": "PyTorch",
    "jax": "JAX",
    "keras": "Keras",
    "huggingface": "Hugging Face",
    "numpy": "NumPy",
    "scipy": "SciPy",

    # --- Named methods / techniques ---
    "flow matching": "Flow Matching",
    "flow matching with optimal transport": "Flow Matching with Optimal Transport",
    "flow matching with ot": "Flow Matching with OT",
    "agent harness": "Agent Harness",
    "code as agent harness": "Code as Agent Harness",
    "code as harness": "Code as Harness",
    "rope": "RoPE",
    "react": "ReAct",
    "toolformer": "Toolformer",
    "draw": "DRAW",
    "orca": "Orca",
    "aevb": "AEVB",
    "sgvb": "SGVB",
    "mcem": "MCEM",
    "hmc": "HMC",
    "em algorithm": "EM Algorithm",
    "variational bayes": "Variational Bayes",
    "monte carlo": "Monte Carlo",
    "markov chain": "Markov Chain",
    "monte carlo em": "Monte Carlo EM",
    "reinforce": "REINFORCE",
    "reparameterization trick": "Reparameterization Trick",
    "score matching": "Score Matching",
    "diffusion model": "Diffusion Model",
    "diffusion models": "Diffusion Models",
    "energy-based model": "Energy-Based Model",
    "energy-based models": "Energy-Based Models",
    "autoregressive model": "Autoregressive Model",
    "autoregressive models": "Autoregressive Models",
    "flow-based model": "Flow-Based Model",
    "flow-based models": "Flow-Based Models",
    "deep q-learning": "Deep Q-Learning",
    "deep q-network": "Deep Q-Network",
    "deep q-networks": "Deep Q-Networks",
    "dynamic bayesian networks": "Dynamic Bayesian Networks",
    "bayesian networks": "Bayesian Networks",
    "bayes by backprop": "Bayes by Backprop",
    "bahdanau attention": "Bahdanau Attention",
    "luong attention": "Luong Attention",
    "multi-head attention": "Multi-Head Attention",
    "self-attention": "Self-Attention",
    "cross-attention": "Cross-Attention",
    "chain-of-thought": "Chain-of-Thought",
    "chain of thought": "Chain of Thought",
    "chain-of-thought prompting": "Chain-of-Thought Prompting",
    "few-shot": "Few-Shot",
    "few-shot examples": "Few-Shot Examples",
    "few-shot learning": "Few-Shot Learning",
    "zero-shot": "Zero-Shot",
    "zero-shot learning": "Zero-Shot Learning",
    "in-context learning": "In-Context Learning",
    "instruction tuning": "Instruction Tuning",
    "supervised fine-tuning": "Supervised Fine-Tuning",
    "reinforcement learning": "Reinforcement Learning",
    "reinforcement learning from human feedback": "Reinforcement Learning from Human Feedback",
    "human-in-the-loop": "Human-in-the-Loop",
    "machine learning": "Machine Learning",
    "deep learning": "Deep Learning",
    "neural network": "Neural Network",
    "neural networks": "Neural Networks",
    "convolutional neural network": "Convolutional Neural Network",
    "convolutional neural networks": "Convolutional Neural Networks",
    "recurrent neural network": "Recurrent Neural Network",
    "recurrent neural networks": "Recurrent Neural Networks",
    "graph neural network": "Graph Neural Network",
    "graph neural networks": "Graph Neural Networks",
    "deep residual learning": "Deep Residual Learning",
    "layer-selective rank reduction": "Layer-Selective Rank Reduction",
    "knowledge distillation": "Knowledge Distillation",
    "homotopic distillation": "Homotopic Distillation",
    "distilling step-by-step": "Distilling Step-by-Step",
    "graph of thoughts": "Graph of Thoughts",
    "tree of thoughts": "Tree of Thoughts",
    "tool learning": "Tool Learning",
    "code generation": "Code Generation",
    "lean theorem prover": "Lean Theorem Prover",
    "brown clustering": "Brown Clustering",

    # --- Metrics / losses ---
    "kl divergence": "KL Divergence",
    "kl": "KL",
    "elbo": "ELBO",
    "fid": "FID",
    "inception distance": "Inception Distance",
    "bleu": "BLEU",
    "rouge": "ROUGE",
    "meteor": "METEOR",

    # --- Hardware / infra ---
    "gpu": "GPU",
    "gpus": "GPUs",
    "cpu": "CPU",
    "cpus": "CPUs",
    "tpu": "TPU",
    "tpus": "TPUs",

    # --- Other methods / acronyms ---
    "ppo": "PPO",
    "rag": "RAG",
    "dni": "DNI",
    "cnf": "CNF",
    "ode": "ODE",
    "fm-ot": "FM-OT",
    "sft": "SFT",
    "rlhf": "RLHF",
    "dpo": "DPO",
    "grpo": "GRPO",
    "flops": "FLOPs",
    "api": "API",
    "apis": "APIs",

    # --- Paper-specific proper nouns / named algorithms ---
    "roofline": "Roofline",
    "clock algorithm": "Clock Algorithm",
    "pizza algorithm": "Pizza Algorithm",
    "clock logit": "Clock Logit",
}

# ---------------------------------------------------------------------------
# Regex engine
# ---------------------------------------------------------------------------

def _build_replacements(mapping: dict[str, str]) -> list[tuple[re.Pattern, str, str]]:
    """Compile case-insensitive regexes, longest-first to avoid partial matches."""
    reps: list[tuple[re.Pattern, str, str]] = []
    for original, normalized in mapping.items():
        pattern = re.escape(original).replace(r"\ ", r"[\s\-]+")
        if " " in original or "-" in original:
            regex = rf"(?<![A-Za-z0-9]){pattern}(?![A-Za-z0-9])"
        else:
            regex = rf"\b{pattern}\b"
        reps.append((re.compile(regex, re.IGNORECASE), normalized, original))
    reps.sort(key=lambda x: -len(x[2]))
    return reps


REPLACEMENTS = _build_replacements(OBVIOUS_MAPPINGS)

# ---------------------------------------------------------------------------
# Math / code protection
# ---------------------------------------------------------------------------

def _protect_blocks(text: str) -> tuple[str, list[str]]:
    """Replace $math$, ```code```, and `inline` with placeholders."""
    placeholders: list[str] = []
    counter = 0

    def _store(match: re.Match) -> str:
        nonlocal counter
        ph = f"\x00PH{counter}\x00"
        placeholders.append(match.group(0))
        counter += 1
        return ph

    text = re.sub(r"\$\$[\s\S]*?\$\$", _store, text)
    text = re.sub(r"\$[^$\n]+?\$", _store, text)
    text = re.sub(r"```[\s\S]*?```", _store, text)
    text = re.sub(r"`[^`\n]+?`", _store, text)
    return text, placeholders


def _restore_blocks(text: str, placeholders: list[str]) -> str:
    for i, ph in enumerate(placeholders):
        text = text.replace(f"\x00PH{i}\x00", ph, 1)
    return text

# ---------------------------------------------------------------------------
# Core normalizer
# ---------------------------------------------------------------------------

def normalize_text(text: str) -> tuple[str, dict[str, int]]:
    """Return (normalized_text, {original: change_count})."""
    protected, placeholders = _protect_blocks(text)
    changes: dict[str, int] = {}

    for regex, normalized, original in REPLACEMENTS:
        def _repl(m: re.Match, *, norm: str = normalized, orig: str = original) -> str:
            changes[orig] = changes.get(orig, 0) + 1
            return norm
        protected = regex.sub(_repl, protected)

    return _restore_blocks(protected, placeholders), changes

# ---------------------------------------------------------------------------
# DB helpers
# ---------------------------------------------------------------------------

def _db_url() -> str:
    """Best-effort DATABASE_URL from .env."""
    env_path = Path(__file__).resolve().parent.parent / ".env"
    if env_path.exists():
        for line in env_path.read_text().splitlines():
            line = line.strip()
            if line.startswith("DATABASE_URL="):
                return line.split("=", 1)[1].strip().strip("\"'")
    return os.environ.get("DATABASE_URL", "")


def _psql_base_args(url: str) -> list[str]:
    return ["psql", url] if url else ["psql"]


def fetch_insights(db_url: str) -> list[tuple[str, str]]:
    """Export (paper_id, insight) rows via COPY CSV."""
    csv_path = Path("/tmp/_norm_insights_in.csv")
    subprocess.run(
        [*_psql_base_args(db_url), "-c",
         f"\\copy (SELECT paper_id, insight FROM paper_insights WHERE insight != '') "
         f"TO '{csv_path}' WITH CSV HEADER;"],
        check=True,
    )
    rows: list[tuple[str, str]] = []
    with open(csv_path, "r", encoding="utf-8", newline="") as f:
        for rec in csv.DictReader(f):
            rows.append((rec["paper_id"], rec["insight"]))
    return rows


def apply_updates(db_url: str, rows: list[tuple[str, str]]) -> None:
    """Write normalized CSV and UPDATE both insight tables."""
    csv_path = Path("/tmp/_norm_insights_out.csv")
    with open(csv_path, "w", encoding="utf-8", newline="") as f:
        w = csv.writer(f)
        w.writerow(["paper_id", "insight"])
        for pid, insight in rows:
            w.writerow([pid, insight])

    sql = f"""BEGIN;
DROP TABLE IF EXISTS _tmp_insight_norm;
CREATE TEMP TABLE _tmp_insight_norm (paper_id TEXT PRIMARY KEY, insight TEXT);
\\COPY _tmp_insight_norm (paper_id, insight) FROM '{csv_path}' WITH CSV HEADER;
UPDATE paper_insights  SET insight = t.insight FROM _tmp_insight_norm t WHERE paper_insights.paper_id  = t.paper_id;
UPDATE paper_insight_backups SET insight = t.insight FROM _tmp_insight_norm t WHERE paper_insight_backups.paper_id = t.paper_id;
DELETE FROM _cache_insight_html;
DROP TABLE _tmp_insight_norm;
COMMIT;
"""
    sql_path = Path("/tmp/_norm_insights.sql")
    sql_path.write_text(sql, encoding="utf-8")
    subprocess.run([*_psql_base_args(db_url), "-f", str(sql_path)], check=True)
    print("DB updated. Insight HTML cache cleared.")

# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------

def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--apply", action="store_true",
                        help="Write changes to DB (default: dry-run preview only)")
    args = parser.parse_args()

    db_url = _db_url()
    if not db_url:
        print("ERROR: DATABASE_URL not found in .env or env", file=sys.stderr)
        sys.exit(1)

    print(f"DB: {db_url.split('@')[-1]}")
    rows = fetch_insights(db_url)
    print(f"Loaded {len(rows)} insights.")

    total_changes: dict[str, int] = {}
    normalized_rows: list[tuple[str, str]] = []
    for pid, insight in rows:
        norm, changes = normalize_text(insight)
        normalized_rows.append((pid, norm))
        for k, v in changes.items():
            total_changes[k] = total_changes.get(k, 0) + v

    if not total_changes:
        print("All terms already normalized -- nothing to do.")
        return

    print(f"\n{'Replacements:':}")
    for term, count in sorted(total_changes.items(), key=lambda x: -x[1]):
        print(f"  {term!r:>40s} -> {OBVIOUS_MAPPINGS[term]!r}  ({count}x)")

    if not args.apply:
        print("\nDry run. Re-run with --apply to write changes to DB.")
        return

    apply_updates(db_url, normalized_rows)


if __name__ == "__main__":
    main()
