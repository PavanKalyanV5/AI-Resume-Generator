//! Small, deterministic ML primitives. No dependencies beyond serde; everything serialises.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub type Xs = [Vec<f32>];

/// Tiny deterministic xorshift64* generator.
#[derive(Clone, Debug)]
pub struct Rng(u64);
impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }
    pub fn next_u64(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    /// Uniform in [0, 1).
    pub fn f32(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32
    }
    pub fn shuffle<T>(&mut self, v: &mut [T]) {
        for i in (1..v.len()).rev() {
            v.swap(i, (self.next_u64() % (i as u64 + 1)) as usize);
        }
    }
}

pub fn sigmoid(z: f32) -> f32 {
    1.0 / (1.0 + (-z.clamp(-30.0, 30.0)).exp())
}

/// Solve A x = b by Gauss-Jordan with partial pivoting (small dims). None if singular.
pub fn solve(mut a: Vec<Vec<f64>>, mut b: Vec<f64>) -> Option<Vec<f64>> {
    let n = b.len();
    for c in 0..n {
        let p = (c..n).max_by(|&i, &j| a[i][c].abs().total_cmp(&a[j][c].abs()))?;
        if a[p][c].abs() < 1e-12 {
            return None;
        }
        a.swap(c, p);
        b.swap(c, p);
        let d = a[c][c];
        a[c].iter_mut().for_each(|v| *v /= d);
        b[c] /= d;
        for r in 0..n {
            if r != c {
                let f = a[r][c];
                if f != 0.0 {
                    let row = a[c].clone();
                    a[r].iter_mut().zip(&row).for_each(|(v, w)| *v -= f * w);
                    b[r] -= f * b[c];
                }
            }
        }
    }
    Some(b)
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Standardizer {
    pub mean: Vec<f32>,
    pub std: Vec<f32>,
}
impl Standardizer {
    pub fn fit(x: &Xs) -> Self {
        let d = x.first().map_or(0, |r| r.len());
        let n = x.len().max(1) as f32;
        let mean: Vec<f32> = (0..d).map(|j| x.iter().map(|r| r[j]).sum::<f32>() / n).collect();
        let std = (0..d).map(|j| ((x.iter().map(|r| (r[j] - mean[j]).powi(2)).sum::<f32>() / n).sqrt()).max(1e-6)).collect();
        Standardizer { mean, std }
    }
    pub fn transform(&self, r: &[f32]) -> Vec<f32> {
        r.iter().enumerate().map(|(j, v)| (v - self.mean.get(j).copied().unwrap_or(0.0)) / self.std.get(j).copied().unwrap_or(1.0)).collect()
    }
    pub fn transform_all(&self, x: &Xs) -> Vec<Vec<f32>> {
        x.iter().map(|r| self.transform(r)).collect()
    }
}

/// Logistic regression, fitted by damped IRLS (Newton) with L2 on weights. Deterministic.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LogReg {
    pub w: Vec<f32>,
    pub b: f32,
    pub l2: f32,
}
impl LogReg {
    pub fn new(w: Vec<f32>, b: f32) -> Self {
        LogReg { w, b, l2: 1.0 }
    }
    pub fn logit(&self, x: &[f32]) -> f32 {
        self.b + self.w.iter().zip(x).map(|(w, x)| w * x).sum::<f32>()
    }
    pub fn predict_proba(&self, x: &[f32]) -> f32 {
        sigmoid(self.logit(x))
    }
    /// Data fit shrunk toward `prior`: w = (n*w_data + k*w_prior) / (n + k). `k_prior` is the
    /// prior's pseudo-sample count. Without a prior (or n == 0) behaves as plain/ no-op fit.
    pub fn fit(x: &Xs, y: &[f32], prior: Option<&LogReg>, k_prior: f32) -> LogReg {
        Self::fit_with(x, y, prior, k_prior, 1.0, false)
    }
    pub fn fit_with(x: &Xs, y: &[f32], prior: Option<&LogReg>, k_prior: f32, l2: f32, balanced: bool) -> LogReg {
        let n = x.len();
        let d = x.first().map_or_else(|| prior.map_or(0, |p| p.w.len()), |r| r.len());
        if n == 0 {
            return prior.cloned().unwrap_or(LogReg { w: vec![0.0; d], b: 0.0, l2 });
        }
        let pos = y.iter().filter(|&&v| v > 0.5).count() as f64;
        let neg = n as f64 - pos;
        let sw: Vec<f64> = y.iter().map(|&v| if !balanced || pos == 0.0 || neg == 0.0 { 1.0 } else if v > 0.5 { n as f64 / (2.0 * pos) } else { n as f64 / (2.0 * neg) }).collect();
        let mut th = vec![0.0f64; d + 1]; // last = bias
        for _ in 0..30 {
            let mut g = vec![0.0f64; d + 1];
            let mut h = vec![vec![0.0f64; d + 1]; d + 1];
            for (i, r) in x.iter().enumerate() {
                let z: f64 = th[d] + r.iter().enumerate().map(|(j, v)| th[j] * *v as f64).sum::<f64>();
                let p = 1.0 / (1.0 + (-z.clamp(-30.0, 30.0)).exp());
                let (e, s) = (sw[i] * (p - y[i] as f64), sw[i] * p * (1.0 - p) + 1e-9);
                let xi: Vec<f64> = r.iter().map(|&v| v as f64).chain(std::iter::once(1.0)).collect();
                for a in 0..=d {
                    g[a] += e * xi[a];
                    for c in 0..=d {
                        h[a][c] += s * xi[a] * xi[c];
                    }
                }
            }
            for j in 0..d {
                g[j] += l2 as f64 * th[j];
                h[j][j] += l2 as f64;
            }
            h[d][d] += 1e-6;
            let Some(step) = solve(h, g) else { break };
            let mx = step.iter().fold(0.0f64, |m, v| m.max(v.abs()));
            let sc = if mx > 4.0 { 4.0 / mx } else { 1.0 };
            th.iter_mut().zip(&step).for_each(|(t, s)| *t -= s * sc);
            if mx * sc < 1e-6 {
                break;
            }
        }
        let mut m = LogReg { w: th[..d].iter().map(|&v| v as f32).collect(), b: th[d] as f32, l2 };
        if let Some(p) = prior.filter(|p| p.w.len() == d) {
            let (n, k) = (n as f32, k_prior.max(0.0));
            m.w.iter_mut().zip(&p.w).for_each(|(w, pw)| *w = (n * *w + k * pw) / (n + k));
            m.b = (n * m.b + k * p.b) / (n + k);
        }
        m
    }
}

