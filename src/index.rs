use std::path::Path;

use anyhow::{Context, Result};
use glob::Pattern;
use tracing::info;
use walkdir::WalkDir;

use vector::{Vector, VectorDb};

use crate::chunk::{self, Chunk};
use crate::config::{RagConfig, SourceDir};
use crate::embed::VoyageClient;

pub async fn run(config: &RagConfig) -> Result<()> {
    let api_key = RagConfig::voyage_api_key()?;
    let client = VoyageClient::new(api_key, config.voyage_model.clone(), config.dimensions);

    // Walk and chunk all source dirs
    let chunks = walk_and_chunk_all(&config.source_dirs)?;
    info!(total_chunks = chunks.len(), "chunking complete");

    if chunks.is_empty() {
        info!("no chunks to index");
        return Ok(());
    }

    // Embed
    let texts: Vec<String> = chunks.iter().map(|c| c.text.clone()).collect();
    let embeddings = client
        .embed_documents_batched(&texts, config.embed_batch_size)
        .await
        .context("failed to embed chunks")?;

    // Open VectorDb
    let vector_config = config.to_vector_config();
    let db = VectorDb::open(vector_config).await?;

    // Write in batches
    let batch_size = 100;
    let total_vectors = chunks.len();

    for (batch_idx, batch) in chunks.chunks(batch_size).enumerate() {
        let start = batch_idx * batch_size;
        let vectors: Vec<Vector> = batch
            .iter()
            .enumerate()
            .map(|(i, chunk)| {
                let idx = start + i;
                let external_id = format!(
                    "{}:{}-{}",
                    chunk.file_path, chunk.start_line, chunk.end_line
                );
                Vector::builder(external_id, embeddings[idx].clone())
                    .attribute("file_path", chunk.file_path.as_str())
                    .attribute("start_line", chunk.start_line as i64)
                    .attribute("end_line", chunk.end_line as i64)
                    .attribute("item_name", chunk.item_name.as_str())
                    .attribute("chunk_text", chunk.text.as_str())
                    .attribute("language", chunk.language.as_str())
                    .build()
            })
            .collect();

        db.write(vectors).await?;
        info!(
            batch = batch_idx + 1,
            vectors_written = start + batch.len(),
            total = total_vectors,
            "batch written"
        );
    }

    db.flush().await?;
    info!(total_vectors, "index complete");

    Ok(())
}

fn walk_and_chunk_all(source_dirs: &[SourceDir]) -> Result<Vec<Chunk>> {
    let mut all_chunks = Vec::new();

    for source_dir in source_dirs {
        let dir_path = Path::new(&source_dir.path);
        // Use the last component of the path as the prefix (e.g. "opendata", "slatedb")
        let prefix = dir_path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| source_dir.path.clone());

        info!(
            dir = source_dir.path,
            prefix = prefix,
            "indexing source directory"
        );

        let chunks = walk_and_chunk(
            dir_path,
            &prefix,
            &source_dir.include_patterns,
            &source_dir.exclude_patterns,
        )?;
        all_chunks.extend(chunks);
    }

    Ok(all_chunks)
}

fn walk_and_chunk(
    source_dir: &Path,
    prefix: &str,
    include_patterns: &[String],
    exclude_patterns: &[String],
) -> Result<Vec<Chunk>> {
    let include: Vec<Pattern> = include_patterns
        .iter()
        .map(|p| Pattern::new(p))
        .collect::<Result<_, _>>()
        .context("invalid include pattern")?;

    let exclude: Vec<Pattern> = exclude_patterns
        .iter()
        .map(|p| Pattern::new(p))
        .collect::<Result<_, _>>()
        .context("invalid exclude pattern")?;

    let mut all_chunks = Vec::new();

    for entry in WalkDir::new(source_dir).into_iter().filter_map(|e| e.ok()) {
        if !entry.file_type().is_file() {
            continue;
        }

        let path = entry.path();
        let rel_path = path
            .strip_prefix(source_dir)
            .unwrap_or(path)
            .to_string_lossy()
            .to_string();

        // Check include patterns
        if !include.is_empty() && !include.iter().any(|p| p.matches(&rel_path)) {
            continue;
        }

        // Check exclude patterns
        if exclude.iter().any(|p| p.matches(&rel_path)) {
            continue;
        }

        // Prefix the file path with the source dir name for disambiguation
        let prefixed_path = format!("{}/{}", prefix, rel_path);

        let source = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "skipping unreadable file");
                continue;
            }
        };

        match chunk::chunk_file(&prefixed_path, &source) {
            Ok(chunks) => {
                info!(file = prefixed_path, chunks = chunks.len(), "chunked file");
                all_chunks.extend(chunks);
            }
            Err(e) => {
                tracing::warn!(file = prefixed_path, error = %e, "failed to chunk file");
            }
        }
    }

    Ok(all_chunks)
}
