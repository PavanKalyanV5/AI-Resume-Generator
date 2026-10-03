//! Deterministic arrangement (section and item order) derived from the knowledge graph's intervals.
//! Newest first (current, then end, then start), unknown last, stable; undated projects/certs sort by the date
//! inherited from their linked role. Skill rows: JD relevance, then recency of last use, then proficiency.
use crate::graph::{build_graph, EdgeKind, Graph};
use crate::jd::{canonical, term_counts, Jd};
use crate::profile_model::ProfileFacts;
use crate::schema::Resume;
use crate::select::Selection;
use crate::seniority::{now_month, Interval};
use serde::Serialize;
use std::cmp::Reverse;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum SectionId {
    Summary,
    Experience,
    Education,
    Skills,
    Projects,
    Certifications,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Reason {
    /// Graph-style id: `role:i`, `edu:i`, `project:i`, `cert:i`, `skill_row:i`.
    pub item_id: String,
    pub why: String,
}

/// Orders are permutations of indices into the resume that was arranged (the selected view).
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Arrangement {
    pub sections: Vec<SectionId>,
    pub experience_order: Vec<usize>,
    pub education_order: Vec<usize>,
    pub project_order: Vec<usize>,
    pub cert_order: Vec<usize>,
    pub skill_rows_order: Vec<usize>,
    pub reasons: Vec<Reason>,
}

/// Arrange what `sel` picks from `r`.
pub fn arrange(r: &Resume, sel: &Selection, jd: Option<&Jd>, facts: Option<&ProfileFacts>) -> Arrangement {
    arrange_resume(&sel.pick(r), jd, facts)
}

pub fn arrange_resume(v: &Resume, jd: Option<&Jd>, facts: Option<&ProfileFacts>) -> Arrangement {
    let (g, now) = (build_graph(v, None), now_month());
    let mut a = Arrangement::default();
    let mut order = |prefix: &str, ivs: Vec<Option<Interval>>, inferred: &dyn Fn(usize) -> Option<String>| {
        let mut idx: Vec<usize> = (0..ivs.len()).collect();
        idx.sort_by_cached_key(|&i| Reverse(ivs[i].and_then(|x| x.sort_key(now))));
        for &i in &idx {
            let why = match (ivs[i], inferred(i)) {
                (_, Some(role)) => format!("inherited date from role {role}"),
                (Some(x), _) if x.current => "current".into(),
                (Some(_), _) => "newest first".into(),
                (None, _) => "undated, kept last".into(),
            };
            a.reasons.push(Reason { item_id: format!("{prefix}:{i}"), why });
        }
        idx
    };
    let iv = |g: &Graph, id: String| g.nodes.iter().find(|n| n.id == id).and_then(|n| n.interval);
    let inherited = |g: &Graph, id: String| {
        let n = g.nodes.iter().find(|n| n.id == id).filter(|n| n.inferred)?;
        let e = g.edges.iter().find(|e| e.kind == EdgeKind::During && e.from == n.id)?;
        g.nodes.iter().find(|x| x.id == e.to).map(|x| x.label.clone())
    };
    let n = |p: &str, len: usize, g: &Graph| (0..len).map(|i| iv(g, format!("{p}:{i}"))).collect::<Vec<_>>();
    a.experience_order = order("role", n("role", v.experience.len(), &g), &|_| None);
    a.education_order = order("edu", v.education.iter().map(|e| Interval::parse(&e.date_label)).collect(), &|_| None);
    a.project_order = order("project", n("project", v.projects.len(), &g), &|i| inherited(&g, format!("project:{i}")));
    a.cert_order = order("cert", n("cert", v.certifications.len(), &g), &|i| inherited(&g, format!("cert:{i}")));

    // skill rows: (JD relevance, newest last use, best proficiency), all descending; ties keep the incoming order
    let key = |c: &crate::schema::SkillCategory| {
        let rel = jd.map_or(0.0, |jd| term_counts(&c.skills.join(" "), jd).iter().zip(&jd.requirements).filter(|(n, _)| **n > 0).map(|(_, q)| q.weight).sum::<f32>());
        let fs: Vec<_> = facts.into_iter().flat_map(|f| c.skills.iter().filter_map(|s| f.skill(&canonical(s)?))).collect();
        (rel, fs.iter().filter_map(|s| s.last_seen).max(), fs.iter().map(|s| s.proficiency).fold(0.0, f32::max))
    };
    let keys: Vec<_> = v.skills.iter().map(key).collect();
    a.skill_rows_order = (0..keys.len()).collect();
    a.skill_rows_order.sort_by(|&x, &y| keys[y].0.total_cmp(&keys[x].0).then(keys[y].1.cmp(&keys[x].1)).then(keys[y].2.total_cmp(&keys[x].2)));
    for &i in &a.skill_rows_order {
        let why = if keys[i].0 > 0.0 { "JD relevance" } else if keys[i].1.is_some() { "recent use" } else { "original order" };
        a.reasons.push(Reason { item_id: format!("skill_row:{i}"), why: why.into() });
    }

    // fixed section order (experience before projects), empty sections omitted
    let has = [!v.summary.is_empty(), !v.experience.is_empty(), !v.education.is_empty(), !v.skills.is_empty(), !v.projects.is_empty(), !v.certifications.is_empty()];
    use SectionId::*;
    a.sections = [Summary, Experience, Education, Skills, Projects, Certifications].into_iter().zip(has).filter(|x| x.1).map(|x| x.0).collect();
    a
}

pub fn apply_arrangement(v: &Resume, a: &Arrangement) -> Resume {
    fn pick<T: Clone>(items: &[T], order: &[usize]) -> Vec<T> {
        order.iter().filter_map(|&i| items.get(i).cloned()).collect()
    }
    Resume {
        experience: pick(&v.experience, &a.experience_order),
        education: pick(&v.education, &a.education_order),
        projects: pick(&v.projects, &a.project_order),
        certifications: pick(&v.certifications, &a.cert_order),
        skills: pick(&v.skills, &a.skill_rows_order),
        ..v.clone()
    }
}

/// The single entry point renderers and `Selection::apply` use.
pub fn arranged(v: &Resume) -> Resume {
    apply_arrangement(v, &arrange_resume(v, None, None))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::*;
    use crate::select::testutil::fake;

    fn exp(role: &str, label: &str, b: &str) -> Experience {
        Experience { role: role.into(), date_label: label.into(), bullets: vec![b.into()], ..Default::default() }
    }

    #[test]
    fn parser_table() {
        let p = |s| Interval::parse(s);
        let iv = |s, e, current, expires| Some(Interval { start: s, end: e, current, expires });
        assert_eq!(p("Jun 2024 – Present"), iv(Some((2024, 6)), None, true, None));
        assert_eq!(p("Jan. 2024 - Jun. 2024"), iv(Some((2024, 1)), Some((2024, 6)), false, None));
        assert_eq!(p("2020 – 2024"), iv(Some((2020, 1)), Some((2024, 12)), false, None));
        assert_eq!(p("Sep 2026"), iv(Some((2026, 9)), Some((2026, 9)), false, None));
        assert_eq!(p("Jan 2024 · Expires Jan 2027"), iv(Some((2024, 1)), Some((2024, 1)), false, Some((2027, 1))));
        assert_eq!(p("Jul 2022"), iv(Some((2022, 7)), Some((2022, 7)), false, None));
        assert_eq!(p("unknown"), None);
        assert_eq!(p("2025 - 2024").and_then(|i| i.range(0)), None);
    }

    #[test]
    fn newest_first_current_first_unknown_last_stable() {
        let r = Resume {
            experience: vec![exp("U1", "n/a", "x"), exp("Old", "2018 - 2019", "x"), exp("Cur", "Jan 2020 - Present", "x"), exp("U2", "", "x"), exp("New", "2021 - 2022", "x")],
            ..Default::default()
        };
        let a = arrange_resume(&r, None, None);
        assert_eq!(a.experience_order, vec![2, 4, 1, 0, 3]);
        assert_eq!(a.reasons[0], Reason { item_id: "role:2".into(), why: "current".into() });
        assert!(a.reasons.iter().any(|x| x.item_id == "role:0" && x.why == "undated, kept last"));
        let mut s = r.clone();
        s.sort_reverse_chrono();
        assert_eq!(s, apply_arrangement(&r, &a));
    }

    #[test]
    fn undated_project_inherits_role_date() {
        let r = Resume {
            experience: vec![exp("Dev", "Jan 2020 - Dec 2021", "Built kubernetes tooling"), exp("Dev", "Jan 2018 - Dec 2019", "Wrote java services")],
            projects: vec![
                Project { name: "Spring thing".into(), tech_stack: vec!["Java".into()], date_label: "2019".into(), ..Default::default() },
                Project { name: "Kube tool".into(), tech_stack: vec!["Kubernetes".into()], ..Default::default() },
                Project { name: "Mystery".into(), tech_stack: vec!["Cobol".into()], ..Default::default() },
            ],
            ..Default::default()
        };
        let g = build_graph(&r, None);
        let n = |id: &str| g.nodes.iter().find(|n| n.id == id).unwrap().clone();
        assert!(n("project:1").inferred && n("project:1").interval == n("role:0").interval);
        assert!(!n("project:0").inferred && n("project:2").interval.is_none());
        let a = arrange_resume(&r, None, None);
        assert_eq!(a.project_order, vec![1, 0, 2]);
        assert!(a.reasons.iter().any(|x| x.item_id == "project:1" && x.why == "inherited date from role Dev"));
    }

    #[test]
    fn skill_rows_use_recency_then_jd() {
        let r = Resume { skills: vec![SkillCategory { label: "Old".into(), skills: vec!["Java".into()] }, SkillCategory { label: "New".into(), skills: vec!["Kubernetes".into()] }], ..Default::default() };
        let mut with = r.clone();
        with.experience = vec![exp("A", "2015 - 2016", "Used Java"), exp("B", "2022 - 2023", "Used Kubernetes")];
        let facts = crate::profile_model::build_facts(&with, (2024, 1));
        assert_eq!(arrange_resume(&r, None, None).skill_rows_order, vec![0, 1]);
        let a = arrange_resume(&r, None, Some(&facts));
        assert_eq!(a.skill_rows_order, vec![1, 0]);
        assert!(a.reasons.iter().any(|x| x.item_id == "skill_row:1" && x.why == "recent use"));
        let jd = crate::jd::parse_jd("Backend\nJava");
        assert_eq!(arrange_resume(&r, None, Some(&facts)).skill_rows_order, vec![1, 0]);
        assert_eq!(arrange_resume(&r, Some(&jd), Some(&facts)).skill_rows_order, vec![0, 1]);
    }

    #[test]
    fn renderers_unchanged_for_sorted_data() {
        let mut sorted = fake();
        sorted.sort_reverse_chrono();
        assert_eq!(arranged(&sorted), sorted);
        assert_eq!(crate::latex::render_tex(&fake()), crate::latex::render_tex(&sorted));
        let sec = arrange_resume(&sorted, None, None).sections;
        assert!(sec.iter().position(|x| *x == SectionId::Experience) < sec.iter().position(|x| *x == SectionId::Projects));
    }
}
