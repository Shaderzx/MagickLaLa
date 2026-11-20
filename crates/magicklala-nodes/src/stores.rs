use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};

use async_trait::async_trait;
use magicklala_graph::{KnowledgeHit, KnowledgeService, MemoryItem, MemoryService, NodeError};

use crate::nodes::split_chunks;

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn relevance(query: &str, content: &str) -> f32 {
    let haystack = content.to_lowercase();
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    if words.is_empty() {
        return 0.0;
    }
    let hits = words.iter().filter(|word| haystack.contains(word.as_str())).count();
    hits as f32 / words.len() as f32
}

fn ranked<T: Clone>(items: &[T], limit: usize, score: impl Fn(&T) -> f32) -> Vec<(T, f32)> {
    let mut scored: Vec<(T, f32)> = items.iter().map(|item| (item.clone(), score(item))).filter(|(_, score)| *score > 0.0).collect();
    scored.sort_by(|left, right| right.1.total_cmp(&left.1));
    scored.truncate(limit);
    scored
}

#[derive(Default)]
pub struct InMemoryMemory {
    items: Mutex<BTreeMap<String, Vec<MemoryItem>>>,
    next_id: AtomicU64,
}

impl InMemoryMemory {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl MemoryService for InMemoryMemory {
    async fn add(&self, agent_id: &str, content: &str, metadata: serde_json::Value) -> Result<String, NodeError> {
        let id = format!("mem-{}", self.next_id.fetch_add(1, Ordering::SeqCst) + 1);
        locked(&self.items).entry(agent_id.to_owned()).or_default().push(MemoryItem { id: id.clone(), content: content.to_owned(), score: 0.0, metadata });
        Ok(id)
    }

    async fn recall(&self, agent_id: &str, query: &str, limit: usize) -> Result<Vec<MemoryItem>, NodeError> {
        let items = locked(&self.items).get(agent_id).cloned().unwrap_or_default();
        Ok(ranked(&items, limit, |item| relevance(query, &item.content)).into_iter().map(|(item, score)| MemoryItem { score, ..item }).collect())
    }

    async fn clear(&self, agent_id: &str) -> Result<u64, NodeError> {
        let removed = locked(&self.items).remove(agent_id).map_or(0, |items| items.len());
        Ok(u64::try_from(removed).unwrap_or(u64::MAX))
    }
}

#[derive(Default)]
pub struct InMemoryKnowledge {
    chunks: Mutex<BTreeMap<String, Vec<KnowledgeHit>>>,
    next_id: AtomicU64,
}

impl InMemoryKnowledge {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl KnowledgeService for InMemoryKnowledge {
    async fn add(&self, agent_id: &str, title: &str, content: &str) -> Result<String, NodeError> {
        let doc_id = format!("doc-{}", self.next_id.fetch_add(1, Ordering::SeqCst) + 1);
        let body = if title.is_empty() { content.to_owned() } else { format!("{title}\n{content}") };
        let hits = split_chunks(&body, 500, 50).into_iter().enumerate().map(|(index, text)| KnowledgeHit {
            doc_id: doc_id.clone(),
            chunk_index: u32::try_from(index).unwrap_or(u32::MAX),
            text,
            score: 0.0,
        });
        locked(&self.chunks).entry(agent_id.to_owned()).or_default().extend(hits);
        Ok(doc_id)
    }

    async fn search(&self, agent_id: &str, query: &str, limit: usize) -> Result<Vec<KnowledgeHit>, NodeError> {
        let chunks = locked(&self.chunks).get(agent_id).cloned().unwrap_or_default();
        Ok(ranked(&chunks, limit, |hit| relevance(query, &hit.text)).into_iter().map(|(hit, score)| KnowledgeHit { score, ..hit }).collect())
    }
}