/// Platt scaling: p = sigmoid(a*score + b).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Platt {
    pub a: f32,
    pub b: f32,
}
impl Default for Platt {
    fn default() -> Self {
        Platt { a: 1.0, b: 0.0 }
    }
}
impl Platt {
    pub fn fit(scores: &[f32], y: &[f32]) -> Platt {
        let x: Vec<Vec<f32>> = scores.iter().map(|&s| vec![s]).collect();
        let m = LogReg::fit_with(&x, y, None, 0.0, 0.1, false);
        Platt { a: m.w.first().copied().unwrap_or(1.0), b: m.b }
    }
    pub fn apply(&self, s: f32) -> f32 {
        sigmoid(self.a * s + self.b)
    }
}

/// Ridge regression (closed form), intercept unpenalised. Optional shrinkage to a prior like LogReg.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Ridge {
    pub w: Vec<f32>,
    pub b: f32,
    pub l2: f32,
}
impl Ridge {
    pub fn predict(&self, x: &[f32]) -> f32 {
        self.b + self.w.iter().zip(x).map(|(w, x)| w * x).sum::<f32>()
    }
    pub fn fit(x: &Xs, y: &[f32], l2: f32) -> Ridge {
        Self::fit_prior(x, y, l2, None, 0.0)
    }
    pub fn fit_prior(x: &Xs, y: &[f32], l2: f32, prior: Option<&Ridge>, k_prior: f32) -> Ridge {
        let n = x.len();
        let d = x.first().map_or_else(|| prior.map_or(0, |p| p.w.len()), |r| r.len());
        if n == 0 {
            return prior.cloned().unwrap_or(Ridge { w: vec![0.0; d], b: 0.0, l2 });
        }
        let mut a = vec![vec![0.0f64; d + 1]; d + 1];
        let mut r = vec![0.0f64; d + 1];
        for (row, &t) in x.iter().zip(y) {
            let xi: Vec<f64> = row.iter().map(|&v| v as f64).chain(std::iter::once(1.0)).collect();
            for i in 0..=d {
                r[i] += xi[i] * t as f64;
                for j in 0..=d {
                    a[i][j] += xi[i] * xi[j];
                }
            }
        }
        for (j, row) in a.iter_mut().enumerate().take(d) {
            row[j] += l2 as f64;
        }
        a[d][d] += 1e-9;
        let th = solve(a, r).unwrap_or_else(|| vec![0.0; d + 1]);
        let mut m = Ridge { w: th[..d].iter().map(|&v| v as f32).collect(), b: th[d] as f32, l2 };
        if let Some(p) = prior.filter(|p| p.w.len() == d) {
            let (n, k) = (n as f32, k_prior.max(0.0));
            m.w.iter_mut().zip(&p.w).for_each(|(w, pw)| *w = (n * *w + k * pw) / (n + k));
            m.b = (n * m.b + k * p.b) / (n + k);
        }
        m
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TokStat {
    pub counts: Vec<f32>,
    pub df: f32,
}

/// Multinomial naive Bayes over tokens with Laplace smoothing, sublinear tf and idf weighting,
/// and online `update`. Likelihoods are tempered for long docs so probabilities stay honest.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct NaiveBayes {
    pub classes: Vec<String>,
    pub docs: Vec<f32>,
    pub totals: Vec<f32>,
    pub vocab: BTreeMap<String, TokStat>,
    pub alpha: f32,
}
impl NaiveBayes {
    pub fn new(classes: &[&str]) -> Self {
        NaiveBayes { classes: classes.iter().map(|s| s.to_string()).collect(), docs: vec![0.0; classes.len()], totals: vec![0.0; classes.len()], vocab: BTreeMap::new(), alpha: 1.0 }
    }
    fn tf(tokens: &[String]) -> BTreeMap<&str, f32> {
        let mut m = BTreeMap::new();
        for t in tokens {
            *m.entry(t.as_str()).or_insert(0.0) += 1.0;
        }
        m
    }
    /// Online update with one labelled document (`weight` > 1 to emphasise user corrections).
    pub fn update(&mut self, tokens: &[String], class: &str, weight: f32) {
        let Some(c) = self.classes.iter().position(|k| k == class) else { return };
        let nc = self.classes.len();
        self.docs[c] += weight;
        for (t, n) in Self::tf(tokens) {
            let s = self.vocab.entry(t.to_string()).or_insert_with(|| TokStat { counts: vec![0.0; nc], df: 0.0 });
            let w = weight * (1.0 + n.ln());
            s.counts[c] += w;
            s.df += weight;
            self.totals[c] += w;
        }
    }
    fn scores(&self, tokens: &[String]) -> (Vec<f32>, Vec<(String, Vec<f32>)>) {
        let nc = self.classes.len();
        let n_docs: f32 = self.docs.iter().sum();
        let v = self.vocab.len() as f32;
        let mut ll = vec![0.0f32; nc];
        let mut per_tok = Vec::new();
        let mut known = 0.0f32;
        for (t, n) in Self::tf(tokens) {
            let Some(s) = self.vocab.get(t) else { continue };
            let idf = ((n_docs + 1.0) / (s.df + 1.0)).ln() + 1.0;
            let w = idf * (1.0 + n.ln());
            known += 1.0;
            let lp: Vec<f32> = (0..nc).map(|c| w * ((s.counts[c] + self.alpha) / (self.totals[c] + self.alpha * v)).ln()).collect();
            ll.iter_mut().zip(&lp).for_each(|(a, b)| *a += b);
            per_tok.push((t.to_string(), lp));
        }
        let temper = (known / 20.0).max(1.0).sqrt();
        let sc = (0..nc).map(|c| ((self.docs[c] + 1.0) / (n_docs + nc as f32)).ln() + ll[c] / temper).collect();
        (sc, per_tok)
    }
    /// (class, probability) in class order.
    pub fn predict(&self, tokens: &[String]) -> Vec<(String, f32)> {
        let (s, _) = self.scores(tokens);
        let m = s.iter().copied().fold(f32::MIN, f32::max);
        let e: Vec<f32> = s.iter().map(|v| (v - m).exp()).collect();
        let z: f32 = e.iter().sum::<f32>().max(f32::MIN_POSITIVE);
        self.classes.iter().cloned().zip(e.iter().map(|v| v / z)).collect()
    }
    /// Tokens that most favour the best class over the average of the others.
    pub fn evidence(&self, tokens: &[String], k: usize) -> Vec<String> {
        let (s, per) = self.scores(tokens);
        let Some(best) = (0..s.len()).max_by(|&a, &b| s[a].total_cmp(&s[b])) else { return vec![] };
        let nc = self.classes.len().max(2) as f32 - 1.0;
        let mut v: Vec<(String, f32)> = per.into_iter().map(|(t, lp)| {
            let others: f32 = lp.iter().enumerate().filter(|(c, _)| *c != best).map(|(_, x)| x).sum::<f32>() / nc;
            (t, lp[best] - others)
        }).collect();
        v.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        v.into_iter().filter(|x| x.1 > 0.0).take(k).map(|x| x.0).collect()
    }
}

/// Cosine k-nearest-neighbour regressor/classifier (similarity-weighted mean of labels).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Knn {
    pub xs: Vec<Vec<f32>>,
    pub ys: Vec<f32>,
}
impl Knn {
    pub fn push(&mut self, x: Vec<f32>, y: f32) {
        self.xs.push(x);
        self.ys.push(y);
    }
    /// Top-k (index, cosine) pairs, best first.
    pub fn neighbors(&self, q: &[f32], k: usize) -> Vec<(usize, f32)> {
        let mut v: Vec<(usize, f32)> = self.xs.iter().enumerate().map(|(i, x)| (i, crate::embed::cosine(q, x))).collect();
        v.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        v.truncate(k);
        v
    }
    pub fn predict(&self, q: &[f32], k: usize) -> Option<f32> {
        let nb: Vec<_> = self.neighbors(q, k).into_iter().filter(|x| x.1 > 0.0).collect();
        let z: f32 = nb.iter().map(|x| x.1).sum();
        (z > 0.0).then(|| nb.iter().map(|&(i, s)| s * self.ys[i]).sum::<f32>() / z)
    }
}

