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

/// The list to show when pull requests come from two places: a server (which tracks only the
/// repositories its GitHub App is installed on) and the client's own polling of GitHub.
///
/// A pull request in both appears once, as the server's copy, except that it takes the branch
/// names from the polled copy: the server does not send them and stacks need them. The result is
/// most recently updated first; on a tie, server copies come first and each source keeps its own
/// order.
pub fn merge(server: &[PullRequest], polled: &[PullRequest]) -> Vec<PullRequest> {
    let mut merged: Vec<PullRequest> = server
        .iter()
        .map(|s| {
            let mut copy = s.clone();
            if let Some(p) = polled.iter().find(|p| same(p, s)) {
                if copy.base_ref.is_empty() {
                    copy.base_ref.clone_from(&p.base_ref);
                }
                if copy.head_ref.is_empty() {
                    copy.head_ref.clone_from(&p.head_ref);
                }
            }
            copy
        })
        .collect();
    merged.extend(
        polled
            .iter()
            .filter(|p| !server.iter().any(|s| same(s, p)))
            .cloned(),
    );
    merged.sort_by_key(|p| std::cmp::Reverse(p.updated_at));
    merged
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ChecksStatus, ReviewStatus};

    #[test]
    fn merge_dedupes_preferring_the_server_and_sorts_newest_first() {
        let at = |s: &str| s.parse().unwrap();
        let mut server_copy = pr(1, "from the server");
        server_copy.updated_at = at("2026-01-02T00:00:00Z");
        let mut polled_copy = pr(1, "from polling");
        polled_copy.updated_at = at("2026-01-05T00:00:00Z");
        let mut only_polled = pr(2, "asu");
        only_polled.updated_at = at("2026-01-03T00:00:00Z");
        let mut older = pr(3, "older");
        older.updated_at = at("2026-01-01T00:00:00Z");

        let merged = merge(
            &[older.clone(), server_copy.clone()],
            &[polled_copy, only_polled.clone()],
        );

        assert_eq!(merged, vec![only_polled, server_copy, older]);
    }

    #[test]
    fn the_server_copy_keeps_the_branches_only_the_polled_copy_knows() {
        let server_copy = pr(1, "from the server");
        let mut polled_copy = pr(1, "from polling");
        polled_copy.base_ref = "main".into();
        polled_copy.head_ref = "feat-1".into();

        let merged = merge(&[server_copy], &[polled_copy]);

        assert_eq!(merged[0].title, "from the server");
        assert_eq!(merged[0].base_ref, "main");
        assert_eq!(merged[0].head_ref, "feat-1");
    }

    #[test]
    fn merge_with_one_empty_side_is_the_other_side() {
        let list = vec![pr(1, "a"), pr(2, "b")];
        assert_eq!(merge(&list, &[]), list);
        assert_eq!(merge(&[], &list), list);
        assert!(merge(&[], &[]).is_empty());
    }

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
            base_ref: String::new(),
            head_ref: String::new(),
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
