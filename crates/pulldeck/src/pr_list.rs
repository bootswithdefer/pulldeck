//! A list of pull requests as a Qt list model: the ones waiting for the user's review, and the
//! user's recently closed ones.
//!
//! These always come from polling GitHub (a server is not involved), once per refresh together
//! with the user's own pull requests. The Qt object itself (`PrListModel`) is declared in
//! `backend.rs`'s bridge, for the same reason as `InboxModel`.

use crate::backend::qobject;
use cxx_qt::CxxQtType;
use cxx_qt_lib::{QByteArray, QHash, QHashPair_i32_QByteArray, QModelIndex, QString, QVariant};
use pulldeck_core::PullRequest;
use std::pin::Pin;

// Same role ids and names as the user's own list, so one kind of delegate reads either model.
const TITLE: i32 = 0x100;
const REPO: i32 = 0x101;
const NUMBER: i32 = 0x102;
const URL: i32 = 0x103;
const AUTHOR: i32 = 0x104;
const IS_DRAFT: i32 = 0x105;
const REVIEW: i32 = 0x106;
const CHECKS: i32 = 0x107;
const UPDATED_AT: i32 = 0x108;

/// Backing state for [`qobject::PrListModel`].
#[derive(Default)]
pub struct PrListModelRust {
    prs: Vec<PullRequest>,
    /// Whether a poll has succeeded yet; until then nothing counts as a new request.
    loaded: bool,
    pub(crate) count: i32,
    /// Whether pull requests new to this list raise a desktop notification.
    pub(crate) announce: bool,
}

impl qobject::PrListModel {
    /// Shows a fresh poll. The list is small and replaced as a whole, but an unchanged poll
    /// touches nothing, so scrolling and selection survive the once-a-minute refresh.
    pub fn replace(mut self: Pin<&mut Self>, prs: Vec<PullRequest>) {
        let was_loaded = std::mem::replace(&mut self.as_mut().rust_mut().loaded, true);
        if self.prs == prs {
            return;
        }
        if self.announce {
            crate::notify::announce_review_requests(was_loaded.then_some(&self.prs[..]), &prs);
        }
        self.as_mut().list_begin_reset_model();
        self.as_mut().rust_mut().prs = prs;
        self.as_mut().list_end_reset_model();
        let count = self.prs.len() as i32;
        tracing::debug!(count, announce = self.announce, "pull request list updated");
        self.as_mut().set_count(count);
    }

    pub fn row_count(&self, _parent: &QModelIndex) -> i32 {
        self.prs.len() as i32
    }

    pub fn role_names(&self) -> QHash<QHashPair_i32_QByteArray> {
        let mut roles = QHash::<QHashPair_i32_QByteArray>::default();
        for (role, name) in [
            (TITLE, "title"),
            (REPO, "repo"),
            (NUMBER, "number"),
            (URL, "url"),
            (AUTHOR, "author"),
            (IS_DRAFT, "isDraft"),
            (REVIEW, "review"),
            (CHECKS, "checks"),
            (UPDATED_AT, "updatedAt"),
        ] {
            roles.insert(role, QByteArray::from(name));
        }
        roles
    }

    pub fn data(&self, index: &QModelIndex, role: i32) -> QVariant {
        let Some(pr) = usize::try_from(index.row())
            .ok()
            .and_then(|row| self.prs.get(row))
        else {
            return QVariant::default();
        };
        let text = |s: &str| QVariant::from(&QString::from(s));
        match role {
            TITLE => text(&pr.title),
            REPO => text(&pr.repo),
            NUMBER => QVariant::from(&(pr.number as i64)),
            URL => text(&pr.url),
            AUTHOR => text(&pr.author),
            IS_DRAFT => QVariant::from(&pr.is_draft),
            REVIEW => text(pr.review.as_str()),
            CHECKS => text(pr.checks.as_str()),
            UPDATED_AT => QVariant::from(&pr.updated_at.timestamp()),
            _ => QVariant::default(),
        }
    }
}
