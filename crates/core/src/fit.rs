//! Fit a selected resume into N pages by trimming least-valuable content first, using real
//! tectonic renders. Core experience is protected: secondary items, layout and secondary-role
//! bullets go before primary-role bullets, which never drop below a floor.
use crate::heuristics::Heuristics;
use crate::jd::Jd;
use crate::latex::{pdf_pages, render_pdf_with, Layout};
use crate::schema::Resume;
use crate::select::{project_doc, weight_of, year, Selection};
use crate::seniority::{latest, RoleKind};
use anyhow::Result;
use std::cell::Cell;
use std::path::Path;

#[derive(Clone, Debug)]
pub struct FitResult {
    pub resume: Resume,
    /// 0 = default layout, 1..=3 = `Layout::levels()`.
    pub layout_level: usize,
    pub pages: usize,
    /// Human-readable actions taken, in order (plus a warning when it could not fit).
    pub steps: Vec<String>,
    /// Labels of dropped items.
    pub dropped: Vec<String>,
}

/// `r` is the full (un-trimmed, possibly AI-rewritten) resume; `sel` says what to keep.
pub fn fit_to_pages(r: &Resume, sel: &Selection, jd: &Jd, target_pages: u8, dir: &Path) -> Result<FitResult> {
    fit_to_pages_with(r, sel, jd, target_pages, 0, &Heuristics::default(), dir)
}

fn clip(s: &str) -> String {
    let t: String = s.chars().take(48).collect();
    if t.len() < s.len() { format!("{t}...") } else { t }
}

/// Remove the lowest-weighted (later-first on ties) items from `v` until `floor` remain.
fn trim<T>(v: &mut Vec<usize>, floor: usize, key: impl Fn(usize) -> T, label: impl Fn(usize) -> String, dropped: &mut Vec<String>) -> bool
where
    T: PartialOrd,
{
    let before = v.len();
    while v.len() > floor {
        let pos = (0..v.len()).reduce(|a, b| if key(v[b]).partial_cmp(&key(v[a])).is_some_and(|o| o.is_le()) { b } else { a }).unwrap();
        dropped.push(label(v.remove(pos)));
    }
    v.len() < before
}

