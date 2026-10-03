//! Learns how many text lines a resume renders to, and how many lines fit a page per layout, so
//! page-fit can often skip trial renders.
//!
//! Server usage (fit.rs is untouched): build `ContentStats::from_resume(&sel.apply(r))` for the
//! candidate selections at each `Layout` (default, then `Layout::levels()`), call
//! `model.suggest_start(&candidates, target_pages)`. If `confidence >= ~0.6` render only that
//! plan (and verify with one real render); otherwise pass `index` as `start_level` to
//! `fit_to_pages_with`. After every real render call `observe_pages` with `pdf_line_counts(pdf)`
//! (or `observe` with total pages/lines) so the model learns.
use super::core::*;
use crate::latex::Layout;
use crate::schema::Resume;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ContentStats {
    pub chars: usize,
    pub bullets: usize,
    pub sections: usize,
    pub skill_rows: usize,
    pub certs: usize,
    pub projects: usize,
    pub roles: usize,
    pub avg_bullet_len: f32,
}
impl ContentStats {
    pub fn from_resume(r: &Resume) -> Self {
        let bl: Vec<&String> = r.experience.iter().flat_map(|e| &e.bullets).chain(r.projects.iter().flat_map(|p| &p.bullets)).collect();
        let chars = bl.iter().map(|b| b.chars().count()).sum::<usize>() + r.summary.iter().map(|s| s.chars().count()).sum::<usize>();
        let sections = [!r.summary.is_empty(), !r.experience.is_empty(), !r.education.is_empty(), !r.skills.is_empty(), !r.projects.is_empty(), !r.certifications.is_empty()].iter().filter(|&&b| b).count();
        ContentStats {
            chars,
            bullets: bl.len(),
            sections,
            skill_rows: r.skills.len(),
            certs: r.certifications.len(),
            projects: r.projects.len(),
            roles: r.experience.len(),
            avg_bullet_len: if bl.is_empty() { 0.0 } else { bl.iter().map(|b| b.chars().count()).sum::<usize>() as f32 / bl.len() as f32 },
        }
    }
}

/// Serialisable copy of `latex::Layout`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct LayoutKey {
    pub font_pt: u8,
    pub margin_in: f32,
    pub item_sep_pt: f32,
    pub section_gap_pt: f32,
}
impl From<&Layout> for LayoutKey {
    fn from(l: &Layout) -> Self {
        LayoutKey { font_pt: l.font_pt, margin_in: l.margin_in, item_sep_pt: l.item_sep_pt, section_gap_pt: l.section_gap_pt }
    }
}
impl LayoutKey {
    pub fn key(&self) -> String {
        format!("{}/{:.2}/{:.1}/{:.1}", self.font_pt, self.margin_in, self.item_sep_pt, self.section_gap_pt)
    }
}

/// Cold-start estimate of rendered text lines (wrapping + headings), from page geometry.
pub fn analytic_lines(s: &ContentStats, l: &LayoutKey) -> f32 {
    let cpl = ((8.5 - 2.0 * l.margin_in) * 72.0 / (l.font_pt as f32 * 0.5)).max(20.0) * 0.95;
    let per_bullet = (s.avg_bullet_len / cpl).ceil().max(1.0);
    s.bullets as f32 * per_bullet + 2.0 * s.roles as f32 + s.sections as f32 + s.skill_rows as f32 + s.certs as f32 + s.projects as f32 + 4.0
}
/// Cold-start lines per page: text height / line pitch, 10% lost to spacing.
pub fn analytic_capacity(l: &LayoutKey) -> f32 {
    (11.0 - 2.0 * l.margin_in) * 72.0 / (l.font_pt as f32 * 1.2) * 0.9
}

