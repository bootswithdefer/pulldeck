//! Stacked pull requests: a chain where each PR targets the branch of the one below it.
//!
//! GitHub has no stack object, so a stack is inferred from branch names: PR `B` sits on PR `A`
//! when both are in the same repository and `B` merges into the branch `A` comes from. Tools that
//! make stacks (Graphite, git-town, spr, Sapling) all produce exactly this shape. A PR with
//! several PRs on top of it forms a tree, which is shown the same way.

use crate::PullRequest;
use std::collections::HashMap;

/// Where a pull request sits in its stack.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StackPlace {
    /// How many pull requests are below this one on the path to the bottom of the stack.
    pub depth: usize,
    /// 1-based place in the stack, bottom first.
    pub position: usize,
    /// How many pull requests the stack has.
    pub size: usize,
}

/// A pull request's identity: repository and number.
pub type PrKey = (String, u64);

/// The key of `pr`.
pub fn key(pr: &PullRequest) -> PrKey {
    (pr.repo.clone(), pr.number)
}

/// Orders `prs` so every stack is together, bottom first, and says where each member of a stack
/// sits. Pull requests that are in no stack get no entry and keep their place.
///
/// A stack takes the place of its most recent member in the input, so with the usual
/// newest-first input a stack rises when any of its pull requests is touched.
pub fn arrange(prs: Vec<PullRequest>) -> (Vec<PullRequest>, HashMap<PrKey, StackPlace>) {
    let parent: Vec<Option<usize>> = prs
        .iter()
        .enumerate()
        .map(|(i, pr)| {
            if pr.base_ref.is_empty() {
                return None;
            }
            prs.iter()
                .enumerate()
                .filter(|&(j, other)| {
                    j != i && other.repo == pr.repo && other.head_ref == pr.base_ref
                })
                // Two PRs from the same branch: build on the lower number.
                .min_by_key(|(_, other)| other.number)
                .map(|(j, _)| j)
        })
        .collect();

    let mut children: Vec<Vec<usize>> = vec![Vec::new(); prs.len()];
    for (i, p) in parent.iter().enumerate() {
        if let Some(p) = p {
            children[*p].push(i);
        }
    }
    for list in &mut children {
        list.sort_by_key(|&i| prs[i].number);
    }

    // Each tree is emitted bottom first. Trees are ordered by their earliest member in the input.
    let mut order: Vec<usize> = Vec::with_capacity(prs.len());
    let mut places: HashMap<PrKey, StackPlace> = HashMap::new();
    let mut seen = vec![false; prs.len()];

    let mut roots: Vec<usize> = (0..prs.len()).filter(|&i| parent[i].is_none()).collect();
    // A cycle (A targets B's branch and B targets A's) has no root; break it at its first member.
    let mut emit = |root: usize, order: &mut Vec<usize>, seen: &mut Vec<bool>| {
        let mut tree: Vec<(usize, usize)> = Vec::new(); // (index, depth), bottom first
        let mut pending = vec![(root, 0usize)];
        while let Some((i, depth)) = pending.pop() {
            if seen[i] {
                continue;
            }
            seen[i] = true;
            tree.push((i, depth));
            // Reverse so the lowest-numbered child is taken first.
            pending.extend(children[i].iter().rev().map(|&c| (c, depth + 1)));
        }
        let size = tree.len();
        if size > 1 {
            for (position, &(i, depth)) in tree.iter().enumerate() {
                places.insert(
                    key(&prs[i]),
                    StackPlace {
                        depth,
                        position: position + 1,
                        size,
                    },
                );
            }
        }
        order.extend(tree.iter().map(|&(i, _)| i));
    };

    // Order the trees by where their earliest member was, without moving unstacked rows.
    let first_member = |root: usize| -> usize {
        let mut earliest = root;
        let mut pending = vec![root];
        let mut visited = vec![false; prs.len()];
        while let Some(i) = pending.pop() {
            if std::mem::replace(&mut visited[i], true) {
                continue;
            }
            earliest = earliest.min(i);
            pending.extend(children[i].iter().copied());
        }
        earliest
    };
    roots.sort_by_key(|&r| first_member(r));
    for root in roots {
        emit(root, &mut order, &mut seen);
    }
    for i in 0..prs.len() {
        if !seen[i] {
            emit(i, &mut order, &mut seen);
        }
    }

    let mut slots: Vec<Option<PullRequest>> = prs.into_iter().map(Some).collect();
    let arranged = order.into_iter().filter_map(|i| slots[i].take()).collect();
    (arranged, places)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ChecksStatus, ReviewStatus};

    fn pr(number: u64, base: &str, head: &str) -> PullRequest {
        PullRequest {
            number,
            title: format!("PR {number}"),
            url: String::new(),
            repo: "o/r".into(),
            author: "me".into(),
            is_draft: false,
            review: ReviewStatus::None,
            checks: ChecksStatus::None,
            updated_at: "2026-01-01T00:00:00Z".parse().unwrap(),
            base_ref: base.into(),
            head_ref: head.into(),
        }
    }

    fn numbers(prs: &[PullRequest]) -> Vec<u64> {
        prs.iter().map(|p| p.number).collect()
    }

    fn place(places: &HashMap<PrKey, StackPlace>, number: u64) -> Option<(usize, usize, usize)> {
        places
            .get(&("o/r".to_string(), number))
            .map(|p| (p.depth, p.position, p.size))
    }

    #[test]
    fn a_chain_is_ordered_bottom_first_with_places() {
        // Newest first, as listed: the top of the stack was touched most recently.
        let (arranged, places) = pr_chain();
        assert_eq!(numbers(&arranged), [1, 2, 3]);
        assert_eq!(place(&places, 1), Some((0, 1, 3)));
        assert_eq!(place(&places, 2), Some((1, 2, 3)));
        assert_eq!(place(&places, 3), Some((2, 3, 3)));
    }

    fn pr_chain() -> (Vec<PullRequest>, HashMap<PrKey, StackPlace>) {
        arrange(vec![
            pr(3, "feat-2", "feat-3"),
            pr(1, "main", "feat-1"),
            pr(2, "feat-1", "feat-2"),
        ])
    }

    #[test]
    fn unstacked_pull_requests_are_untouched_and_a_stack_takes_its_newest_place() {
        let (arranged, places) = arrange(vec![
            pr(9, "main", "solo-a"),
            pr(3, "feat-2", "feat-3"),
            pr(8, "main", "solo-b"),
            pr(1, "main", "feat-1"),
            pr(2, "feat-1", "feat-2"),
        ]);
        assert_eq!(numbers(&arranged), [9, 1, 2, 3, 8]);
        assert_eq!(place(&places, 9), None);
        assert_eq!(place(&places, 8), None);
    }

    #[test]
    fn two_pull_requests_on_one_branch_form_a_tree() {
        let (arranged, places) = arrange(vec![
            pr(1, "main", "base"),
            pr(3, "base", "right"),
            pr(2, "base", "left"),
        ]);
        assert_eq!(numbers(&arranged), [1, 2, 3]);
        assert_eq!(place(&places, 2), Some((1, 2, 3)));
        assert_eq!(place(&places, 3), Some((1, 3, 3)));
    }

    #[test]
    fn only_the_same_repository_counts() {
        let mut other = pr(2, "feat-1", "feat-2");
        other.repo = "o/other".into();
        let (arranged, places) = arrange(vec![pr(1, "main", "feat-1"), other]);
        assert_eq!(numbers(&arranged), [1, 2]);
        assert!(places.is_empty());
    }

    #[test]
    fn unknown_branches_never_stack() {
        // The server does not send branches: nothing is inferred, nothing moves.
        let (arranged, places) = arrange(vec![pr(2, "", ""), pr(1, "", "")]);
        assert_eq!(numbers(&arranged), [2, 1]);
        assert!(places.is_empty());
    }

    #[test]
    fn a_cycle_does_not_loop_or_lose_rows() {
        let (arranged, _) = arrange(vec![pr(1, "b", "a"), pr(2, "a", "b"), pr(3, "main", "c")]);
        let mut got = numbers(&arranged);
        got.sort_unstable();
        assert_eq!(got, [1, 2, 3]);
    }

    #[test]
    fn a_pull_request_never_stacks_on_itself() {
        let (arranged, places) = arrange(vec![pr(1, "same", "same")]);
        assert_eq!(numbers(&arranged), [1]);
        assert!(places.is_empty());
    }
}
