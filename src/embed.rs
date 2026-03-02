use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

pub struct VoyageClient {
    client: reqwest::Client,
    api_key: String,
    model: String,
    dimensions: u16,
}

#[derive(Serialize)]
struct EmbedRequest<'a> {
    model: &'a str,
    input: &'a [String],
    input_type: &'a str,
    output_dimension: u16,
}

#[derive(Deserialize)]
struct EmbedResponse {
    data: Vec<EmbedData>,
    usage: Option<Usage>,
}

#[derive(Deserialize)]
struct EmbedData {
    embedding: Vec<f32>,
}

#[derive(Deserialize)]
struct Usage {
    total_tokens: u64,
}

impl VoyageClient {
    pub fn new(api_key: String, model: String, dimensions: u16) -> Self {
        let client = reqwest::Client::new();
        Self {
            client,
            api_key,
            model,
            dimensions,
        }
    }

    pub async fn embed_documents(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        self.embed(texts, "document").await
    }

    pub async fn embed_query(&self, text: &str) -> Result<Vec<f32>> {
        let texts = vec![text.to_string()];
        let mut results = self.embed(&texts, "query").await?;
        results
            .pop()
            .context("empty embedding response from Voyage API")
    }

    async fn embed(&self, texts: &[String], input_type: &str) -> Result<Vec<Vec<f32>>> {
        let request = EmbedRequest {
            model: &self.model,
            input: texts,
            input_type,
            output_dimension: self.dimensions,
        };

        let mut retries = 0;
        let max_retries = 5;

        loop {
            let response = self
                .client
                .post("https://api.voyageai.com/v1/embeddings")
                .header("Authorization", format!("Bearer {}", self.api_key))
                .json(&request)
                .send()
                .await
                .context("failed to send request to Voyage API")?;

            let status = response.status();

            if status.is_success() {
                let body: EmbedResponse = response
                    .json()
                    .await
                    .context("failed to parse Voyage API response")?;

                if let Some(usage) = &body.usage {
                    debug!(
                        tokens = usage.total_tokens,
                        batch_size = texts.len(),
                        "voyage embedding batch complete"
                    );
                }

                return Ok(body.data.into_iter().map(|d| d.embedding).collect());
            }

            if (status == reqwest::StatusCode::TOO_MANY_REQUESTS || status.is_server_error())
                && retries < max_retries
            {
                retries += 1;
                let backoff = std::time::Duration::from_millis(500 * 2u64.pow(retries));
                warn!(
                    status = %status,
                    retry = retries,
                    backoff_ms = backoff.as_millis() as u64,
                    "voyage API error, retrying"
                );
                tokio::time::sleep(backoff).await;
                continue;
            }

            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("Voyage API error ({}): {}", status, body);
        }
    }

    pub async fn embed_documents_batched(
        &self,
        texts: &[String],
        batch_size: usize,
    ) -> Result<Vec<Vec<f32>>> {
        let mut all_embeddings = Vec::with_capacity(texts.len());
        let total_tokens = 0u64;

        for (i, batch) in texts.chunks(batch_size).enumerate() {
            debug!(
                batch = i + 1,
                total_batches = (texts.len() + batch_size - 1) / batch_size,
                batch_size = batch.len(),
                "embedding batch"
            );
            let embeddings = self.embed_documents(batch).await?;
            all_embeddings.extend(embeddings);
        }

        info!(
            total_texts = texts.len(),
            total_tokens, "embedding complete"
        );
        Ok(all_embeddings)
    }
}
