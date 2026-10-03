//! Learns bullet preferences from explicit keep/remove feedback; prior = the hand-tuned select
//! score (0.5*bm25_norm + 0.5*cosine, + 0.1*proficiency boost) squashed through a sigmoid.
use super::core::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Action {
    Keep,
    Remove,
    AddBack,
    Edit,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Feedback {
    pub job_id: String,
    pub bullet_id: String,
    pub action: Action,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BulletFeatures {
    /// bm25 / max bm25 within the JD's candidate set (0..1).
    pub bm25_norm: f32,
    pub max_cos: f32,
    /// Summed JD weight of terms hit (normalised by caller, 0..1).
    pub kw_hit: f32,
    pub skill_prof: f32,
    pub recency: f32,
    pub has_metric: bool,
    /// 0 weak/none, 1 medium, 2 strong.
    pub verb_class: u8,
    pub length: usize,
    /// Index within its role / role bullet count (0..1).
    pub position: f32,
    /// 0 full-time, 1 internship, 2 other.
    pub role_kind: u8,
}

pub const FEATURES: [&str; 11] = ["bm25_norm", "max_cos", "kw_hit", "skill_prof", "recency", "has_metric", "verb_class", "length", "position", "is_fulltime", "is_intern"];

impl BulletFeatures {
    pub fn to_vec(&self) -> Vec<f32> {
        vec![
            self.bm25_norm,
            self.max_cos,
            self.kw_hit,
            self.skill_prof,
            self.recency,
            self.has_metric as u8 as f32,
            self.verb_class.min(2) as f32 / 2.0,
            (self.length as f32 / 200.0).min(2.0),
            self.position,
            (self.role_kind == 0) as u8 as f32,
            (self.role_kind == 1) as u8 as f32,
        ]
    }
}

const K_PRIOR: f32 = 20.0;
const CAP: usize = 3000;
/// Max share of the final score the ML model may take.
pub const MAX_ML_SHARE: f32 = 0.6;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BulletRanker {
    pub model: LogReg,
    pub samples: Vec<(Vec<f32>, f32)>,
    pub updated_at: String,
}
impl Default for BulletRanker {
    fn default() -> Self {
        BulletRanker { model: Self::prior(), samples: vec![], updated_at: String::new() }
    }
}

impl BulletRanker {
    pub fn prior() -> LogReg {
        // logit = 6*(0.5*bm25 + 0.5*cos + 0.1*prof) - 3: monotone in the hand-tuned score.
        LogReg::new(vec![3.0, 3.0, 0.0, 0.6, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0], -3.0)
    }
    /// Record one feedback event (Edit is ignored: it says nothing about selection).
    pub fn observe(&mut self, f: &Feedback, x: &BulletFeatures) {
        let y = match f.action {
            Action::Keep | Action::AddBack => 1.0,
            Action::Remove => 0.0,
            Action::Edit => return,
        };
        if self.samples.len() >= CAP {
            self.samples.remove(0);
        }
        self.samples.push((x.to_vec(), y));
    }
    pub fn refit(&mut self) {
        let (x, y): (Vec<Vec<f32>>, Vec<f32>) = self.samples.iter().cloned().unzip();
        self.model = LogReg::fit_with(&x, &y, Some(&Self::prior()), K_PRIOR, 1.0, true);
        self.updated_at = now_iso();
    }
    /// Observe a batch then refit once.
    pub fn train(&mut self, events: &[(Feedback, BulletFeatures)]) {
        events.iter().for_each(|(f, x)| self.observe(f, x));
        self.refit();
    }
    pub fn n(&self) -> usize {
        self.samples.len()
    }
    /// Scores in 0..1 (higher = keep).
    pub fn rank(&self, xs: &[BulletFeatures]) -> Vec<f32> {
        xs.iter().map(|x| self.model.predict_proba(&x.to_vec())).collect()
    }
    /// 0 until 10 events, then n/(n+40).
    pub fn confidence(&self) -> f32 {
        let n = self.n() as f32;
        if self.n() < COLD_BELOW { 0.0 } else { n / (n + 40.0) }
    }
    pub fn card(&self) -> ModelCard {
        let n = self.n();
        let (x, y): (Vec<Vec<f32>>, Vec<f32>) = self.samples.iter().cloned().unzip();
        let pred = |tx: &Xs, ty: &[f32], q: &Xs| {
            let m = LogReg::fit_with(tx, ty, Some(&Self::prior()), K_PRIOR, 1.0, true);
            q.iter().map(|r| m.predict_proba(r)).collect()
        };
        let cv = cross_val_score(&x, &y, 11, pred, auc);
        let mut c = ModelCard::new("bullet_ranker", n, "AUC (CV)", cv, Some(0.5), true, &self.updated_at, "Learns keep/remove taste from explicit feedback; prior = hand-tuned select score. Blend share capped.");
        c.top_features = weight_importance(&FEATURES, &self.model.w, None).into_iter().take(5).collect();
        c.learning_curve = learning_curve(&x, &y, &[10, 20, 40, 80, 160], 11, pred, auc);
        c
    }
}

/// Min-max normalise scores to 0..1 (constant input -> 0.5) so select's score can be blended.
pub fn normalise(v: &[f32]) -> Vec<f32> {
    let (lo, hi) = v.iter().fold((f32::MAX, f32::MIN), |(l, h), &x| (l.min(x), h.max(x)));
    v.iter().map(|&x| if hi > lo { (x - lo) / (hi - lo) } else { 0.5 }).collect()
}

/// Blend a select score with the ML score (both 0..1): ml weight = confidence * MAX_ML_SHARE.
pub fn blend(select_score: f32, ml_score: f32, confidence: f32) -> f32 {
    let a = confidence.clamp(0.0, 1.0) * MAX_ML_SHARE;
    (1.0 - a) * select_score + a * ml_score
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bf(bm: f32, metric: bool) -> BulletFeatures {
        BulletFeatures { bm25_norm: bm, max_cos: bm, has_metric: metric, ..Default::default() }
    }

    #[test]
    fn prior_orders_by_relevance_then_feedback_moves_weights() {
        let mut r = BulletRanker::default();
        let s = r.rank(&[bf(0.9, false), bf(0.1, false)]);
        assert!(s[0] > s[1]);
        assert_eq!(r.confidence(), 0.0);
        let mut g = Rng::new(2);
        let ev: Vec<_> = (0..80)
            .map(|i| {
                let m = i % 2 == 0;
                let x = bf(g.f32(), m);
                (Feedback { job_id: format!("j{}", i / 8), bullet_id: format!("b{i}"), action: if m { Action::Keep } else { Action::Remove } }, x)
            })
            .collect();
        r.train(&ev);
        assert!(r.model.w[5] > 1.0, "metric weight {:?}", r.model.w);
        let s = r.rank(&[bf(0.5, true), bf(0.5, false)]);
        assert!(s[0] > s[1] + 0.2);
        assert!(r.confidence() > 0.5);
        assert!((blend(0.2, 0.8, 0.0) - 0.2).abs() < 1e-6 && blend(0.2, 0.8, 1.0) > 0.5);
        assert_eq!(normalise(&[1.0, 3.0, 2.0]), vec![0.0, 1.0, 0.5]);
        let c = r.card();
        assert_eq!(c.n_samples, 80);
        assert!(c.metric_cv.unwrap() > 0.5);
    }
}