pub fn fit_to_pages_with(r: &Resume, sel: &Selection, jd: &Jd, target_pages: u8, start_level: usize, h: &Heuristics, dir: &Path) -> Result<FitResult> {
    let levels: Vec<Layout> = std::iter::once(Layout::default()).chain(Layout::levels()).collect();
    let target = target_pages.max(1) as usize;
    let (mut s, mut level) = (sel.clone(), start_level.min(levels.len() - 1));
    let (mut steps, mut dropped) = (Vec::new(), Vec::new());
    let renders = Cell::new(0usize);
    let render = |s: &Selection, lvl: usize| -> Result<usize> {
        renders.set(renders.get() + 1);
        pdf_pages(&render_pdf_with(&s.apply(r), &levels[lvl], dir)?)
    };
    let can = || renders.get() < h.fit_max_renders;

    if level > 0 {
        steps.push(format!("Started at layout level {level} (dense one-page profile)"));
    }
    let mut pages = render(&s, level)?;
    'fit: {
        if pages <= target {
            break 'fit;
        }
        // 1) secondary items down to their floors
        let mut changed = false;
        let before = s.certs.len();
        if trim(&mut s.certs, h.fit_certs_floor, |i| (weight_of(&format!("{} {}", r.certifications[i].title, r.certifications[i].issuer), jd), year(&r.certifications[i].date_label)), |i| format!("Certification: {}", r.certifications[i].title), &mut dropped) {
            steps.push(format!("Trimmed certifications {before}->{}", s.certs.len()));
            changed = true;
        }
        let before = s.projects.len();
        if trim(&mut s.projects, h.fit_projects_floor, |i| weight_of(&project_doc(&r.projects[i]), jd), |i| format!("Project: {}", r.projects[i].name), &mut dropped) {
            steps.push(format!("Trimmed projects {before}->{}", s.projects.len()));
            changed = true;
        }
        // never drop a whole category: trim each row to its JD-matching items (items are relevance-ordered)
        let n: usize = s.skills.iter().map(|c| c.1.len()).sum();
        for (ci, idx) in s.skills.iter_mut() {
            let keep = idx.iter().filter(|&&k| weight_of(&r.skills[*ci].skills[k], jd) > 0.0).count().max(3);
            idx.truncate(keep);
        }
        if s.skills.iter().map(|c| c.1.len()).sum::<usize>() < n {
            steps.push("Trimmed non-matching skills within rows".to_string());
            changed = true;
        }
        if changed {
            if !can() {
                break 'fit;
            }
            pages = render(&s, level)?;
        }
        // 2) tighten layout
        while pages > target && level + 1 < levels.len() && can() {
            level += 1;
            steps.push(format!("Tightened layout to level {level}"));
            pages = render(&s, level)?;
        }
        if pages <= target || !can() {
            break 'fit;
        }
        // 3) secondary-role bullets down to the secondary floor
        let primary = latest(r, RoleKind::FullTime).filter(|&i| !s.bullets[i].is_empty()).or_else(|| s.bullets.iter().position(|b| !b.is_empty()));
        let rank = |s: &Selection, i: usize| s.bullets[i].clone();
        let drop_role = |s: &mut Selection, i: usize, floor: usize, dropped: &mut Vec<String>| {
            let mut v = rank(s, i);
            let t = trim(&mut v, floor, |j| (weight_of(&r.experience[i].bullets[j], jd), -(j as f32)), |j| format!("{} bullet: {}", r.experience[i].role, clip(&r.experience[i].bullets[j])), dropped);
            s.bullets[i] = v;
            t
        };
        let mut changed = false;
        for i in (0..s.bullets.len()).filter(|&i| Some(i) != primary) {
            changed |= drop_role(&mut s, i, h.fit_secondary_floor, &mut dropped);
        }
        if changed {
            steps.push(format!("Trimmed secondary-role bullets to {} each", h.fit_secondary_floor));
            pages = render(&s, level)?;
        }
        // 4) primary-role bullets: half the excess first, then down to the floor (never below)
        if let (Some(p), true) = (primary, pages > target) {
            let have = s.bullets[p].len();
            for floor in [have - have.saturating_sub(h.fit_primary_floor).div_ceil(2), h.fit_primary_floor] {
                if pages <= target || !can() || s.bullets[p].len() <= floor.max(h.fit_primary_floor) {
                    continue;
                }
                let from = s.bullets[p].len();
                drop_role(&mut s, p, floor.max(h.fit_primary_floor), &mut dropped);
                steps.push(format!("Trimmed primary-role bullets {from}->{}", s.bullets[p].len()));
                pages = render(&s, level)?;
            }
        }
    }
    if pages > target {
        steps.push(format!("Warning: could not fit in {target} pages without losing core experience"));
    }
    Ok(FitResult { resume: s.apply(r), layout_level: level, pages, steps, dropped })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jd::parse_jd;
    use crate::schema::*;

    fn bullet(n: usize) -> String {
        format!("Designed and delivered Python service number {n} on Kubernetes with automated testing, monitoring dashboards and careful capacity planning for downstream teams")
    }

    fn long_resume() -> Resume {
        let s = |x: &str| x.to_string();
        let role = |title: &str, org: &str, label: &str, n: usize| Experience { role: s(title), organization: s(org), date_label: s(label), bullets: (0..n).map(|i| format!("{} {}", bullet(i), title)).collect(), ..Default::default() };
        Resume {
            profile: Profile { name: s("Jane Doe"), email: s("jane@example.com"), role: s("Engineer"), ..Default::default() },
            summary: vec![s("Backend engineer focused on reliable Python services."), s("Hands-on with Kubernetes and Terraform.")],
            experience: vec![
                role("Backend Engineer", "Acme", "Jan 2022 - Present", 8),
                role("Software Intern", "Initech", "Jun 2021 - Aug 2021", 4),
                role("Developer", "Hooli", "2019 - 2021", 4),
                role("Club Lead", "Robotics Club", "2018 - 2019", 4),
            ],
            skills: vec![
                SkillCategory { label: s("Languages"), skills: vec![s("Python"), s("Rust")] },
                SkillCategory { label: s("Cloud"), skills: vec![s("Kubernetes"), s("Terraform")] },
                SkillCategory { label: s("Misc"), skills: vec![s("Juggling")] },
            ],
            projects: (0..3).map(|i| Project { name: format!("Project {i}"), description: s("Tool"), tech_stack: vec![s("Python")], bullets: vec![bullet(i), bullet(i + 9)], ..Default::default() }).collect(),
            certifications: (0..5).map(|i| Certification { title: format!("Certificate {i}"), issuer: s("Issuer"), date_label: format!("{}", 2018 + i), verification_url: Some(s("https://v.example/x")), ..Default::default() }).collect(),
            education: vec![Education { institution: s("State University"), credential: s("BSc"), grade: s("3.8"), date_label: s("2018") }],
            ..Default::default()
        }
    }

    fn full(r: &Resume) -> Selection {
        Selection {
            bullets: r.experience.iter().map(|e| (0..e.bullets.len()).collect()).collect(),
            projects: (0..r.projects.len()).collect(),
            skills: r.skills.iter().enumerate().map(|(i, c)| (i, (0..c.skills.len()).collect())).collect(),
            certs: (0..r.certifications.len()).collect(),
        }
    }

    #[test]
    fn fits_long_resume_protecting_primary_role() {
        let (r, jd) = (long_resume(), parse_jd("Backend Engineer\nRequirements:\n- Python and Kubernetes\n- Terraform"));
        let dir = std::env::temp_dir().join(format!("fit-{}", std::process::id()));
        let sel = full(&r);
        // sanity: the untrimmed content really overflows two pages
        let res = fit_to_pages(&r, &sel, &jd, 1, &dir).unwrap();
        assert!(res.pages <= 1, "{:?}", res.steps);
        let primary = &res.resume.experience[0];
        assert_eq!(primary.role, "Backend Engineer");
        assert!(primary.bullets.len() >= 5, "primary floor");
        assert!(res.steps[0].starts_with("Trimmed certifications 5->3"), "{:?}", res.steps);
        assert!(res.resume.certifications.len() == 3 && res.resume.projects.len() == 1);
        assert!(res.dropped.iter().any(|d| d.starts_with("Certification:")));
        // chronology and summary untouched
        let roles: Vec<_> = res.resume.experience.iter().map(|e| e.role.as_str()).collect();
        let mut sorted = r.clone();
        sorted.sort_reverse_chrono();
        assert!(roles.windows(2).all(|w| sorted.experience.iter().position(|e| e.role == w[0]) < sorted.experience.iter().position(|e| e.role == w[1])));
        assert_eq!(res.resume.summary, r.summary);
        // secondary roles are trimmed before the primary one
        let i_sec = res.steps.iter().position(|s| s.contains("secondary-role"));
        let i_pri = res.steps.iter().position(|s| s.contains("primary-role"));
        assert!(i_pri.is_none() || (i_sec.is_some() && i_sec < i_pri), "{:?}", res.steps);
        // a roomy target needs no trimming
        let easy = fit_to_pages(&r, &sel, &jd, 2, &dir).unwrap();
        assert!(easy.pages <= 2 && easy.resume.experience[0].bullets.len() >= res.resume.experience[0].bullets.len(), "{:?}", easy.steps);
        eprintln!("1p: {:?} pages {} lvl {}\n2p: {:?} pages {} lvl {}", res.steps, res.pages, res.layout_level, easy.steps, easy.pages, easy.layout_level);
    }
}
