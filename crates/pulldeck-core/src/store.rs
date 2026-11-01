//! Turns "the old list" and "the new list" into the minimal per-row changes, so a view can
//! animate and update only what actually changed instead of rebuilding everything.

use crate::PullRequest;

/// One step in turning the old list into the new one. Apply the steps of a [`diff`] in order;
/// each index refers to the list as it is *after* the previous steps.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    /// Remove the row at this index.
    Remove(usize),
    /// Insert this pull request so it ends up at this index.
    Insert(usize, PullRequest),
    /// The row at this index is the same pull request with new data.
    Update(usize, PullRequest),
}

impl Change {
    /// Applies this step to `list`.
    pub fn apply(&self, list: &mut Vec<PullRequest>) {
        match self {
            Self::Remove(index) => {
                list.remove(*index);
            }
            Self::Insert(index, pr) => list.insert(*index, pr.clone()),
            Self::Update(index, pr) => list[*index] = pr.clone(),
        }
    }
}

fn same(a: &PullRequest, b: &PullRequest) -> bool {
    a.repo == b.repo && a.number == b.number
}

/// Computes the changes that turn `current` into `new`, preserving `new`'s order.
///
/// Pull requests are identified by repository and number. Rows that vanished are removed,
/// rows that moved are removed and re-inserted, new rows are inserted and rows whose data
/// changed are updated in place. Identical lists produce no changes.
pub fn diff(current: &[PullRequest], new: &[PullRequest]) -> Vec<Change> {
    let mut changes = Vec::new();
    let mut working = current.to_vec();

    // Drop rows that are gone, back to front so earlier indices stay valid.
    for index in (0..working.len()).rev() {
        if !new.iter().any(|n| same(n, &working[index])) {
            working.remove(index);
            changes.push(Change::Remove(index));
        }
    }

    // Walk the new order; everything before `i` already matches.
    for (i, wanted) in new.iter().enumerate() {
        if working.get(i).is_some_and(|have| same(have, wanted)) {
            if working[i] != *wanted {
                working[i] = wanted.clone();
                changes.push(Change::Update(i, wanted.clone()));
            }
            continue;
        }
        if let Some(from) = working.iter().position(|have| same(have, wanted)) {
            working.remove(from);
            changes.push(Change::Remove(from));
        }
        working.insert(i, wanted.clone());
        changes.push(Change::Insert(i, wanted.clone()));
    }

    // Only reachable if `new` repeats a pull request; keep the result exact regardless.
    while working.len() > new.len() {
        changes.push(Change::Remove(working.len() - 1));
        working.pop();
    }

    changes
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ChecksStatus, ReviewStatus};

    fn pr(number: u64, title: &str) -> PullRequest {
        PullRequest {
            number,
            title: title.into(),
            url: format!("https://github.com/o/r/pull/{number}"),
            repo: "o/r".into(),
            author: "octocat".into(),
            is_draft: false,
            review: ReviewStatus::None,
            checks: ChecksStatus::None,
            updated_at: "2026-01-01T00:00:00Z".parse().unwrap(),
        }
    }

    fn apply_all(start: &[PullRequest], changes: &[Change]) -> Vec<PullRequest> {
        let mut list = start.to_vec();
        for change in changes {
            change.apply(&mut list);
        }
        list
    }

    #[test]
    fn identical_lists_need_no_changes() {
        let list = vec![pr(1, "a"), pr(2, "b")];
        assert!(diff(&list, &list).is_empty());
        assert!(diff(&[], &[]).is_empty());
    }

    #[test]
    fn first_load_inserts_everything_in_order() {
        let new = vec![pr(1, "a"), pr(2, "b")];
        assert_eq!(
            diff(&[], &new),
            vec![Change::Insert(0, pr(1, "a")), Change::Insert(1, pr(2, "b"))]
        );
    }

    #[test]
    fn closed_pull_requests_are_removed() {
        let current = vec![pr(1, "a"), pr(2, "b"), pr(3, "c")];
        let new = vec![pr(2, "b")];
        // Back to front, so the first index is still valid after the second removal.
        assert_eq!(
            diff(&current, &new),
            vec![Change::Remove(2), Change::Remove(0)]
        );
    }

    #[test]
    fn changed_data_updates_in_place() {
        let current = vec![pr(1, "a"), pr(2, "b")];
        let new = vec![pr(1, "a"), pr(2, "b renamed")];
        assert_eq!(
            diff(&current, &new),
            vec![Change::Update(1, pr(2, "b renamed"))]
        );
    }

    #[test]
    fn a_pr_that_jumps_to_the_top_is_moved_not_rebuilt() {
        let current = vec![pr(1, "a"), pr(2, "b"), pr(3, "c")];
        let new = vec![pr(3, "c"), pr(1, "a"), pr(2, "b")];
        assert_eq!(
            diff(&current, &new),
            vec![Change::Remove(2), Change::Insert(0, pr(3, "c"))]
        );
    }

    #[test]
    fn same_number_in_another_repo_is_a_different_pr() {
        let mut other = pr(1, "a");
        other.repo = "o/other".into();
        let current = vec![pr(1, "a")];
        let new = vec![other.clone()];
        assert_eq!(apply_all(&current, &diff(&current, &new)), new);
        assert_eq!(diff(&current, &new).len(), 2);
    }

    /// Every ordered selection of up to four ids (65 lists), so every old/new pair (4,225)
    /// covers insert, remove, move and update in all combinations.
    fn all_lists() -> Vec<Vec<u64>> {
        fn extend(prefix: &mut Vec<u64>, out: &mut Vec<Vec<u64>>) {
            out.push(prefix.clone());
            for id in 1..=4 {
                if !prefix.contains(&id) {
                    prefix.push(id);
                    extend(prefix, out);
                    prefix.pop();
                }
            }
        }
        let mut out = Vec::new();
        extend(&mut Vec::new(), &mut out);
        out
    }

    #[test]
    fn applying_the_diff_always_reproduces_the_new_list() {
        let lists = all_lists();
        assert_eq!(lists.len(), 65);
        for old_ids in &lists {
            for new_ids in &lists {
                let current: Vec<_> = old_ids.iter().map(|&n| pr(n, "old")).collect();
                // Even ids also change content, so updates mix with moves and inserts.
                let new: Vec<_> = new_ids
                    .iter()
                    .map(|&n| pr(n, if n % 2 == 0 { "new" } else { "old" }))
                    .collect();
                let changes = diff(&current, &new);
                assert_eq!(
                    apply_all(&current, &changes),
                    new,
                    "old {old_ids:?} -> new {new_ids:?} via {changes:?}"
                );
            }
        }
    }
}
