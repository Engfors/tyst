//! Phrase boosting for greedy TDT decoding: a port of Klang's `phrase_boost.py` (Pianissimo, CC BY 4.0),
//! itself a port of NeMo's GPU-PB boosting tree (`ContextGraph`, `GPUBoostingTreeModel`, NeMo 2.7,
//! Apache-2.0). A trie over token IDs with Aho-Corasick failure links; each step into a phrase earns
//! a bonus that grows with depth, and the bonus is taken back when a partial match is abandoned.
//!
//! Unlike Klang's dense per-state cache (a vocabulary-sized array per visited state), bonuses are
//! computed sparsely: the tokens that continue a match on the failure chain get their own bonus,
//! every other token the same fallback.

use std::collections::VecDeque;

use super::spm::SpmEncoder;

/// NeMo's recommended TDT defaults (and Klang's).
pub const CONTEXT_SCORE: f64 = 1.0;
pub const DEPTH_SCALING: f64 = 2.0;

#[derive(Debug, Clone)]
struct Node {
    /// `(token, node)`, sorted by token.
    children: Vec<(u32, u32)>,
    token_score: f64,
    node_score: f64,
    is_end: bool,
    fail: u32,
}

impl Node {
    fn child(&self, token: u32) -> Option<u32> {
        self.children.binary_search_by_key(&token, |c| c.0).ok().map(|i| self.children[i].1)
    }
}

/// The boosting tree. State 0 is the root (no partial match).
#[derive(Debug, Clone)]
pub struct BoostTree {
    nodes: Vec<Node>,
}

/// Bonuses from one state: `matched` for the tokens that extend a match (sorted by token), and
/// `fallback` with the root as next state for every other token.
#[derive(Debug, Clone, PartialEq)]
pub struct Scores {
    pub matched: Vec<(u32, f32, u32)>,
    pub fallback: f32,
}

impl Scores {
    /// Bonus and next state for `token`.
    pub fn get(&self, token: u32) -> (f32, u32) {
        match self.matched.binary_search_by_key(&token, |m| m.0) {
            Ok(i) => (self.matched[i].1, self.matched[i].2),
            Err(_) => (self.fallback, 0),
        }
    }
}

impl BoostTree {
    /// Builds the tree from tokenized phrases; exact duplicates are dropped, empty ones ignored.
    pub fn new(phrases: &[Vec<u32>], context_score: f64, depth_scaling: f64) -> Self {
        let root = Node { children: Vec::new(), token_score: 0.0, node_score: 0.0, is_end: false, fail: 0 };
        let mut tree = Self { nodes: vec![root] };
        let mut seen = std::collections::HashSet::new();
        for p in phrases {
            if seen.insert(p) {
                tree.add(p, context_score, depth_scaling);
            }
        }
        tree.fill_fail();
        tree
    }

    /// Number of states, root included.
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// True when no phrase was added.
    pub fn is_empty(&self) -> bool {
        self.nodes.len() == 1
    }

    fn add(&mut self, tokens: &[u32], context_score: f64, depth_scaling: f64) {
        let mut node = 0u32;
        for (i, &token) in tokens.iter().enumerate() {
            let last = i == tokens.len() - 1;
            let parent_score = self.nodes[node as usize].node_score;
            node = match self.nodes[node as usize].child(token) {
                Some(c) => {
                    let child = &mut self.nodes[c as usize];
                    child.token_score = child.token_score.max(context_score);
                    child.node_score = parent_score + child.token_score;
                    child.is_end |= last;
                    c
                }
                None => {
                    let token_score =
                        if i > 0 { context_score * depth_scaling + ((i + 1) as f64).ln() } else { context_score };
                    let id = self.nodes.len() as u32;
                    self.nodes.push(Node {
                        children: Vec::new(),
                        token_score,
                        node_score: parent_score + token_score,
                        is_end: last,
                        fail: 0,
                    });
                    let children = &mut self.nodes[node as usize].children;
                    let at = children.partition_point(|c| c.0 < token);
                    children.insert(at, (token, id));
                    id
                }
            };
        }
    }

