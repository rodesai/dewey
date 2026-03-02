use std::time::Duration;

use anyhow::{Context, Result};
use serde::Deserialize;

use common::StorageConfig;
use common::storage::config::{
    AwsObjectStoreConfig, LocalObjectStoreConfig, ObjectStoreConfig, SlateDbStorageConfig,
};
use vector::{Config as VectorConfig, DistanceMetric, FieldType, MetadataFieldSpec};

#[derive(Debug, Clone, Deserialize)]
pub struct SourceDir {
    pub path: String,
    /// File patterns to include (e.g. ["**/*.rs", "**/*.md"])
    #[serde(default)]
    pub include_patterns: Vec<String>,
    /// File patterns to exclude (e.g. ["**/target/**"])
    #[serde(default)]
    pub exclude_patterns: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct RagConfig {
    /// S3 bucket name (used when storage_type is "s3")
    pub s3_bucket: Option<String>,
    /// S3 region (used when storage_type is "s3")
    pub s3_region: Option<String>,
    /// SlateDB path prefix
    pub s3_path: String,
    /// Local object store path (used when storage_type is "local")
    pub local_path: Option<String>,
    /// Source directories to index
    #[serde(default)]
    pub source_dirs: Vec<SourceDir>,
    /// Voyage AI model name
    #[serde(default = "default_voyage_model")]
    pub voyage_model: String,
    /// Embedding dimensions
    #[serde(default = "default_dimensions")]
    pub dimensions: u16,
    /// Batch size for embedding requests
    #[serde(default = "default_embed_batch_size")]
    pub embed_batch_size: usize,
}

fn default_voyage_model() -> String {
    "voyage-code-3".to_string()
}

fn default_dimensions() -> u16 {
    1024
}

fn default_embed_batch_size() -> usize {
    128
}

impl RagConfig {
    pub fn load(path: &str) -> Result<Self> {
        let contents = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read config file: {}", path))?;
        let config: RagConfig =
            serde_yaml::from_str(&contents).with_context(|| "failed to parse config YAML")?;
        Ok(config)
    }

    pub fn to_vector_config(&self) -> VectorConfig {
        let storage = if let (Some(bucket), Some(region)) = (&self.s3_bucket, &self.s3_region) {
            StorageConfig::SlateDb(SlateDbStorageConfig {
                path: self.s3_path.clone(),
                object_store: ObjectStoreConfig::Aws(AwsObjectStoreConfig {
                    region: region.clone(),
                    bucket: bucket.clone(),
                }),
                settings_path: None,
            })
        } else if let Some(local_path) = &self.local_path {
            StorageConfig::SlateDb(SlateDbStorageConfig {
                path: self.s3_path.clone(),
                object_store: ObjectStoreConfig::Local(LocalObjectStoreConfig {
                    path: local_path.clone(),
                }),
                settings_path: None,
            })
        } else {
            StorageConfig::SlateDb(SlateDbStorageConfig {
                path: self.s3_path.clone(),
                object_store: ObjectStoreConfig::Local(LocalObjectStoreConfig {
                    path: ".data".to_string(),
                }),
                settings_path: None,
            })
        };

        VectorConfig {
            storage,
            dimensions: self.dimensions,
            distance_metric: DistanceMetric::Cosine,
            flush_interval: Duration::from_secs(60),
            metadata_fields: vec![
                MetadataFieldSpec::new("file_path", FieldType::String, false),
                MetadataFieldSpec::new("start_line", FieldType::Int64, false),
                MetadataFieldSpec::new("end_line", FieldType::Int64, false),
                MetadataFieldSpec::new("item_name", FieldType::String, false),
                MetadataFieldSpec::new("chunk_text", FieldType::String, false),
                MetadataFieldSpec::new("language", FieldType::String, false),
            ],
            ..VectorConfig::default()
        }
    }

    pub fn voyage_api_key() -> Result<String> {
        std::env::var("VOYAGE_API_KEY").context("VOYAGE_API_KEY environment variable not set")
    }
}