// ---- metrics (y = truth, p = prediction / probability) ----
pub fn accuracy(y: &[f32], p: &[f32]) -> f32 {
    if y.is_empty() { return 0.0 }
    y.iter().zip(p).filter(|(a, b)| (**a > 0.5) == (**b >= 0.5)).count() as f32 / y.len() as f32
}
pub fn f1(y: &[f32], p: &[f32]) -> f32 {
    let (mut tp, mut fp, mut fn_) = (0.0, 0.0, 0.0);
    for (a, b) in y.iter().zip(p) {
        match (*a > 0.5, *b >= 0.5) {
            (true, true) => tp += 1.0,
            (false, true) => fp += 1.0,
            (true, false) => fn_ += 1.0,
            _ => {}
        }
    }
    if tp == 0.0 { 0.0 } else { 2.0 * tp / (2.0 * tp + fp + fn_) }
}
/// Rank-based AUC with tie averaging; 0.5 when only one class present.
pub fn auc(y: &[f32], p: &[f32]) -> f32 {
    let (np, nn) = (y.iter().filter(|&&v| v > 0.5).count() as f32, y.iter().filter(|&&v| v <= 0.5).count() as f32);
    if np == 0.0 || nn == 0.0 { return 0.5 }
    let mut idx: Vec<usize> = (0..y.len()).collect();
    idx.sort_by(|&a, &b| p[a].total_cmp(&p[b]));
    let mut rank_sum = 0.0;
    let mut i = 0;
    while i < idx.len() {
        let mut j = i;
        while j + 1 < idx.len() && p[idx[j + 1]] == p[idx[i]] { j += 1 }
        let r = (i + j) as f32 / 2.0 + 1.0;
        rank_sum += idx[i..=j].iter().filter(|&&k| y[k] > 0.5).count() as f32 * r;
        i = j + 1;
    }
    (rank_sum - np * (np + 1.0) / 2.0) / (np * nn)
}
pub fn mae(y: &[f32], p: &[f32]) -> f32 {
    if y.is_empty() { return 0.0 }
    y.iter().zip(p).map(|(a, b)| (a - b).abs()).sum::<f32>() / y.len() as f32
}
pub fn r2(y: &[f32], p: &[f32]) -> f32 {
    let m = y.iter().sum::<f32>() / y.len().max(1) as f32;
    let sst: f32 = y.iter().map(|a| (a - m).powi(2)).sum();
    if sst <= 0.0 { return 0.0 }
    1.0 - y.iter().zip(p).map(|(a, b)| (a - b).powi(2)).sum::<f32>() / sst
}
pub fn brier(y: &[f32], p: &[f32]) -> f32 {
    if y.is_empty() { return 0.0 }
    y.iter().zip(p).map(|(a, b)| (a - b).powi(2)).sum::<f32>() / y.len() as f32
}

