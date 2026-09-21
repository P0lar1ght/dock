//! Search / embedding / MMR config (Grok `xai-grok-config-types` memory section,
//! without the v2 name). Also hosts [`SearchResult`] shared by hybrid search + MMR.

use std::collections::HashMap;

use serde::Deserialize;

/// Hybrid search scoring (`[memory.search]`).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default)]
pub struct MemorySearchConfig {
    pub max_results: usize,
    pub min_score: f32,
    pub vector_weight: f32,
    pub text_weight: f32,
    pub temporal_decay: TemporalDecayConfig,
    pub mmr: MmrConfig,
    pub source_weights: HashMap<String, f32>,
}

impl Default for MemorySearchConfig {
    fn default() -> Self {
        let mut source_weights = HashMap::new();
        source_weights.insert("workspace".to_string(), 1.0);
        source_weights.insert("session".to_string(), 1.0);
        source_weights.insert("global".to_string(), 1.0);
        source_weights.insert("legacy".to_string(), 0.8);
        Self {
            max_results: 6,
            // Default gate for hybrid/vec. FTS-only raises to ~0.7 in search.rs.
            min_score: 0.35,
            vector_weight: 0.7,
            text_weight: 0.3,
            temporal_decay: TemporalDecayConfig::default(),
            mmr: MmrConfig::default(),
            source_weights,
        }
    }
}

impl MemorySearchConfig {
    /// Half-life days when temporal decay is enabled; `None` disables decay.
    pub fn effective_half_life_days(&self) -> Option<f64> {
        if self.temporal_decay.enabled {
            Some(self.temporal_decay.half_life_days)
        } else {
            None
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default)]
pub struct TemporalDecayConfig {
    pub enabled: bool,
    pub half_life_days: f64,
}

impl Default for TemporalDecayConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            half_life_days: 30.0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default)]
pub struct MmrConfig {
    pub enabled: bool,
    pub lambda: f64,
}

impl Default for MmrConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            lambda: 0.7,
        }
    }
}

/// Embedding provider knobs (`[memory.embedding]`).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default)]
pub struct MemoryEmbeddingConfig {
    /// Model name. `None` / empty → FTS-only (no vector index writes).
    pub model: Option<String>,
    /// OpenAI-compatible API base (e.g. `https://api.openai.com/v1`).
    pub base: Option<String>,
    /// Bearer API key (or from env later). Missing → FTS-only.
    pub api_key: Option<String>,
    pub dimensions: usize,
}

impl Default for MemoryEmbeddingConfig {
    fn default() -> Self {
        Self {
            model: None,
            base: None,
            api_key: None,
            dimensions: 1024,
        }
    }
}

impl MemoryEmbeddingConfig {
    pub fn is_configured(&self) -> bool {
        self.model.as_ref().is_some_and(|m| !m.is_empty())
            && self.base.as_ref().is_some_and(|b| !b.is_empty())
    }
}

/// A search hit with merged scoring from FTS and (optional) vector search.
#[derive(Debug, Clone)]
pub struct SearchResult {
    pub chunk_id: String,
    pub path: String,
    pub start_line: usize,
    pub end_line: usize,
    pub score: f64,
    pub snippet: String,
    pub source: String,
    pub created_at: i64,
}
