mod chunker;
mod extract;
mod http;
mod local;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub use chunker::{TextSpan, chunk_text};
pub use extract::{extract_text, tidy};
pub use http::HttpEmbedder;
pub use local::LocalHashEmbedder;

#[derive(Debug, Error)]
pub enum EmbedderError {
    #[error("unsupported document kind {0:?}")]
    Unsupported(DocumentKind),
    #[error("document is not valid UTF-8")]
    Encoding,
    #[error("embedding provider error: {0}")]
    Provider(String),
    #[error("invalid chunk config: {0}")]
    Config(String),
    #[error("text extraction failed: {0}")]
    Extraction(String),
}

#[async_trait]
pub trait Embedder: Send + Sync {
    fn model(&self) -> &str;
    fn dimensions(&self) -> usize;
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, EmbedderError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChunkConfig {
    pub max_chars: usize,
    pub overlap_chars: usize,
}

impl Default for ChunkConfig {
    fn default() -> Self {
        Self { max_chars: 1_000, overlap_chars: 200 }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DocumentKind {
    Text,
    Markdown,
    Html,
    Pdf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentInput {
    pub title: String,
    pub kind: DocumentKind,
    pub source_uri: Option<String>,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IngestedChunk {
    pub index: u32,
    pub text: String,
    pub char_start: usize,
    pub char_end: usize,
    pub embedding: Vec<f32>,
}

pub struct KnowledgePipeline<E: Embedder> {
    embedder: E,
    config: ChunkConfig,
}

impl<E: Embedder> KnowledgePipeline<E> {
    pub fn new(embedder: E, config: ChunkConfig) -> Result<Self, EmbedderError> {
        if config.max_chars == 0 || config.overlap_chars >= config.max_chars {
            return Err(EmbedderError::Config(format!("max_chars {} must exceed overlap_chars {}", config.max_chars, config.overlap_chars)));
        }
        Ok(Self { embedder, config })
    }

    pub fn config(&self) -> ChunkConfig {
        self.config
    }

    pub fn embedder(&self) -> &E {
        &self.embedder
    }

    pub fn extract(&self, document: &DocumentInput) -> Result<String, EmbedderError> {
        extract_text(document.kind, &document.bytes)
    }

    pub async fn ingest(&self, document: &DocumentInput) -> Result<Vec<IngestedChunk>, EmbedderError> {
        let text = self.extract(document)?;
        let spans = chunk_text(&text, self.config);
        if spans.is_empty() {
            return Ok(Vec::new());
        }
        let texts: Vec<String> = spans.iter().map(|span| text[span.byte_start..span.byte_end].to_owned()).collect();
        let vectors = self.embedder.embed(&texts).await?;
        if vectors.len() != texts.len() {
            return Err(EmbedderError::Provider(format!("embedder returned {} vectors for {} chunks", vectors.len(), texts.len())));
        }
        Ok(spans
            .into_iter()
            .zip(texts)
            .zip(vectors)
            .enumerate()
            .map(|(index, ((span, text), embedding))| IngestedChunk {
                index: u32::try_from(index).unwrap_or(u32::MAX),
                text,
                char_start: span.char_start,
                char_end: span.char_end,
                embedding,
            })
            .collect())
    }
}
