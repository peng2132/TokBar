#!/usr/bin/env python3
"""Regenerate pricing-snapshot.json from LiteLLM's model price list.

Usage:
    curl -fsSLo litellm.json https://raw.githubusercontent.com/BerriAI/litellm/main/model_prices_and_context_window.json
    python3 update_snapshot.py litellm.json

Keeps the models `pricing::is_relevant_model` accepts and only the price
fields `pricing::pricing_from_litellm` reads, then update SNAPSHOT_DATE in
src/pricing.rs. Run before every release so offline installs (and users who
cannot reach GitHub) start with current prices.
"""

import json
import re
import sys
from pathlib import Path

PREFIXES = [
    "claude", "gpt", "o1", "o3", "o4", "codex", "gemini", "chatgpt", "kimi", "moonshot",
    "minimax", "glm", "qwen", "deepseek", "step", "longcat", "meituan", "z-ai", "zai",
    "zhipu",
]
FIELD = re.compile(
    r"^(input_cost_per_token|output_cost_per_token|cache_read_input_token_cost"
    r"|cache_creation_input_token_cost(_above_1hr)?)(_above_\d+k_tokens)?(_priority)?$"
)


def relevant(key: str) -> bool:
    k = key.lower()
    return any(
        k.startswith(p) or k.startswith(f"{scope}/{p}")
        for p in PREFIXES
        for scope in ("anthropic", "moonshot", "dashscope", "openrouter")
    )


def main(src: str) -> None:
    data = json.loads(Path(src).read_text())
    out = {}
    for key, entry in sorted(data.items()):
        if not isinstance(entry, dict) or not relevant(key):
            continue
        prices = {
            f: v
            for f, v in entry.items()
            if FIELD.match(f) and isinstance(v, (int, float))
        }
        if "input_cost_per_token" in prices or "output_cost_per_token" in prices:
            out[key] = prices
    dest = Path(__file__).with_name("pricing-snapshot.json")
    dest.write_text(json.dumps(out, separators=(",", ":"), sort_keys=True) + "\n")
    print(f"wrote {len(out)} models to {dest}")


if __name__ == "__main__":
    main(sys.argv[1])
