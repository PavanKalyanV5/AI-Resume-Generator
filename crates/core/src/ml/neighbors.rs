//! Nearest past JDs by cosine similarity.
use crate::embed::{cosine, Embedder};

const DIM: usize = 512;
const STOP: [&str; 24] = ["the", "a", "an", "and", "or", "of", "to", "in", "for", "with", "on", "as", "is", "are", "be", "we", "you", "our", "will", "your", "at", "by", "this", "that"];

fn fnv(s: &str) -> u64 {
    s.bytes().fold(0xcbf29ce484222325, |h, b| (h ^ b as u64).wrapping_mul(0x100000001b3))
}

/// Top-k (id, cosine) with similarity >= `min_sim`, best first (ties by id). Query and past
/// vectors must come from the same source (all embedder or all hashed).
pub fn similar_jobs(query: &[f32], past: &[(String, Vec<f32>)], k: usize, min_sim: f32) -> Vec<(String, f32)> {
    let mut v: Vec<(String, f32)> = past.iter().map(|(id, x)| (id.clone(), cosine(query, x))).filter(|(_, s)| *s >= min_sim).collect();
    v.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    v.truncate(k);
    v
}

/// JD vector: the embedder's vector when it works, else hashed sublinear-tf unigrams (stopwords
/// dropped), L2-normalised, 512 dims. Store which kind you used alongside persisted vectors.
pub fn jd_vector(text: &str, emb: Option<&dyn Embedder>) -> Vec<f32> {
    if let Some(v) = emb.and_then(|e| e.embed(&[text.to_string()]).ok()).and_then(|mut v| v.pop()).filter(|v| !v.is_empty()) {
        return v;
    }
    let mut v = vec![0f32; DIM];
    for t in crate::jd::tokens(text).iter().filter(|t| !STOP.contains(&t.as_str())) {
        v[fnv(t) as usize % DIM] += 1.0;
    }
    v.iter_mut().for_each(|x| *x = if *x > 0.0 { 1.0 + x.ln() } else { 0.0 });
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if n > 0.0 {
        v.iter_mut().for_each(|x| *x /= n);
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_similar_by_hashed_vectors() {
        let past = vec![
            ("be".to_string(), jd_vector("Backend engineer Java Spring microservices PostgreSQL Kafka", None)),
            ("fe".to_string(), jd_vector("Frontend developer React TypeScript CSS accessibility", None)),
        ];
        let q = jd_vector("Senior backend engineer: Java, Spring Boot, Kafka and PostgreSQL", None);
        let r = similar_jobs(&q, &past, 2, 0.0);
        assert_eq!(r[0].0, "be");
        assert!(similar_jobs(&q, &past, 2, 0.99).is_empty());
        assert_eq!(similar_jobs(&q, &past, 1, 0.0).len(), 1);
    }
}
