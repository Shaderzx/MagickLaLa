use async_trait::async_trait;

use crate::{Embedder, EmbedderError};

const STOPWORDS: [&str; 48] = [
    "a", "an", "and", "are", "as", "at", "be", "by", "can", "do", "does", "for", "from", "has", "have", "how", "i", "in", "is", "it", "its", "of", "on", "or",
    "our", "so", "than", "that", "the", "their", "them", "then", "there", "these", "this", "to", "was", "we", "were", "what", "when", "where", "which", "who",
    "why", "will", "with", "you",
];
const WORD_WEIGHT: f32 = 1.0;
const PAIR_WEIGHT: f32 = 0.5;
const TRIGRAM_BUDGET: f32 = 0.6;

#[derive(Debug, Clone)]
pub struct LocalHashEmbedder {
    dimensions: usize,
    model: String,
}

impl Default for LocalHashEmbedder {
    fn default() -> Self {
        Self::new(256)
    }
}

impl LocalHashEmbedder {
    pub fn new(dimensions: usize) -> Self {
        let dimensions = dimensions.max(8);
        Self { dimensions, model: format!("local-hash-{dimensions}") }
    }

    pub fn embed_one(&self, text: &str) -> Vec<f32> {
        let mut vector = vec![0.0f32; self.dimensions];
        let terms = terms(text);
        for term in &terms {
            self.add_feature(&mut vector, &format!("w:{term}"), WORD_WEIGHT);
            let padded: Vec<char> = format!("^{term}$").chars().collect();
            let trigrams = padded.len().saturating_sub(2).max(1);
            for window in padded.windows(3) {
                let gram: String = window.iter().collect();
                self.add_feature(&mut vector, &format!("g:{gram}"), TRIGRAM_BUDGET / trigrams as f32);
            }
        }
        for pair in terms.windows(2) {
            self.add_feature(&mut vector, &format!("p:{}_{}", pair[0], pair[1]), PAIR_WEIGHT);
        }
        let norm = vector.iter().map(|value| value * value).sum::<f32>().sqrt();
        if norm > 0.0 {
            vector.iter_mut().for_each(|value| *value /= norm);
        }
        vector
    }

    fn add_feature(&self, vector: &mut [f32], feature: &str, weight: f32) {
        let hash = fnv1a(feature.as_bytes());
        let slot = usize::try_from(hash % self.dimensions as u64).unwrap_or(0);
        let sign = if hash >> 63 == 0 { 1.0 } else { -1.0 };
        vector[slot] += sign * weight;
    }
}

fn terms(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_lowercase)
        .filter(|word| !STOPWORDS.contains(&word.as_str()))
        .map(|word| stem(&word))
        .collect()
}

fn stem(word: &str) -> String {
    for suffix in ["ingly", "edly", "ing", "ies", "es", "ed", "ly", "s"] {
        if let Some(root) = word.strip_suffix(suffix)
            && root.chars().count() >= 3
        {
            return if suffix == "ies" { format!("{root}y") } else { root.to_owned() };
        }
    }
    word.to_owned()
}

fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325u64, |hash, byte| (hash ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01b3))
}

#[async_trait]
impl Embedder for LocalHashEmbedder {
    fn model(&self) -> &str {
        &self.model
    }

    fn dimensions(&self) -> usize {
        self.dimensions
    }

    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, EmbedderError> {
        Ok(texts.iter().map(|text| self.embed_one(text)).collect())
    }
}