    /// Aho-Corasick failure links, breadth first.
    fn fill_fail(&mut self) {
        let mut queue: VecDeque<u32> = self.nodes[0].children.iter().map(|c| c.1).collect();
        while let Some(node) = queue.pop_front() {
            for i in 0..self.nodes[node as usize].children.len() {
                let (token, child) = self.nodes[node as usize].children[i];
                let mut fail = self.nodes[node as usize].fail;
                while fail != 0 && self.nodes[fail as usize].child(token).is_none() {
                    fail = self.nodes[fail as usize].fail;
                }
                self.nodes[child as usize].fail = self.nodes[fail as usize].child(token).unwrap_or(0);
                queue.push_back(child);
            }
        }
    }

    /// Bonus and next state for every token from `state` (Klang's `PhraseBoost.scores`).
    pub fn scores(&self, state: u32) -> Scores {
        let mut matched: Vec<(u32, f32, u32)> = Vec::new();
        let mut node = state;
        let mut taken_back = 0.0f64;
        loop {
            let n = &self.nodes[node as usize];
            for &(token, child) in &n.children {
                // The deepest match on the chain wins.
                if !matched.iter().any(|m| m.0 == token) {
                    matched.push((token, (taken_back + self.nodes[child as usize].token_score) as f32, child));
                }
            }
            if node == 0 {
                break;
            }
            if !n.is_end {
                taken_back += self.nodes[n.fail as usize].node_score - n.node_score;
            }
            node = n.fail;
        }
        matched.sort_unstable_by_key(|m| m.0);
        Scores { matched, fallback: taken_back as f32 }
    }
}

/// Boosting as the decoder applies it: the tree and its weight (Klang's and NeMo's `alpha`).
#[derive(Debug, Clone)]
pub struct PhraseBoost {
    pub tree: BoostTree,
    pub alpha: f32,
}

impl PhraseBoost {
    /// Tokenizes `phrases` (whitespace trimmed and collapsed, since the tokenizer keeps extra
    /// spaces as tokens) and builds the tree with NeMo's defaults. Case matters: `Klang AI` and
    /// `klang ai` are different paths.
    pub fn new<S: AsRef<str>>(phrases: &[S], encoder: &SpmEncoder, alpha: f32) -> Self {
        let tokenized: Vec<Vec<u32>> = phrases
            .iter()
            .map(|p| clean_phrase(p.as_ref()))
            .filter(|p| !p.is_empty())
            .map(|p| encoder.encode(&p))
            .collect();
        Self { tree: BoostTree::new(&tokenized, CONTEXT_SCORE, DEPTH_SCALING), alpha }
    }

    /// Picks the boosted token on a frame where the unboosted argmax was not blank: the argmax of
    /// `logits + alpha * bonus` over the non-blank tokens. Advances `state` and returns the token.
    pub fn choose(&self, token_logits: &[f32], blank: u32, state: &mut u32) -> u32 {
        let scores = self.tree.scores(*state);
        let fallback = self.alpha * scores.fallback;
        let mut matched = scores.matched.iter().peekable();
        let mut best: Option<(u32, f32)> = None;
        for (i, &logit) in token_logits.iter().enumerate() {
            let i = i as u32;
            while matched.next_if(|m| m.0 < i).is_some() {}
            let bonus = match matched.peek() {
                Some(m) if m.0 == i => self.alpha * m.1,
                _ => fallback,
            };
            if i == blank {
                continue;
            }
            let v = logit + bonus;
            if best.is_none_or(|(_, b)| v > b) {
                best = Some((i, v));
            }
        }
        let token = best.map_or(blank, |b| b.0);
        *state = scores.get(token).1;
        token
    }
}