/// Out-of-fold score: k=5 folds, or leave-one-out when n < 20. `predict(train_x, train_y, test_x)`
/// returns predictions; `metric(truth, preds)` is applied once to the pooled predictions (so AUC
/// etc. work with LOO). None when n < 4.
pub fn cross_val_score(
    x: &Xs,
    y: &[f32],
    seed: u64,
    predict: impl Fn(&Xs, &[f32], &Xs) -> Vec<f32>,
    metric: impl Fn(&[f32], &[f32]) -> f32,
) -> Option<f32> {
    let n = x.len();
    if n < 4 || y.len() != n {
        return None;
    }
    let k = if n < 20 { n } else { 5 };
    let mut order: Vec<usize> = (0..n).collect();
    Rng::new(seed).shuffle(&mut order);
    let mut preds = vec![0.0f32; n];
    for f in 0..k {
        let test: Vec<usize> = order.iter().enumerate().filter(|(p, _)| p % k == f).map(|(_, &i)| i).collect();
        let train: Vec<usize> = order.iter().enumerate().filter(|(p, _)| p % k != f).map(|(_, &i)| i).collect();
        let (tx, ty): (Vec<Vec<f32>>, Vec<f32>) = train.iter().map(|&i| (x[i].clone(), y[i])).unzip();
        let qx: Vec<Vec<f32>> = test.iter().map(|&i| x[i].clone()).collect();
        for (&i, p) in test.iter().zip(predict(&tx, &ty, &qx)) {
            preds[i] = p;
        }
    }
    Some(metric(y, &preds))
}