const K_PRIOR: f32 = 8.0;
const K_CAP: f32 = 2.0;
const OBS_CAP: usize = 500;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FitModel {
    /// Lines ~ ridge([analytic_lines, bullets, chars/1000, sections]).
    pub ridge: Ridge,
    pub obs: Vec<(Vec<f32>, f32)>,
    /// layout key -> (mean lines per full page, samples)
    pub capacity: BTreeMap<String, (f32, f32)>,
    pub updated_at: String,
}
impl Default for FitModel {
    fn default() -> Self {
        FitModel { ridge: Self::prior(), obs: vec![], capacity: BTreeMap::new(), updated_at: String::new() }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Suggestion {
    pub index: usize,
    pub pages: f32,
    pub confidence: f32,
}

impl FitModel {
    fn prior() -> Ridge {
        Ridge { w: vec![1.0, 0.0, 0.0, 0.0], b: 0.0, l2: 1.0 }
    }
    fn feats(s: &ContentStats, l: &LayoutKey) -> Vec<f32> {
        vec![analytic_lines(s, l), s.bullets as f32, s.chars as f32 / 1000.0, s.sections as f32]
    }
    fn refit(&mut self) {
        let (x, y): (Vec<Vec<f32>>, Vec<f32>) = self.obs.iter().cloned().unzip();
        self.ridge = Ridge::fit_prior(&x, &y, 1.0, Some(&Self::prior()), K_PRIOR);
        self.updated_at = now_iso();
    }
    fn add_cap(&mut self, l: &LayoutKey, sample: f32, w: f32) {
        let e = self.capacity.entry(l.key()).or_insert((0.0, 0.0));
        e.0 = (e.0 * e.1 + sample * w) / (e.1 + w);
        e.1 += w;
    }
    /// Learn from a render: total pages and total text lines. Capacity is only inferred for
    /// multi-page documents (last page assumed half full); prefer `observe_pages`.
    pub fn observe(&mut self, s: &ContentStats, layout: &Layout, measured_pages: usize, measured_lines: usize) {
        let l = LayoutKey::from(layout);
        if self.obs.len() >= OBS_CAP {
            self.obs.remove(0);
        }
        self.obs.push((Self::feats(s, &l), measured_lines as f32));
        if measured_pages >= 2 {
            self.add_cap(&l, measured_lines as f32 / (measured_pages as f32 - 0.5), 0.5);
        }
        self.refit();
    }
    /// Like `observe` with exact per-page line counts (see `pdf_line_counts`); full pages give capacity.
    pub fn observe_pages(&mut self, s: &ContentStats, layout: &Layout, lines_per_page: &[usize]) {
        let l = LayoutKey::from(layout);
        for &n in lines_per_page.iter().take(lines_per_page.len().saturating_sub(1)) {
            self.add_cap(&l, n as f32, 1.0);
        }
        let total: usize = lines_per_page.iter().sum();
        let before = self.capacity.get(&l.key()).copied();
        self.observe(s, layout, 1, total); // pages=1: no half-page capacity guess
        if let Some(b) = before {
            self.capacity.insert(l.key(), b);
        }
    }
    pub fn n_obs(&self) -> usize {
        self.obs.len()
    }
    pub fn capacity_of(&self, l: &LayoutKey) -> f32 {
        let a = analytic_capacity(l);
        self.capacity.get(&l.key()).map_or(a, |&(m, n)| (n * m + K_CAP * a) / (n + K_CAP))
    }
    pub fn predict_lines(&self, s: &ContentStats, layout: &Layout) -> f32 {
        self.ridge.predict(&Self::feats(s, &LayoutKey::from(layout))).max(0.0)
    }
    /// (fractional pages, confidence 0..1). Confidence grows with observations and with
    /// capacity samples for this exact layout; cold start is ~0.15.
    pub fn predict_pages(&self, s: &ContentStats, layout: &Layout) -> (f32, f32) {
        let l = LayoutKey::from(layout);
        let n = self.obs.len() as f32;
        let cn = self.capacity.get(&l.key()).map_or(0.0, |c| c.1);
        let conf = (0.15 + 0.8 * n / (n + 10.0)) * (0.6 + 0.4 * cn / (cn + 2.0));
        (self.predict_lines(s, layout) / self.capacity_of(&l), conf)
    }
    /// First candidate (candidates ordered least-trimmed first, e.g. selection x layout levels)
    /// predicted to fit `target_pages` with a 3% safety margin; falls back to the last one.
    pub fn suggest_start(&self, candidates: &[(ContentStats, Layout)], target_pages: u8) -> Option<Suggestion> {
        let t = target_pages.max(1) as f32 * 0.97;
        let mut last = None;
        for (i, (s, l)) in candidates.iter().enumerate() {
            let (pages, confidence) = self.predict_pages(s, l);
            last = Some(Suggestion { index: i, pages, confidence });
            if pages <= t {
                return last;
            }
        }
        last
    }
    pub fn card(&self) -> ModelCard {
        let n = self.obs.len();
        let (x, y): (Vec<Vec<f32>>, Vec<f32>) = self.obs.iter().cloned().unzip();
        let pred = |tx: &Xs, ty: &[f32], q: &Xs| {
            let m = Ridge::fit_prior(tx, ty, 1.0, Some(&Self::prior()), K_PRIOR);
            q.iter().map(|r| m.predict(r)).collect()
        };
        let cv = cross_val_score(&x, &y, 7, pred, mae);
        let base = (n >= 4).then(|| mae(&y, &x.iter().map(|r| r[0]).collect::<Vec<_>>()));
        let mut c = ModelCard::new("fit_model", n, "MAE lines (CV)", cv, base, false, &self.updated_at, "Predicts rendered line count; baseline = analytic estimate. Capacity per layout learned separately.");
        c.top_features = weight_importance(&["analytic_lines", "bullets", "chars_k", "sections"], &self.ridge.w, None);
        c.learning_curve = learning_curve(&x, &y, &[5, 10, 20, 40, 80], 7, pred, mae);
        c
    }
}

/// Non-empty text lines per page of a rendered PDF.
pub fn pdf_line_counts(path: &std::path::Path) -> anyhow::Result<Vec<usize>> {
    let pages = pdf_extract::extract_text_by_pages(path).map_err(|e| anyhow::anyhow!("pdf text: {e}"))?;
    Ok(pages.iter().map(|p| p.lines().filter(|l| !l.trim().is_empty()).count()).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stats(r: &mut Rng) -> ContentStats {
        ContentStats { chars: 3000, bullets: 8 + (r.f32() * 14.0) as usize, sections: 5, skill_rows: 3 + (r.f32() * 3.0) as usize, certs: (r.f32() * 4.0) as usize, projects: (r.f32() * 3.0) as usize, roles: 3, avg_bullet_len: 90.0 + r.f32() * 80.0 }
    }

    #[test]
    fn learns_planted_lines_and_picks_plan() {
        let mut m = FitModel::default();
        let mut r = Rng::new(5);
        let lay = Layout::default();
        let k = LayoutKey::from(&lay);
        let truth = |s: &ContentStats| 0.8 * analytic_lines(s, &k) + 5.0;
        let probe = stats(&mut r);
        let cold = (m.predict_lines(&probe, &lay) - truth(&probe)).abs();
        for _ in 0..40 {
            let s = stats(&mut r);
            let t = truth(&s) as usize;
            let pages: Vec<usize> = if t > 50 { vec![50, t - 50] } else { vec![t] };
            m.observe_pages(&s, &lay, &pages);
        }
        let warm = (m.predict_lines(&probe, &lay) - truth(&probe)).abs();
        assert!(warm < cold && warm < 3.0, "cold {cold} warm {warm}");
        assert!((m.capacity_of(&k) - 50.0).abs() < 3.0);
        assert!(m.predict_pages(&probe, &lay).1 > 0.5);

        let mut small = probe.clone();
        small.bullets = 6;
        let mut big = probe.clone();
        big.bullets = 60;
        let s = m.suggest_start(&[(big.clone(), lay), (small, lay)], 1).unwrap();
        assert_eq!(s.index, 1);
        assert!(s.pages <= 1.0);
        let json = serde_json::to_string(&m).unwrap();
        assert_eq!(m, serde_json::from_str(&json).unwrap());
        assert!(m.card().n_samples == 40);
    }
}
