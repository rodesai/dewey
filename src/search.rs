use anyhow::Result;
use tracing::info;

use vector::{AttributeValue, VectorDb};

use crate::config::RagConfig;
use crate::embed::VoyageClient;

pub async fn run(config: &RagConfig, query: &str, k: usize) -> Result<()> {
    let api_key = RagConfig::voyage_api_key()?;
    let client = VoyageClient::new(api_key, config.voyage_model.clone(), config.dimensions);

    // Embed query
    info!(query, "embedding query");
    let embedding = client.embed_query(query).await?;

    // Open VectorDb
    let vector_config = config.to_vector_config();
    let db = VectorDb::open(vector_config).await?;

    // Search
    let results = db.search(&embedding, k).await?;

    if results.is_empty() {
        println!("No results found.");
        return Ok(());
    }

    for (i, result) in results.iter().enumerate() {
        let file_path = match result.attributes.get("file_path") {
            Some(AttributeValue::String(s)) => s.as_str(),
            _ => &result.external_id,
        };
        let start_line = match result.attributes.get("start_line") {
            Some(AttributeValue::Int64(n)) => *n,
            _ => 0,
        };
        let end_line = match result.attributes.get("end_line") {
            Some(AttributeValue::Int64(n)) => *n,
            _ => 0,
        };
        let item_name = match result.attributes.get("item_name") {
            Some(AttributeValue::String(s)) => s.as_str(),
            _ => "",
        };
        let chunk_text = match result.attributes.get("chunk_text") {
            Some(AttributeValue::String(s)) => s.as_str(),
            _ => "",
        };

        println!("Result {} (score: {:.4})", i + 1, result.score);
        println!("  File: {}:{}-{}", file_path, start_line, end_line);
        if !item_name.is_empty() {
            println!("  Item: {}", item_name);
        }
        println!("  ---");
        // Indent chunk text for readability
        for line in chunk_text.lines() {
            println!("  {}", line);
        }
        println!("  ---");
        println!();
    }

    Ok(())
}
