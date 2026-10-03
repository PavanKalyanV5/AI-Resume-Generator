//! P(response) from application features, plus funnel stats. Needs real history: no prediction
//! below MIN_LABELLED labelled applications.
use super::core::*;
use serde::{Deserialize, Serialize};

pub const MIN_LABELLED: usize = 25;
const BOOT: usize = 60;
const K_PRIOR: f32 = 5.0;
const CAP: usize = 1000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Stage {
    Applied,
    Response,
    Interview,
    Offer,
    Rejected,
    Ghosted,
}
impl Stage {
    /// Some(true) = company responded (any reply incl. rejection), Some(false) = ghosted, None = still pending.
    pub fn responded(self) -> Option<bool> {
        match self {
            Stage::Applied => None,
            Stage::Ghosted => Some(false),
            _ => Some(true),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Application {
    pub job_id: String,
    pub applied_at: String,
    pub stage: Stage,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ApplicationFeatures {
    pub coverage: f32,
    pub semantic_share: f32,
    pub required_missing: f32,
    /// JD years required minus candidate years (negative = surplus).
    pub years_gap: f32,
    pub pages: f32,
    pub family_match_prob: f32,
    pub seniority_match: f32,
}
pub const FEATURES: [&str; 7] = ["coverage", "semantic_share", "required_missing", "years_gap", "pages", "family_match_prob", "seniority_match"];
impl ApplicationFeatures {
    pub fn to_vec(&self) -> Vec<f32> {
        vec![self.coverage, self.semantic_share, self.required_missing, self.years_gap, self.pages, self.family_match_prob, self.seniority_match]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Prediction {
    pub p: f32,
    /// ~90% bootstrap interval.
    pub lo: f32,
    pub hi: f32,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct OutcomeModel {
    pub samples: Vec<(Vec<f32>, f32)>,
    pub std: Standardizer,
    pub model: Option<LogReg>,
    pub boot: Vec<LogReg>,
    pub updated_at: String,
}

fn fit_std(x: &Xs, y: &[f32]) -> LogReg {
    LogReg::fit(x, y, Some(&LogReg::new(vec![0.0; x[0].len()], 0.0)), K_PRIOR)
}

impl OutcomeModel {
    pub fn n_labelled(&self) -> usize {
        self.samples.len()
    }
    /// Record a labelled application (pending `Applied` ones are ignored). Call `refit` after a batch.
    pub fn observe(&mut self, f: &ApplicationFeatures, stage: Stage) {
        if let Some(r) = stage.responded() {
            if self.samples.len() >= CAP {
                self.samples.remove(0);
            }
            self.samples.push((f.to_vec(), r as u8 as f32));
        }
    }
    pub fn refit(&mut self) {
        if self.samples.len() < MIN_LABELLED {
            self.model = None;
            self.boot.clear();
            return;
        }
        let (x, y): (Vec<Vec<f32>>, Vec<f32>) = self.samples.iter().cloned().unzip();
        self.std = Standardizer::fit(&x);
        let xs = self.std.transform_all(&x);
        self.model = Some(fit_std(&xs, &y));
        let (n, mut rng) = (xs.len(), Rng::new(17));
        self.boot = (0..BOOT)
            .map(|_| {
                let idx: Vec<usize> = (0..n).map(|_| (rng.next_u64() % n as u64) as usize).collect();
                let (bx, by): (Vec<Vec<f32>>, Vec<f32>) = idx.iter().map(|&i| (xs[i].clone(), y[i])).unzip();
                fit_std(&bx, &by)
            })
            .collect();
        self.updated_at = now_iso();
    }
    /// None until MIN_LABELLED labelled applications have been fitted.
    pub fn predict(&self, f: &ApplicationFeatures) -> Option<Prediction> {
        let m = self.model.as_ref()?;
        let x = self.std.transform(&f.to_vec());
        let p = m.predict_proba(&x);
        let mut b: Vec<f32> = self.boot.iter().map(|m| m.predict_proba(&x)).collect();
        b.sort_by(|a, c| a.total_cmp(c));
        let (lo, hi) = if b.is_empty() { (p, p) } else { (b[b.len() * 5 / 100], b[(b.len() * 95 / 100).min(b.len() - 1)]) };
        Some(Prediction { p, lo: lo.min(p), hi: hi.max(p) })
    }
    pub fn card(&self) -> ModelCard {
        let n = self.n_labelled();
        let (x, y): (Vec<Vec<f32>>, Vec<f32>) = self.samples.iter().cloned().unzip();
        let pred = |tx: &Xs, ty: &[f32], q: &Xs| {
            let s = Standardizer::fit(tx);
            let m = fit_std(&s.transform_all(tx), ty);
            q.iter().map(|r| m.predict_proba(&s.transform(r))).collect()
        };
        let cv = if n >= MIN_LABELLED { cross_val_score(&x, &y, 19, pred, auc) } else { None };
        let mut c = ModelCard::new("outcome", n, "AUC (CV)", cv, Some(0.5), true, &self.updated_at, "No prediction below 25 labelled applications; outcomes are noisy (recruiters, timing), so expect wide intervals.");
        if let Some(m) = &self.model {
            c.top_features = weight_importance(&FEATURES, &m.w, None).into_iter().take(5).collect();
        }
        c.learning_curve = learning_curve(&x, &y, &[10, 20, 40, 80], 19, pred, auc);
        c
    }
}

/// Wilson score interval for k successes in n trials (z = 1.96).
pub fn wilson(k: usize, n: usize) -> (f32, f32) {
    if n == 0 {
        return (0.0, 1.0);
    }
    let (n, p, z) = (n as f32, k as f32 / n as f32, 1.96f32);
    let d = 1.0 + z * z / n;
    let c = (p + z * z / (2.0 * n)) / d;
    let h = z * (p * (1.0 - p) / n + z * z / (4.0 * n * n)).sqrt() / d;
    ((c - h).max(0.0), (c + h).min(1.0))
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Funnel {
    pub total: usize,
    /// Still `Applied` (no outcome yet); excluded from rates.
    pub pending: usize,
    pub responded: usize,
    pub interviews: usize,
    pub offers: usize,
    pub rejected: usize,
    pub ghosted: usize,
    pub response_rate: f32,
    pub response_ci: (f32, f32),
    pub interview_rate: f32,
    pub offer_rate: f32,
}

/// Cumulative funnel: an Offer also counts as an interview and a response.
pub fn funnel(apps: &[Application]) -> Funnel {
    let c = |f: fn(Stage) -> bool| apps.iter().filter(|a| f(a.stage)).count();
    let resolved = apps.iter().filter(|a| a.stage != Stage::Applied).count();
    let (responded, interviews, offers) = (c(|s| s.responded() == Some(true)), c(|s| matches!(s, Stage::Interview | Stage::Offer)), c(|s| s == Stage::Offer));
    let rate = |k: usize| if resolved == 0 { 0.0 } else { k as f32 / resolved as f32 };
    Funnel { total: apps.len(), pending: apps.len() - resolved, responded, interviews, offers, rejected: c(|s| s == Stage::Rejected), ghosted: c(|s| s == Stage::Ghosted), response_rate: rate(responded), response_ci: wilson(responded, resolved), interview_rate: rate(interviews), offer_rate: rate(offers) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feat(c: f32) -> ApplicationFeatures {
        ApplicationFeatures { coverage: c, pages: 1.0, ..Default::default() }
    }

    #[test]
    fn none_until_25_then_sane_interval() {
        let mut m = OutcomeModel::default();
        let mut r = Rng::new(8);
        for i in 0..60 {
            let c = r.f32();
            let resp = r.f32() < sigmoid(6.0 * (c - 0.5));
            m.observe(&feat(c), if resp { Stage::Interview } else { Stage::Ghosted });
            if i == 23 {
                m.refit();
                assert!(m.predict(&feat(0.5)).is_none());
            }
        }
        m.observe(&feat(0.5), Stage::Applied);
        assert_eq!(m.n_labelled(), 60);
        m.refit();
        let (hi, lo) = (m.predict(&feat(0.95)).unwrap(), m.predict(&feat(0.05)).unwrap());
        assert!(hi.p > lo.p);
        assert!(hi.lo <= hi.p && hi.p <= hi.hi && lo.lo >= 0.0 && hi.hi <= 1.0);
        assert!(hi.hi - hi.lo > 0.0);
        assert!(m.card().metric_cv.unwrap() > 0.5);
    }

    #[test]
    fn funnel_counts() {
        let a = |s| Application { job_id: "j".into(), applied_at: "2026-01-01".into(), stage: s };
        let f = funnel(&[a(Stage::Applied), a(Stage::Ghosted), a(Stage::Offer), a(Stage::Rejected), a(Stage::Interview)]);
        assert_eq!((f.total, f.pending, f.responded, f.interviews, f.offers, f.ghosted), (5, 1, 3, 2, 1, 1));
        assert!((f.response_rate - 0.75).abs() < 1e-6 && f.response_ci.0 < 0.75 && f.response_ci.1 > 0.75);
    }
}
