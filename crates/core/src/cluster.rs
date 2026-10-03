//! Deterministic k-means over embeddings, tf-idf cluster labels, PCA 2-D projection.
use crate::embed::{cosine, Embedder};
use crate::jd::tokens;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Point {
    pub id: String,
    pub label: String,
    pub x: f32,
    pub y: f32,
    pub cluster: usize,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct ClusterInfo {
    pub id: usize,
    pub label: String,
    pub size: usize,
}

#[derive(Serialize, Clone, Debug, Default, PartialEq)]
pub struct Clusters {
    pub points: Vec<Point>,
    pub clusters: Vec<ClusterInfo>,
}

const SEED: u64 = 0x9e3779b97f4a7c15;
const ITERS: usize = 50;
const STOP: &str = "the and for with from that this into using used use via per over under all any are was were has have had its our your their can will but not out new more than also built build work worked working team teams";

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> f64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 11) as f64 / (1u64 << 53) as f64
    }
}

fn d2(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| (x - y) * (x - y)).sum()
}

fn nearest(v: &[f32], cs: &[Vec<f32>]) -> usize {
    (0..cs.len()).min_by(|&a, &b| d2(v, &cs[a]).total_cmp(&d2(v, &cs[b])).then(a.cmp(&b))).unwrap_or(0)
}

fn kmeans(v: &[Vec<f32>], k: usize) -> Vec<usize> {
    let mut rng = Rng(SEED);
    let mut cs = vec![v[(rng.next() * v.len() as f64) as usize % v.len()].clone()];
    while cs.len() < k {
        let d: Vec<f32> = v.iter().map(|x| cs.iter().map(|c| d2(x, c)).fold(f32::MAX, f32::min)).collect();
        let total: f32 = d.iter().sum();
        let pick = if total <= 0.0 {
            cs.len() % v.len()
        } else {
            let mut t = rng.next() as f32 * total;
            d.iter().position(|&w| { t -= w; t <= 0.0 }).unwrap_or(v.len() - 1)
        };
        cs.push(v[pick].clone());
    }
    let mut asg = vec![usize::MAX; v.len()];
    for _ in 0..ITERS {
        let next: Vec<usize> = v.iter().map(|x| nearest(x, &cs)).collect();
        if next == asg {
            break;
        }
        asg = next;
        for (c, cen) in cs.iter_mut().enumerate() {
            let m: Vec<&Vec<f32>> = v.iter().zip(&asg).filter(|(_, &a)| a == c).map(|(x, _)| x).collect();
            if !m.is_empty() {
                *cen = (0..cen.len()).map(|i| m.iter().map(|x| x[i]).sum::<f32>() / m.len() as f32).collect();
            }
        }
    }
    asg
}

/// Top-3 words most characteristic of each cluster: p_in * ln((p_in+e)/(p_out+e)) over document frequency.
fn labels(texts: &[String], asg: &[usize], k: usize) -> Vec<String> {
    let stop: BTreeSet<&str> = STOP.split_whitespace().collect();
    let docs: Vec<BTreeSet<String>> = texts.iter().map(|t| tokens(t).into_iter().filter(|w| w.len() >= 3 && !stop.contains(w.as_str()) && !w.chars().all(|c| c.is_ascii_digit())).collect()).collect();
    let mut df: Vec<BTreeMap<&str, usize>> = vec![BTreeMap::new(); k];
    let mut size = vec![0usize; k];
    for (d, &c) in docs.iter().zip(asg) {
        size[c] += 1;
        d.iter().for_each(|w| *df[c].entry(w).or_default() += 1);
    }
    let n = docs.len();
    (0..k)
        .map(|c| {
            let out = |w: &str| (0..k).filter(|&o| o != c).map(|o| df[o].get(w).copied().unwrap_or(0)).sum::<usize>() as f32 / (n - size[c]).max(1) as f32;
            let mut s: Vec<(f32, &str)> = df[c].iter().map(|(w, &f)| {
                let p = f as f32 / size[c] as f32;
                (p * ((p + 0.01) / (out(w) + 0.01)).ln(), *w)
            }).collect();
            s.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(b.1)));
            s.iter().take(3).map(|x| x.1).collect::<Vec<_>>().join(" ")
        })
        .collect()
}

fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

/// Top principal axis of centred `x`, orthogonal to `ortho` if given (power iteration, fixed start).
fn axis(x: &[Vec<f32>], ortho: Option<&[f32]>) -> Vec<f32> {
    let dim = x[0].len();
    let mut v: Vec<f32> = (0..dim).map(|i| 1.0 + (i % 7) as f32 * 0.1).collect();
    for _ in 0..100 {
        let mut w = vec![0f32; dim];
        for r in x {
            let p = dot(r, &v);
            w.iter_mut().zip(r).for_each(|(a, b)| *a += p * b);
        }
        if let Some(o) = ortho {
            let p = dot(&w, o);
            w.iter_mut().zip(o).for_each(|(a, b)| *a -= p * b);
        }
        let n = dot(&w, &w).sqrt();
        if n < 1e-12 {
            break;
        }
        v = w.iter().map(|a| a / n).collect();
    }
    // sign convention: largest component positive
    if v.iter().copied().max_by(|a, b| a.abs().total_cmp(&b.abs())).is_some_and(|m| m < 0.0) {
        v.iter_mut().for_each(|a| *a = -*a);
    }
    v
}