/// Score on a fixed held-out quarter as the training set grows to each of `sizes`.
pub fn learning_curve(
    x: &Xs,
    y: &[f32],
    sizes: &[usize],
    seed: u64,
    predict: impl Fn(&Xs, &[f32], &Xs) -> Vec<f32>,
    metric: impl Fn(&[f32], &[f32]) -> f32,
) -> Vec<(usize, f32)> {
    let n = x.len();
    if n < 4 {
        return vec![];
    }
    let mut order: Vec<usize> = (0..n).collect();
    Rng::new(seed).shuffle(&mut order);
    let nt = (n / 4).max(1);
    let (pool, test) = order.split_at(n - nt);
    let qx: Vec<Vec<f32>> = test.iter().map(|&i| x[i].clone()).collect();
    let qy: Vec<f32> = test.iter().map(|&i| y[i]).collect();
    sizes
        .iter()
        .filter(|&&s| s >= 2 && s <= pool.len())
        .map(|&s| {
            let (tx, ty): (Vec<Vec<f32>>, Vec<f32>) = pool[..s].iter().map(|&i| (x[i].clone(), y[i])).unzip();
            (s, metric(&qy, &predict(&tx, &ty, &qx)))
        })
        .collect()
}

/// Weight-based importance: |w_j| * std_j (or |w_j|), signed, sorted by magnitude.
pub fn weight_importance(names: &[&str], w: &[f32], scale: Option<&Standardizer>) -> Vec<(String, f32)> {
    let mut v: Vec<(String, f32)> = names.iter().zip(w).enumerate().map(|(j, (n, w))| (n.to_string(), w * scale.and_then(|s| s.std.get(j)).copied().unwrap_or(1.0))).collect();
    v.sort_by(|a, b| b.1.abs().total_cmp(&a.1.abs()).then(a.0.cmp(&b.0)));
    v
}