/// Trims, collapses whitespace runs to one space and drops control characters.
pub fn clean_phrase(s: &str) -> String {
    s.split_whitespace()
        .map(|w| w.chars().filter(|c| !c.is_control()).collect::<String>())
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(phrases: &[&[u32]]) -> BoostTree {
        let p: Vec<Vec<u32>> = phrases.iter().map(|p| p.to_vec()).collect();
        BoostTree::new(&p, CONTEXT_SCORE, DEPTH_SCALING)
    }

    fn close(a: f32, b: f64) -> bool {
        (a as f64 - b).abs() < 1e-5
    }

    #[test]
    fn bonuses_grow_with_depth() {
        let t = tree(&[&[1, 2, 3]]);
        assert_eq!(t.len(), 4);
        let s0 = t.scores(0);
        assert_eq!(s0.matched, vec![(1, 1.0, 1)]);
        assert_eq!(s0.fallback, 0.0);
        let s1 = t.scores(1);
        assert!(close(s1.get(2).0, 2.0 + 2f64.ln()));
        // Leaving the phrase after one token takes its bonus back.
        assert_eq!(s1.get(9), (-1.0, 0));
        // ...but restarting it is a fresh match: -1 + 1.
        assert_eq!(s1.get(1), (0.0, 1));
        let s2 = t.scores(2);
        assert!(close(s2.get(3).0, 2.0 + 3f64.ln()));
        assert!(close(s2.fallback, -(1.0 + 2.0 + 2f64.ln())));
    }

    #[test]
    fn completed_phrases_keep_their_bonus() {
        let t = tree(&[&[1, 2]]);
        let end = t.scores(1).get(2).1;
        let s = t.scores(end);
        assert_eq!(s.fallback, 0.0);
        assert_eq!(s.get(1), (1.0, 1));
    }

    #[test]
    fn failure_links_follow_suffixes() {
        // "1 2 3" and "2 3 4": after 1 2, token 3 continues the first phrase; after 1 2 3, token 4
        // continues the second through the failure link to "2 3".
        let t = tree(&[&[1, 2, 3], &[2, 3, 4]]);
        let s12 = t.scores(t.scores(1).get(2).1);
        let (_, n123) = s12.get(3);
        let s123 = t.scores(n123);
        let (bonus, next) = s123.get(4);
        assert_ne!(next, 0);
        // "1 2 3" is complete, so its bonus stays; the step to depth 3 of "2 3 4" adds its own.
        assert!(close(bonus, 2.0 + 3f64.ln()));
        // Anything else abandons the partial "2 3" and gives back what it earned.
        let tokens: Vec<u32> = s123.matched.iter().map(|m| m.0).collect();
        assert_eq!(tokens, vec![1, 2, 4]);
        assert!(close(s123.fallback, -(1.0 + 2.0 + 2f64.ln())));
    }

    #[test]
    fn duplicates_and_empty_phrases_add_nothing() {
        assert_eq!(tree(&[&[1, 2], &[1, 2], &[]]).len(), 3);
        assert!(tree(&[]).is_empty());
    }

    #[test]
    fn choose_rescoring_respects_blank_and_ties() {
        let boost = PhraseBoost { tree: tree(&[&[2, 3]]), alpha: 1.0 };
        let blank = 4;
        let mut state = 0;
        // Token 1 leads by 0.5; the bonus of 1.0 on token 2 (a phrase start) flips it.
        assert_eq!(boost.choose(&[0.0, 2.0, 1.5, 0.0, 9.0], blank, &mut state), 2);
        assert_ne!(state, 0);
        // Blank is never chosen, however high; a tie goes to the lower token like NumPy.
        let mut s = 0;
        let half = PhraseBoost { tree: tree(&[&[2, 3]]), alpha: 0.5 };
        assert_eq!(half.choose(&[0.0, 2.0, 1.5, 0.0, 9.0], blank, &mut s), 1);
        assert_eq!(s, 0);
    }

    #[test]
    fn cleans_phrases() {
        assert_eq!(clean_phrase("  Klang \t AI\n"), "Klang AI");
        assert_eq!(clean_phrase("a\u{7}b"), "ab");
        assert_eq!(clean_phrase(" \u{7} "), "");
    }
}
