//! Search the shipped reference itself so help and its search index evolve together.
use super::{topic, TOPICS};

pub fn search(query: &str) -> Option<String> {
    let terms: Vec<_> = query.split_whitespace().map(str::to_lowercase).collect();
    if query.len() > 256 || terms.is_empty() || terms.len() > 16 {
        return None;
    }
    let mut documents = vec![("", crate::api::HELP.to_owned())];
    documents.extend(TOPICS.iter().map(|(name, summary)| {
        (
            *name,
            format!("{name}: {summary}\n{}", topic(name).unwrap()),
        )
    }));
    let mut matches = Vec::new();
    for (name, text) in &documents {
        for (line_number, line) in text.lines().enumerate() {
            let lower = line.to_lowercase();
            let score: Option<usize> = terms.iter().map(|term| term_score(term, &lower)).sum();
            if let Some(score) = score {
                matches.push((score, *name, line_number, line.trim()));
            }
        }
    }
    matches.sort_unstable();
    if matches.is_empty() {
        return Some("No matching API help. Try fewer or different words.\n".into());
    }
    Some(
        matches
            .iter()
            .take(20)
            .map(|(_, name, _, line)| {
                let suffix = if name.is_empty() {
                    String::new()
                } else {
                    format!(" {name}")
                };
                format!("worminald api help{suffix}\n  {line}\n")
            })
            .collect(),
    )
}

fn term_score(term: &str, line: &str) -> Option<usize> {
    if line
        .split(|c: char| !c.is_alphanumeric())
        .any(|word| word == term)
    {
        return Some(0);
    }
    if line.contains(term) {
        return Some(1);
    }
    // Short words must match literally; otherwise fuzzy matches swamp useful results.
    let length = term.chars().count();
    let limit = match length {
        0..=3 => return None,
        4..=7 => 1,
        _ => 2,
    };
    line.split(|c: char| !c.is_alphanumeric())
        .filter(|word| word.chars().count().abs_diff(length) <= limit)
        .map(|word| distance(term, word))
        .filter(|distance| *distance <= limit)
        .min()
        .map(|distance| distance + 1)
}

fn distance(left: &str, right: &str) -> usize {
    let right: Vec<_> = right.chars().collect();
    let mut row: Vec<_> = (0..=right.len()).collect();
    for (i, a) in left.chars().enumerate() {
        let mut diagonal = row[0];
        row[0] = i + 1;
        for (j, b) in right.iter().enumerate() {
            let previous = row[j + 1];
            row[j + 1] = (diagonal + usize::from(a != *b))
                .min(row[j] + 1)
                .min(previous + 1);
            diagonal = previous;
        }
    }
    row[right.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn searches_authoritative_commands_and_topics() {
        let exact = search("BRACKETED paste").unwrap();
        assert!(exact.contains("worminald api help observe"));
        assert!(exact.contains("bracketed paste"));
        let typo = search("brackted paste").unwrap();
        assert!(typo.contains("worminald api help observe"));
        assert!(search("enrol").unwrap().contains("enroll NAME ENDPOINT"));
        assert!(search("FRAME_FINISH")
            .unwrap()
            .contains("worminald api help session"));
        assert!(search("bash")
            .unwrap()
            .contains("worminald api help shell-bash"));
    }

    #[test]
    fn bounds_queries_and_results_and_requires_all_terms() {
        for query in ["", " \n ", &"a".repeat(257), &"a ".repeat(17)] {
            assert!(search(query).is_none());
        }
        assert!(search("zzzzzzzzzz").unwrap().starts_with("No matching"));
        assert!(search("bracketed zzzzzzzzzz")
            .unwrap()
            .starts_with("No matching"));
        assert!(search("tab").unwrap().lines().count() <= 40);
        assert_eq!(term_score("cat", "catalog"), Some(1));
        assert_eq!(term_score("cat", "cut"), None);
        assert_eq!(term_score("guard", "guard"), Some(0));
        assert_eq!(distance("révision", "revision"), 1);
    }
}