/// Permutation importance: metric drop (sign-adjusted so bigger = more important) when a column is shuffled.
pub fn permutation_importance(
    x: &Xs,
    y: &[f32],
    names: &[&str],
    seed: u64,
    predict: impl Fn(&Xs) -> Vec<f32>,
    metric: impl Fn(&[f32], &[f32]) -> f32,
    higher_better: bool,
) -> Vec<(String, f32)> {
    let base = metric(y, &predict(x));
    let mut rng = Rng::new(seed);
    let mut out: Vec<(String, f32)> = names
        .iter()
        .enumerate()
        .map(|(j, n)| {
            let mut col: Vec<f32> = x.iter().map(|r| r[j]).collect();
            rng.shuffle(&mut col);
            let xp: Vec<Vec<f32>> = x.iter().zip(&col).map(|(r, &c)| { let mut r = r.clone(); r[j] = c; r }).collect();
            let s = metric(y, &predict(&xp));
            (n.to_string(), if higher_better { base - s } else { s - base })
        })
        .collect();
    out.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    out
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Status {
    #[default]
    ColdStart,
    Learning,
    Ready,
}

/// Cut points: n < 10 -> ColdStart (prior only); 10..30 -> Learning; n >= 30 -> Ready only when
/// the cross-validated metric beats the trivial baseline, otherwise stays Learning.
pub const COLD_BELOW: usize = 10;
pub const READY_AT: usize = 30;
pub fn status_for(n: usize, cv: Option<f32>, baseline: Option<f32>, higher_better: bool) -> Status {
    if n < COLD_BELOW {
        Status::ColdStart
    } else if n < READY_AT {
        Status::Learning
    } else {
        match (cv, baseline) {
            (Some(c), Some(b)) if (higher_better && c > b) || (!higher_better && c < b) => Status::Ready,
            _ => Status::Learning,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ModelCard {
    pub name: String,
    pub n_samples: usize,
    pub metric_name: String,
    pub metric_cv: Option<f32>,
    pub baseline: Option<f32>,
    pub status: Status,
    pub updated_at: String,
    pub top_features: Vec<(String, f32)>,
    pub learning_curve: Vec<(usize, f32)>,
    pub notes: String,
}
impl ModelCard {
    pub fn new(name: &str, n: usize, metric: &str, cv: Option<f32>, baseline: Option<f32>, higher_better: bool, updated_at: &str, notes: &str) -> Self {
        ModelCard {
            name: name.into(),
            n_samples: n,
            metric_name: metric.into(),
            metric_cv: cv,
            baseline,
            status: status_for(n, cv, baseline, higher_better),
            updated_at: updated_at.into(),
            notes: notes.into(),
            ..Default::default()
        }
    }
}

/// UTC "YYYY-MM-DDTHH:MM:SSZ" from the system clock (no chrono dependency).
pub fn now_iso() -> String {
    let s = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs()) as i64;
    let (days, rem) = (s.div_euclid(86400), s.rem_euclid(86400));
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + (m <= 2) as i64;
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

/// Majority-rate Brier baseline for binary labels.
pub fn brier_baseline(y: &[f32]) -> f32 {
    let r = y.iter().sum::<f32>() / y.len().max(1) as f32;
    r * (1.0 - r)
}

#[cfg(test)]
mod tests {
    use super::*;

    pub fn planted(n: usize, w: &[f32], b: f32, seed: u64) -> (Vec<Vec<f32>>, Vec<f32>) {
        let mut r = Rng::new(seed);
        let x: Vec<Vec<f32>> = (0..n).map(|_| w.iter().map(|_| r.f32() * 2.0 - 1.0).collect()).collect();
        let y = x.iter().map(|row| if r.f32() < sigmoid(b + row.iter().zip(w).map(|(a, b)| a * b).sum::<f32>()) { 1.0 } else { 0.0 }).collect();
        (x, y)
    }
    fn lr_predict(x: &Xs, y: &[f32], q: &Xs) -> Vec<f32> {
        let m = LogReg::fit(x, y, None, 0.0);
        q.iter().map(|r| m.predict_proba(r)).collect()
    }

    #[test]
    fn logreg_recovers_weights_and_shrinkage_behaves() {
        let wt = [2.0, -1.5, 0.5];
        let (x, y) = planted(3000, &wt, 0.3, 1);
        let m = LogReg::fit(&x, &y, None, 0.0);
        for (a, b) in m.w.iter().zip(&wt) {
            assert!((a - b).abs() < 0.4, "{a} vs {b}");
        }
        let prior = LogReg::new(vec![-5.0, 5.0, 0.0], 0.0);
        let big = LogReg::fit(&x, &y, Some(&prior), 10.0);
        assert!((big.w[0] - 2.0).abs() < 0.5);
        let tiny = LogReg::fit(&x[..3], &y[..3], Some(&prior), 10.0);
        assert!((tiny.w[0] + 5.0).abs() < (tiny.w[0] - 2.0).abs(), "prior dominates: {:?}", tiny.w);
        assert_eq!(m, LogReg::fit(&x, &y, None, 0.0)); // deterministic
    }

    #[test]
    fn ridge_recovers_coefficients() {
        let mut r = Rng::new(3);
        let x: Vec<Vec<f32>> = (0..200).map(|_| vec![r.f32() * 10.0, r.f32() * 4.0]).collect();
        let y: Vec<f32> = x.iter().map(|v| 3.0 * v[0] - 2.0 * v[1] + 7.0).collect();
        let m = Ridge::fit(&x, &y, 1e-6);
        assert!((m.w[0] - 3.0).abs() < 1e-2 && (m.w[1] + 2.0).abs() < 1e-2 && (m.b - 7.0).abs() < 1e-1);
    }

    #[test]
    fn nb_updates_online() {
        let mut nb = NaiveBayes::new(&["a", "b"]);
        let t = |s: &str| s.split(' ').map(String::from).collect::<Vec<_>>();
        nb.update(&t("apple pie sweet"), "a", 1.0);
        nb.update(&t("steel bolt metal"), "b", 1.0);
        assert!(nb.predict(&t("sweet apple"))[0].1 > 0.5);
        for _ in 0..5 { nb.update(&t("sweet apple"), "b", 1.0) }
        assert!(nb.predict(&t("sweet apple"))[1].1 > 0.5);
    }

    #[test]
    fn metrics_and_cv_monotone() {
        assert_eq!(auc(&[0.0, 0.0, 1.0, 1.0], &[0.1, 0.2, 0.8, 0.9]), 1.0);
        assert_eq!(auc(&[0.0, 1.0], &[0.5, 0.5]), 0.5);
        assert!((r2(&[1.0, 2.0, 3.0], &[1.0, 2.0, 3.0]) - 1.0).abs() < 1e-6);
        let (x, y) = planted(400, &[3.0, -3.0], 0.0, 9);
        let cv = cross_val_score(&x, &y, 1, lr_predict, auc).unwrap();
        assert!(cv > 0.8, "{cv}");
        let lc = learning_curve(&x, &y, &[5, 20, 80, 300], 1, lr_predict, auc);
        assert_eq!(lc.len(), 4);
        assert!(lc[3].1 >= lc[0].1 - 0.02, "{lc:?}");
        assert!(cross_val_score(&x[..10], &y[..10], 1, lr_predict, auc).is_some()); // LOO path
        assert!(cross_val_score(&x[..3], &y[..3], 1, lr_predict, auc).is_none());
        let imp = permutation_importance(&x, &y, &["a", "b"], 1, |q| q.iter().map(|r| LogReg::new(vec![3.0, -3.0], 0.0).predict_proba(r)).collect(), auc, true);
        assert!(imp[0].1 > 0.1);
        assert_eq!(status_for(5, None, None, true), Status::ColdStart);
        assert_eq!(status_for(40, Some(0.8), Some(0.5), true), Status::Ready);
        assert_eq!(status_for(40, Some(0.2), Some(0.25), false), Status::Ready);
        assert_eq!(status_for(40, Some(0.5), Some(0.5), true), Status::Learning);
        assert!(now_iso().starts_with("20"));
    }
}