fn project(v: &[Vec<f32>]) -> Vec<(f32, f32)> {
    let (n, dim) = (v.len(), v[0].len());
    let mean: Vec<f32> = (0..dim).map(|i| v.iter().map(|r| r[i]).sum::<f32>() / n as f32).collect();
    let x: Vec<Vec<f32>> = v.iter().map(|r| r.iter().zip(&mean).map(|(a, m)| a - m).collect()).collect();
    let a1 = axis(&x, None);
    let a2 = axis(&x, Some(&a1));
    let mut p: Vec<(f32, f32)> = x.iter().map(|r| (dot(r, &a1), dot(r, &a2))).collect();
    let (mx, my) = p.iter().fold((0f32, 0f32), |m, q| (m.0.max(q.0.abs()), m.1.max(q.1.abs())));
    p.iter_mut().for_each(|q| {
        q.0 = if mx > 1e-9 { q.0 / mx } else { 0.0 };
        q.1 = if my > 1e-9 { q.1 / my } else { 0.0 };
    });
    p
}

/// items: (id, short label, text to embed). k = clamp(sqrt(n/2), 2, 8), capped at n.
pub fn cluster(items: &[(String, String, String)], emb: &dyn Embedder) -> Clusters {
    if items.is_empty() {
        return Clusters::default();
    }
    let texts: Vec<String> = items.iter().map(|i| i.2.clone()).collect();
    let v = match emb.embed(&texts) {
        Ok(v) if v.len() == items.len() => v,
        _ => return Clusters::default(),
    };
    let k = ((items.len() as f32 / 2.0).sqrt().round() as usize).clamp(2, 8).min(items.len());
    let asg = kmeans(&v, k);
    let names = labels(&texts, &asg, k);
    let pts = project(&v);
    Clusters {
        points: items.iter().zip(&asg).zip(pts).map(|((i, &c), (x, y))| Point { id: i.0.clone(), label: i.1.clone(), x, y, cluster: c }).collect(),
        clusters: (0..k).map(|c| ClusterInfo { id: c, label: names[c].clone(), size: asg.iter().filter(|&&a| a == c).count() }).collect(),
    }
}

/// Pairs (id_a, id_b, cosine) with cosine >= min, best first.
pub fn near_duplicates(items: &[(String, String, String)], emb: &dyn Embedder, min: f32) -> Vec<(String, String, f32)> {
    let texts: Vec<String> = items.iter().map(|i| i.2.clone()).collect();
    let Ok(v) = emb.embed(&texts) else { return Vec::new() };
    let mut out = Vec::new();
    for i in 0..v.len().min(items.len()) {
        for j in i + 1..v.len().min(items.len()) {
            let c = cosine(&v[i], &v[j]);
            if c >= min {
                out.push((items[i].0.clone(), items[j].0.clone(), c));
            }
        }
    }
    out.sort_by(|a, b| b.2.total_cmp(&a.2).then(a.0.cmp(&b.0)).then(a.1.cmp(&b.1)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embed::HashEmbedder;

    fn items() -> Vec<(String, String, String)> {
        let t = [
            "Deployed kubernetes containers with docker pipelines",
            "Automated kubernetes cluster deployments and docker images",
            "Tuned kubernetes containers and docker registry",
            "Baked sourdough bread and pastry for the bakery",
            "Decorated pastry cakes and bread in the bakery",
            "Sold fresh bread, pastry and cakes at the bakery",
        ];
        t.iter().enumerate().map(|(i, x)| (format!("b{i}"), format!("l{i}"), x.to_string())).collect()
    }

    #[test]
    fn deterministic_separated_bounded() {
        let c = cluster(&items(), &HashEmbedder::default());
        assert_eq!(c, cluster(&items(), &HashEmbedder::default()));
        let g = |i: usize| c.points[i].cluster;
        assert_eq!((g(0), g(1)), (g(1), g(2)));
        assert_eq!((g(3), g(4)), (g(4), g(5)));
        assert_ne!(g(0), g(3));
        assert!(c.points.iter().all(|p| p.x.abs() <= 1.0001 && p.y.abs() <= 1.0001));
        assert!(c.points.iter().any(|p| (p.x.abs() - 1.0).abs() < 1e-4));
        assert_eq!(c.clusters.iter().map(|k| k.size).sum::<usize>(), 6);
        assert!(c.clusters[g(0)].label.contains("kubernetes") || c.clusters[g(0)].label.contains("docker"), "{:?}", c.clusters);
        assert!(serde_json::to_string(&c).is_ok());
    }

    #[test]
    fn dedupes_and_tiny_inputs() {
        let mut it = items();
        it.push(("dup".into(), "d".into(), it[0].2.clone()));
        let d = near_duplicates(&it, &HashEmbedder::default(), 0.92);
        assert_eq!((d[0].0.as_str(), d[0].1.as_str()), ("b0", "dup"));
        assert!(d.iter().all(|x| x.2 >= 0.92));
        assert!(cluster(&[], &HashEmbedder::default()).points.is_empty());
        assert_eq!(cluster(&it[..1], &HashEmbedder::default()).points.len(), 1);
    }
}
