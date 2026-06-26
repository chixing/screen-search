pub const HINT_KEYS: &str = "asdfjklghqwertyuiopzxcvbnm";
pub const MAX_PHRASE_WORDS: usize = 6;

#[derive(Clone, Debug)]
pub struct Word {
    pub text: String,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub line: usize,
    pub word: usize,
    pub n: String,
}

#[derive(Clone, Debug)]
pub struct Candidate {
    #[allow(dead_code)]
    pub text: String,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub line: usize,
    pub word: usize,
    pub word_count: usize,
    pub n: String,
    pub hint: String,
    pub selector: String,
    pub hint_typed: String,
    pub sx: i32,
    pub sy: i32,
}

#[derive(Clone, Debug)]
pub struct HintContext {
    pub base: String,
    pub matches: Vec<Candidate>,
}

pub fn norm(s: &str) -> String {
    s.chars()
        .flat_map(char::to_lowercase)
        .filter(|c| c.is_alphanumeric() || *c == '_')
        .collect()
}

fn candidate_from_words(parts: &[Word]) -> Candidate {
    let x1 = parts.iter().map(|w| w.x).fold(f32::INFINITY, f32::min);
    let y1 = parts.iter().map(|w| w.y).fold(f32::INFINITY, f32::min);
    let x2 = parts
        .iter()
        .map(|w| w.x + w.w)
        .fold(f32::NEG_INFINITY, f32::max);
    let y2 = parts
        .iter()
        .map(|w| w.y + w.h)
        .fold(f32::NEG_INFINITY, f32::max);
    let text = parts
        .iter()
        .map(|w| w.text.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    Candidate {
        text: text.clone(),
        x: x1,
        y: y1,
        w: x2 - x1,
        h: y2 - y1,
        line: parts[0].line,
        word: parts[0].word,
        word_count: parts.len(),
        n: norm(&text),
        hint: String::new(),
        selector: String::new(),
        hint_typed: String::new(),
        sx: 0,
        sy: 0,
    }
}

pub fn build_text_candidates(words: &[Word]) -> Vec<Candidate> {
    let mut sorted = words.to_vec();
    sorted.sort_by(|a, b| {
        a.line
            .cmp(&b.line)
            .then(a.word.cmp(&b.word))
            .then(a.x.total_cmp(&b.x))
    });

    let mut candidates = Vec::new();
    let mut start = 0;
    while start < sorted.len() {
        let line = sorted[start].line;
        let mut end = start;
        while end < sorted.len() && sorted[end].line == line {
            end += 1;
        }
        let line_words = &sorted[start..end];
        for i in 0..line_words.len() {
            let max_end = usize::min(line_words.len(), i + MAX_PHRASE_WORDS);
            for j in i + 1..=max_end {
                let c = candidate_from_words(&line_words[i..j]);
                if !c.n.is_empty() {
                    candidates.push(c);
                }
            }
        }
        start = end;
    }
    candidates.sort_by(|a, b| a.y.total_cmp(&b.y).then(a.x.total_cmp(&b.x)));
    candidates
}

fn overlap_ratio(a: &Candidate, b: &Candidate) -> f32 {
    let ax2 = a.x + a.w;
    let ay2 = a.y + a.h;
    let bx2 = b.x + b.w;
    let by2 = b.y + b.h;
    let iw = 0.0_f32.max(ax2.min(bx2) - a.x.max(b.x));
    let ih = 0.0_f32.max(ay2.min(by2) - a.y.max(b.y));
    let intersection = iw * ih;
    if intersection <= 0.0 {
        return 0.0;
    }
    let smaller = 1.0_f32.max((a.w * a.h).min(b.w * b.h));
    intersection / smaller
}

fn suppress_overlapping_matches(matches: Vec<Candidate>) -> Vec<Candidate> {
    let mut sorted = matches;
    sorted.sort_by(|a, b| {
        a.word_count
            .cmp(&b.word_count)
            .then((a.w * a.h).total_cmp(&(b.w * b.h)))
            .then(a.y.total_cmp(&b.y))
            .then(a.x.total_cmp(&b.x))
    });
    let mut kept: Vec<Candidate> = Vec::new();
    'outer: for m in sorted {
        for k in &kept {
            if overlap_ratio(&m, k) > 0.35 {
                continue 'outer;
            }
        }
        kept.push(m);
    }
    kept.sort_by(|a, b| a.y.total_cmp(&b.y).then(a.x.total_cmp(&b.x)));
    kept
}

pub fn text_matches(query: &str, candidates: &[Candidate], exact: bool) -> Vec<Candidate> {
    let mut by_start: Vec<Candidate> = Vec::new();
    for c in candidates {
        let hit = if exact {
            c.n == query
        } else {
            c.n.contains(query)
        };
        if !hit {
            continue;
        }
        if let Some(prev) = by_start
            .iter_mut()
            .find(|m| m.line == c.line && m.word == c.word)
        {
            if c.word_count < prev.word_count {
                *prev = c.clone();
            }
        } else {
            by_start.push(c.clone());
        }
    }
    suppress_overlapping_matches(by_start)
}

fn hint_code(mut index: usize, first_chars: &[char]) -> String {
    let first = if first_chars.is_empty() {
        HINT_KEYS.chars().collect::<Vec<_>>()
    } else {
        first_chars.to_vec()
    };
    let second = HINT_KEYS.chars().collect::<Vec<_>>();
    let width = first.len();
    if index < width * second.len() {
        return format!("{}{}", first[index % width], second[index / width]);
    }
    index -= width * second.len();
    format!(
        "{}{}{}",
        first[index % width],
        second[(index / width) % second.len()],
        second[(index / (width * second.len())) % second.len()]
    )
}

