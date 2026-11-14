use std::collections::{HashMap, HashSet, VecDeque};

fn distances<F: FnMut(&str) -> Vec<String>>(start: &str, parents: &mut F) -> HashMap<String, usize> {
    let mut dist = HashMap::from([(start.to_string(), 0)]);
    let mut queue = VecDeque::from([start.to_string()]);
    while let Some(n) = queue.pop_front() {
        let d = dist[&n];
        for p in parents(&n) {
            if !dist.contains_key(&p) {
                dist.insert(p.clone(), d + 1);
                queue.push_back(p);
            }
        }
    }
    dist
}

pub fn merge_base<F, E>(a: &str, b: &str, mut parents: F, eligible: E) -> Option<String>
where
    F: FnMut(&str) -> Vec<String>,
    E: Fn(&str) -> bool,
{
    let da = distances(a, &mut parents);
    let db = distances(b, &mut parents);
    let mut common: Vec<(usize, &String)> = da.iter().filter_map(|(k, d)| db.get(k).map(|e| (d + e, k))).filter(|(_, k)| eligible(k)).collect();
    common.sort_by(|x, y| x.0.cmp(&y.0).then_with(|| y.1.cmp(x.1)));
    let mut dominated: HashSet<String> = HashSet::new();
    let mut queue: VecDeque<String> = common.iter().flat_map(|(_, c)| parents(c)).collect();
    while let Some(n) = queue.pop_front() {
        if dominated.insert(n.clone()) {
            queue.extend(parents(&n));
        }
    }
    common.into_iter().map(|(_, c)| c).find(|c| !dominated.contains(*c)).cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn graph(edges: &[(&str, &[&str])]) -> HashMap<String, Vec<String>> {
        edges.iter().map(|(k, v)| (k.to_string(), v.iter().map(|s| s.to_string()).collect())).collect()
    }

    fn mb(g: &HashMap<String, Vec<String>>, a: &str, b: &str) -> Option<String> {
        merge_base(a, b, |n| g.get(n).cloned().unwrap_or_default(), |_| true)
    }

    #[test]
    fn linear_and_fork() {
        let g = graph(&[("s1", &["s0"]), ("s2", &["s1"]), ("p1", &["s1"]), ("p2", &["p1"])]);
        assert_eq!(mb(&g, "p2", "s2").as_deref(), Some("s1"));
        assert_eq!(mb(&g, "s2", "s2").as_deref(), Some("s2"));
        assert_eq!(mb(&g, "s1", "p2").as_deref(), Some("s1"));
    }

    #[test]
    fn update_moves_base() {
        let g = graph(&[("s1", &["s0"]), ("s2", &["s1"]), ("s3", &["s2"]), ("p1", &["s0"]), ("u", &["p1", "s2"]), ("p2", &["u"])]);
        assert_eq!(mb(&g, "p2", "s3").as_deref(), Some("s2"));
    }

    #[test]
    fn shortcut_does_not_win_over_newer_ancestor() {
        let g = graph(&[("y", &["o"]), ("a", &["x", "o"]), ("x", &["y"]), ("b", &["y"])]);
        assert_eq!(mb(&g, "a", "b").as_deref(), Some("y"));
        let g = graph(&[("y", &["o"]), ("a", &["x", "o"]), ("x", &["w"]), ("w", &["y"]), ("b", &["y"])]);
        assert_eq!(mb(&g, "a", "b").as_deref(), Some("y"));
    }

    #[test]
    fn ineligible_skipped_and_disjoint() {
        let g = graph(&[("s1", &["s0"]), ("p1", &["s1"]), ("q1", &["s1"])]);
        let got = merge_base("p1", "q1", |n| g.get(n).cloned().unwrap_or_default(), |n| n != "s1");
        assert_eq!(got.as_deref(), Some("s0"));
        let g2 = graph(&[("a", &["x"]), ("b", &["y"])]);
        assert_eq!(mb(&g2, "a", "b"), None);
    }
}
