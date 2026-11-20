use async_trait::async_trait;
use magicklala_embedder::{Embedder, EmbedderError};

pub const HASHING_DIMENSIONS: usize = 256;

#[derive(Debug, Clone, Copy, Default)]
pub struct HashingEmbedder;

impl HashingEmbedder {
    pub fn embed_one(text: &str) -> Vec<f32> {
        let mut vector = vec![0.0_f32; HASHING_DIMENSIONS];
        for token in text.split(|character: char| !character.is_alphanumeric()).filter(|token| !token.is_empty()) {
            let bucket = fnv1a(&token.to_lowercase()) % HASHING_DIMENSIONS as u64;
            if let Some(slot) = vector.get_mut(usize::try_from(bucket).unwrap_or_default()) {
                *slot += 1.0;
            }
        }
        let norm = vector.iter().map(|value| value * value).sum::<f32>().sqrt();
        if norm > 0.0 {
            vector.iter_mut().for_each(|value| *value /= norm);
        }
        vector
    }
}

fn fnv1a(text: &str) -> u64 {
    text.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3))
}

pub fn cosine(left: &[f32], right: &[f32]) -> f32 {
    if left.len() != right.len() {
        return 0.0;
    }
    let dot: f32 = left.iter().zip(right).map(|(a, b)| a * b).sum();
    let norms = left.iter().map(|a| a * a).sum::<f32>().sqrt() * right.iter().map(|b| b * b).sum::<f32>().sqrt();
    if norms > 0.0 { dot / norms } else { 0.0 }
}

#[async_trait]
impl Embedder for HashingEmbedder {
    fn model(&self) -> &str {
        "magicklala-hashing-256"
    }

    fn dimensions(&self) -> usize {
        HASHING_DIMENSIONS
    }

    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, EmbedderError> {
        Ok(texts.iter().map(|text| Self::embed_one(text)).collect())
    }
}