fn next_char_boundary(s: &str, index: usize) -> usize {
    if index >= s.len() {
        return s.len();
    }
    let mut next = index + 1;
    while next < s.len() && !s.is_char_boundary(next) {
        next += 1;
    }
    next
}

pub fn assign_hints(matches: Vec<Candidate>, base_query: &str) -> Vec<Candidate> {
    let mut next_chars = Vec::new();
    for m in &matches {
        let mut start = 0;
        while let Some(haystack) = m.n.get(start..) {
            let Some(idx) = haystack.find(base_query) else {
                break;
            };
            let abs = start + idx;
            let next = abs + base_query.len();
            if let Some(ch) = m.n.get(next..).and_then(|tail| tail.chars().next()) {
                if !next_chars.contains(&ch) {
                    next_chars.push(ch);
                }
            }
            start = next_char_boundary(&m.n, abs);
            if start >= m.n.len() {
                break;
            }
        }
    }
    let first_chars = HINT_KEYS
        .chars()
        .filter(|c| !next_chars.contains(c))
        .collect::<Vec<_>>();
    matches
        .into_iter()
        .enumerate()
        .map(|(i, mut m)| {
            m.hint = hint_code(i, &first_chars);
            m.selector = format!("{}{}", base_query, m.hint);
            m
        })
        .collect()
}

pub fn resolve_selector_matches(
    query: &str,
    candidates: &[Candidate],
    hint_context: Option<&HintContext>,
    exact: bool,
) -> (Vec<Candidate>, Option<HintContext>, String) {
    let text = text_matches(query, candidates, exact);
    if !text.is_empty() {
        let matches = assign_hints(text, query);
        return (
            matches.clone(),
            Some(HintContext {
                base: query.to_string(),
                matches,
            }),
            String::new(),
        );
    }

    if let Some(ctx) = hint_context {
        if query.starts_with(&ctx.base) {
            let suffix = query.get(ctx.base.len()..).unwrap_or("").to_string();
            let matches = ctx
                .matches
                .iter()
                .filter(|m| m.hint.starts_with(&suffix))
                .cloned()
                .map(|mut m| {
                    m.selector = format!("{}{}", ctx.base, m.hint);
                    m.hint_typed = suffix.clone();
                    m
                })
                .collect();
            return (matches, Some(ctx.clone()), suffix);
        }
    }

    (Vec::new(), None, String::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn word(text: &str, x: f32, index: usize) -> Word {
        Word {
            text: text.to_string(),
            x,
            y: 10.0,
            w: text.len() as f32 * 8.0,
            h: 16.0,
            line: 0,
            word: index,
            n: norm(text),
        }
    }

    #[test]
    fn spaces_and_punctuation_are_ignored() {
        let words = vec![word("Open", 10.0, 0), word("File", 50.0, 1)];
        let candidates = build_text_candidates(&words);
        let (matches, _, _) = resolve_selector_matches(&norm("open f"), &candidates, None, false);
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].text, "Open File");
    }

    #[test]
    fn middle_of_word_search_matches_visible_text() {
        let words = vec![word("Settings", 10.0, 0)];
        let candidates = build_text_candidates(&words);
        let (matches, _, suffix) =
            resolve_selector_matches(&norm("ttin"), &candidates, None, false);
        assert_eq!(suffix, "");
        assert_eq!(matches[0].text, "Settings");
    }

    #[test]
    fn hint_assignment_handles_non_ascii_boundaries() {
        let words = vec![word("éé", 10.0, 0), word("éa", 50.0, 1)];
        let candidates = build_text_candidates(&words);
        let (matches, _, suffix) = resolve_selector_matches(&norm("é"), &candidates, None, false);
        assert_eq!(suffix, "");
        assert_eq!(matches.len(), 2);
        assert!(matches.iter().all(|m| !m.hint.is_empty()));
    }

    #[test]
    fn contains_search_suppresses_larger_overlapping_phrases() {
        let words = vec![
            word("Main", 10.0, 0),
            word("app", 48.0, 1),
            word("screen-search-rs.exe", 80.0, 2),
        ];
        let candidates = build_text_candidates(&words);
        let (matches, _, _) = resolve_selector_matches(&norm("app"), &candidates, None, false);
        assert_eq!(
            matches.iter().map(|m| m.text.as_str()).collect::<Vec<_>>(),
            vec!["app"]
        );
    }

    #[test]
    fn selector_suffix_disqualifies_nonmatching_highlights() {
        let words = vec![
            word("Alpha", 10.0, 0),
            word("Also", 80.0, 1),
            word("Alt", 160.0, 2),
        ];
        let candidates = build_text_candidates(&words);
        let (matches, ctx, _) = resolve_selector_matches(&norm("al"), &candidates, None, false);
        assert_eq!(matches.len(), 3);
        let chosen = matches[1].hint.chars().next().unwrap();
        let (narrowed, _, suffix) = resolve_selector_matches(
            &norm(&format!("al{}", chosen)),
            &candidates,
            ctx.as_ref(),
            false,
        );
        assert_eq!(suffix, chosen.to_string());
        assert!(!narrowed.is_empty() && narrowed.len() < matches.len());
    }
}
