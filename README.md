# RAG CLI

Semantic code search over the OpenData codebase. Indexes Rust and markdown files into the vector database, then serves search queries. Designed for RAG integration with Claude Code.

## Prerequisites

- Rust toolchain (edition 2024)
- [Voyage AI](https://voyageai.com/) API key (for embeddings)
- AWS credentials (if using S3 storage) or local filesystem

## Build

```bash
cd rag
cargo build --release
```

The binary is at `target/release/rag`.

## Configuration

Copy and edit `rag.yaml`:

```yaml
# Storage — pick one of S3 or local
s3_bucket: opendata-rag          # S3 bucket (omit for local)
s3_region: us-east-1             # S3 region (omit for local)
s3_path: rag-index               # SlateDB path prefix

# For local development, use local_path instead of s3_bucket/s3_region:
# local_path: /tmp/rag-data

source_dirs:
  - path: /Users/rohan/responsive/opendata
    include_patterns:
      - "**/*.rs"
      - "**/*.md"
    exclude_patterns:
      - "**/target/**"

  - path: /Users/rohan/repos/slatedb
    include_patterns:
      - "**/*.rs"
      - "**/*.md"
    exclude_patterns:
      - "**/target/**"

voyage_model: voyage-code-3
dimensions: 1024
embed_batch_size: 128
```

Each entry in `source_dirs` has its own `path`, `include_patterns`, and `exclude_patterns`. File paths in the index are prefixed with the directory name (e.g. `opendata/vector/src/db.rs`, `slatedb/src/db.rs`) so results from different repos are unambiguous.

Set your Voyage API key:

```bash
export VOYAGE_API_KEY=your-key-here
```

## Index the codebase

```bash
VOYAGE_API_KEY=... cargo run --release -- index --config rag.yaml
```

This walks the source directory, chunks Rust files by declaration (functions, structs, impls, traits, enums) and markdown files by heading, embeds each chunk via Voyage AI, and writes the vectors to the database. Re-running performs a full re-index with upsert semantics.

For local development, use `local_path` in the config instead of S3:

```yaml
local_path: /tmp/rag-data
s3_path: rag-index
source_dirs:
  - path: /Users/rohan/responsive/opendata
    include_patterns:
      - "**/*.rs"
      - "**/*.md"
    exclude_patterns:
      - "**/target/**"
```

## Search from the command line

```bash
VOYAGE_API_KEY=... cargo run --release -- search --config rag.yaml "how does centroid splitting work"
```

Options:
- `-k <N>` — number of results (default: 10)
- `-c <path>` — config file (default: `rag.yaml`)

Example output:

```
Result 1 (score: 0.9234)
  File: vector/src/db.rs:82-106
  Item: VectorDb::split_centroid
  ---
  async fn split_centroid(&self, centroid_id: u64) -> Result<()> {
      ...
  }
  ---
```

Each result includes the file path, line range, item name, similarity score, and the full source text of the chunk.

## Claude Code integration

The search command is self-contained — it reads embeddings and chunk text from the vector database, so it doesn't need access to the source repo at search time.

### Option 1: Add to CLAUDE.md (simplest)

Add this to your project's `CLAUDE.md`:

```markdown
## RAG search

When you need to find relevant code or understand how something works,
run the RAG search tool:

\```bash
VOYAGE_API_KEY=$VOYAGE_API_KEY /path/to/rag search --config /path/to/rag.yaml "<query>" -k 5
\```
```

Claude Code will then use the `rag search` command via its Bash tool when it needs to find relevant code context.

### Option 2: MCP server with stdio transport

Wrap the search command as a stdio MCP server so Claude Code can call it as a native tool. Create a small wrapper script:

```bash
#!/bin/bash
# rag-mcp-wrapper.sh
# Reads JSON-RPC requests on stdin, calls rag search, returns results
# (See MCP protocol docs for full implementation)
```

Then register it with Claude Code:

```bash
claude mcp add --transport stdio opendata-rag \
  --env VOYAGE_API_KEY=$VOYAGE_API_KEY \
  -- /path/to/rag-mcp-wrapper.sh
```

Or add it to `.mcp.json` in the repo root:

```json
{
  "mcpServers": {
    "opendata-rag": {
      "type": "stdio",
      "command": "/path/to/rag-mcp-wrapper.sh",
      "env": {
        "VOYAGE_API_KEY": "${VOYAGE_API_KEY}"
      }
    }
  }
}
```

A full MCP server implementation is a future iteration.

## How it works

**Chunking:**
- Rust files are parsed with tree-sitter. Each top-level declaration (function, struct, enum, impl, trait, const, type, macro) becomes a chunk. Large impl blocks (>100 lines) are split into per-method chunks.
- Markdown files are split on `#`/`##`/`###` headings. Each section becomes a chunk.
- Chunks exceeding 48K characters are truncated (Voyage's 16K token limit).

**Embeddings:** Voyage AI `voyage-code-3` model with 1024 dimensions and cosine similarity.

**Storage:** Vectors are stored in the OpenData vector database (SPANN-style ANN search backed by SlateDB). Each vector carries metadata: file path, line range, item name, language, and the full chunk text. At ~5K vectors the index fits in a single centroid, so search is effectively brute-force and sub-millisecond.
