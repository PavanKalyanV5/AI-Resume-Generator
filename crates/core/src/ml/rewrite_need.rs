//! Predicts whether an LLM rewrite of a bullet is worth its tokens (will be accepted).
use super::core::*;
use serde::{Deserialize, Serialize};

/// Rewrite when P(accepted) >= this once the model has enough data.
pub const THRESHOLD: f32 = 0.35;
const K_PRIOR: f32 = 15.0;
const CAP: usize = 2000;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RewriteFeatures {
    /// JD terms absent from the bullet but backed by profile skills or semantically close (weighted count).
    pub potential_gain: f32,
    pub length: usize,
    pub has_metric: bool,
    /// 0 weak/none .. 1 strong.
    pub verb_strength: f32,
    pub keyword_hits: f32,
}
impl RewriteFeatures {
    pub fn to_vec(&self) -> Vec<f32> {
        vec![self.potential_gain.clamp(0.0, 5.0), (self.length as f32 / 200.0).min(2.0), self.has_metric as u8 as f32, self.verb_strength, self.keyword_hits.clamp(0.0, 5.0)]
    }
}
pub const FEATURES: [&str; 5] = ["potential_gain", "length", "has_metric", "verb_strength", "keyword_hits"];

/// accepted = changed, passed grounding, and not reverted by the user.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RewriteOutcome {
    pub accepted: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RewriteNeed {
    pub model: LogReg,
    pub samples: Vec<(Vec<f32>, f32)>,
    pub updated_at: String,
}
impl Default for RewriteNeed {
    fn default() -> Self {
        RewriteNeed { model: Self::prior(), samples: vec![], updated_at: String::new() }
    }
}

impl RewriteNeed {
    /// More gain -> more likely; strong verbs, metrics, many existing hits and very long bullets -> less.
    pub fn prior() -> LogReg {
        LogReg::new(vec![0.7, -0.3, -0.3, -0.8, -0.2], -0.5)
    }
    pub fn n(&self) -> usize {
        self.samples.len()
    }
    pub fn observe(&mut self, x: &RewriteFeatures, o: RewriteOutcome) {
        if self.samples.len() >= CAP {
            self.samples.remove(0);
        }
        self.samples.push((x.to_vec(), o.accepted as u8 as f32));
    }
    pub fn refit(&mut self) {
        let (x, y): (Vec<Vec<f32>>, Vec<f32>) = self.samples.iter().cloned().unzip();
        self.model = LogReg::fit(&x, &y, Some(&Self::prior()), K_PRIOR);
        self.updated_at = now_iso();
    }
    pub fn p_accept(&self, f: &RewriteFeatures) -> f32 {
        self.model.predict_proba(&f.to_vec())
    }
    /// Zero potential gain is never rewritten (saves tokens at any maturity). In ColdStart
    /// (n < 10) every bullet with gain > 0 is rewritten; afterwards P >= THRESHOLD decides.
    pub fn should_rewrite(&self, f: &RewriteFeatures) -> (bool, f32) {
        let p = self.p_accept(f);
        if f.potential_gain <= 0.0 {
            return (false, p);
        }
        (self.n() < COLD_BELOW || p >= THRESHOLD, p)
    }
    pub fn card(&self) -> ModelCard {
        let n = self.n();
        let (x, y): (Vec<Vec<f32>>, Vec<f32>) = self.samples.iter().cloned().unzip();
        let pred = |tx: &Xs, ty: &[f32], q: &Xs| {
            let m = LogReg::fit(tx, ty, Some(&Self::prior()), K_PRIOR);
            q.iter().map(|r| m.predict_proba(r)).collect()
        };
        let cv = cross_val_score(&x, &y, 13, pred, brier);
        let mut c = ModelCard::new("rewrite_need", n, "Brier (CV, lower better)", cv, (n > 0).then(|| brier_baseline(&y)), false, &self.updated_at, "Skips rewrites unlikely to be accepted; zero-gain bullets are always skipped.");
        c.top_features = weight_importance(&FEATURES, &self.model.w, None);
        c.learning_curve = learning_curve(&x, &y, &[10, 20, 40, 80], 13, pred, brier);
        c
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(gain: f32) -> RewriteFeatures {
        RewriteFeatures { potential_gain: gain, length: 100, ..Default::default() }
    }

    #[test]
    fn cold_start_skips_zero_gain_and_learns_threshold() {
        let mut m = RewriteNeed::default();
        assert!(!m.should_rewrite(&f(0.0)).0);
        assert!(m.should_rewrite(&f(1.0)).0);
        let mut g = Rng::new(4);
        for _ in 0..80 {
            let gain = (g.f32() * 5.0).floor();
            m.observe(&f(gain), RewriteOutcome { accepted: gain >= 3.0 });
        }
        m.refit();
        assert!(!m.should_rewrite(&f(0.0)).0);
        assert!(!m.should_rewrite(&f(1.0)).0, "p={}", m.p_accept(&f(1.0)));
        assert!(m.should_rewrite(&f(4.0)).0);
        assert!(m.card().metric_cv.is_some());
    }
}
