//! Text embeddings: trait, local ONNX model (feature `embeddings`), deterministic hash fallback.
use std::collections::HashMap;
use std::sync::Mutex;

/// Returns one L2-normalised vector per input text.
pub trait Embedder: Send + Sync {
    fn embed(&self, texts: &[String]) -> anyhow::Result<Vec<Vec<f32>>>;
}

/// Cosine similarity (plain dot product for normalised inputs; safe for any).
pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let n = (a.iter().map(|x| x * x).sum::<f32>() * b.iter().map(|x| x * x).sum::<f32>()).sqrt();
    if n > 0.0 { dot / n } else { 0.0 }
}

fn normalise(v: &mut [f32]) {
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if n > 0.0 {
        v.iter_mut().for_each(|x| *x /= n);
    }
}

fn fnv(s: &str) -> u64 {
    s.bytes().fold(0xcbf29ce484222325, |h, b| (h ^ b as u64).wrapping_mul(0x100000001b3))
}

/// Per-process memo keyed by text hash (ponytail: unbounded map; clear when it passes CAP).
#[derive(Default)]
struct Memo(Mutex<HashMap<u64, Vec<f32>>>);
const CAP: usize = 20_000;

impl Memo {
    fn run(&self, texts: &[String], f: impl FnOnce(&[String]) -> anyhow::Result<Vec<Vec<f32>>>) -> anyhow::Result<Vec<Vec<f32>>> {
        let mut m = self.0.lock().map_err(|_| anyhow::anyhow!("embedding cache poisoned"))?;
        let miss: Vec<String> = texts.iter().filter(|t| !m.contains_key(&fnv(t))).cloned().collect();
        if !miss.is_empty() {
            let out = f(&miss)?;
            anyhow::ensure!(out.len() == miss.len(), "embedder returned {} vectors for {} texts", out.len(), miss.len());
            if m.len() + miss.len() > CAP {
                m.clear();
            }
            for (t, v) in miss.iter().zip(out) {
                m.insert(fnv(t), v);
            }
        }
        Ok(texts.iter().map(|t| m[&fnv(t)].clone()).collect())
    }
}

/// Deterministic hashed bag of words + char trigrams, dim 256. Offline fallback and test double.
#[derive(Default)]
pub struct HashEmbedder(Memo);
const HASH_DIM: usize = 256;

impl Embedder for HashEmbedder {
    fn embed(&self, texts: &[String]) -> anyhow::Result<Vec<Vec<f32>>> {
        self.0.run(texts, |texts| Ok(texts
            .iter()
            .map(|t| {
                let mut v = vec![0f32; HASH_DIM];
                for w in crate::jd::tokens(t) {
                    v[fnv(&w) as usize % HASH_DIM] += 1.0;
                    let p: Vec<char> = format!(" {w} ").chars().collect();
                    for g in p.windows(3) {
                        v[fnv(&g.iter().collect::<String>()) as usize % HASH_DIM] += 1.0;
                    }
                }
                normalise(&mut v);
                v
            })
            .collect()))
    }
}

#[cfg(feature = "embeddings")]
pub use fast::FastEmbedder;

#[cfg(feature = "embeddings")]
mod fast {
    use super::*;
    use fastembed::{EmbeddingModel, TextEmbedding, TextInitOptions};

    /// BGE-small-en-v1.5 (quantized) via ONNX Runtime. Model cache: $RESUME_MODEL_DIR or data/models.
    pub struct FastEmbedder {
        model: Mutex<TextEmbedding>,
        memo: Memo,
    }

    impl FastEmbedder {
        /// Clean Err (never panic) when the model is missing and cannot be downloaded.
        pub fn try_new() -> anyhow::Result<Self> {
            let dir = std::env::var("RESUME_MODEL_DIR").unwrap_or_else(|_| "data/models".into());
            let opts = TextInitOptions::new(EmbeddingModel::BGESmallENV15Q).with_cache_dir(dir.into()).with_show_download_progress(false);
            let model = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| TextEmbedding::try_new(opts)))
                .map_err(|_| anyhow::anyhow!("embedding model init panicked"))?
                .map_err(|e| anyhow::anyhow!("embedding model unavailable: {e}"))?;
            Ok(Self { model: Mutex::new(model), memo: Memo::default() })
        }
    }

    impl Embedder for FastEmbedder {
        fn embed(&self, texts: &[String]) -> anyhow::Result<Vec<Vec<f32>>> {
            self.memo.run(texts, |miss| {
                let mut m = self.model.lock().map_err(|_| anyhow::anyhow!("model lock poisoned"))?;
                let mut out = m.embed(miss, None).map_err(|e| anyhow::anyhow!("embed failed: {e}"))?;
                out.iter_mut().for_each(|v| normalise(v));
                Ok(out)
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_is_normalised_deterministic_and_related() {
        let s = |x: &str| x.to_string();
        let e = HashEmbedder::default();
        let v = e.embed(&[s("containerisation work"), s("containers work"), s("baking bread")]).unwrap();
        assert!((cosine(&v[0], &v[0]) - 1.0).abs() < 1e-5);
        assert!(cosine(&v[0], &v[1]) > cosine(&v[0], &v[2]) + 0.2);
        assert_eq!(v, e.embed(&[s("containerisation work"), s("containers work"), s("baking bread")]).unwrap());
    }

    #[test]
    fn memo_calls_inner_once() {
        let m = Memo::default();
        let t = vec!["a".to_string()];
        let one = |x: &[String]| Ok(vec![vec![1.0]; x.len()]);
        m.run(&t, one).unwrap();
        m.run(&t, |_| panic!("should be cached")).unwrap();
    }

    #[cfg(feature = "embeddings")]
    #[test]
    #[ignore]
    fn real_model_semantics() {
        let e = FastEmbedder::try_new().unwrap();
        let v = e.embed(&["Built REST APIs in C#".into(), "backend web services".into(), "baking sourdough bread".into()]).unwrap();
        let (a, b) = (cosine(&v[0], &v[1]), cosine(&v[0], &v[2]));
        eprintln!("related {a} unrelated {b}");
        assert!(a > b);
    }
}
