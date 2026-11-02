//! The notification inbox as a Qt list model, newest first.
//!
//! The state lives in `pulldeck_core::session::Inbox` (deduplication, acknowledgements); this
//! file only maps it onto rows and tells attached views about each change. The Qt object itself
//! (`InboxModel`) is declared in `backend.rs`'s bridge: separate bridges that both derive from
//! `QAbstractListModel` generate duplicate cast symbols and fail to link.

use crate::backend::qobject;
use cxx_qt::CxxQtType;
use cxx_qt_lib::{QByteArray, QHash, QHashPair_i32_QByteArray, QModelIndex, QString, QVariant};
use pulldeck_core::session::{Inbox, Notification, NotificationKind};
use std::pin::Pin;

// Role ids start at Qt::UserRole (0x100) so they never collide with Qt's built-in roles.
const ID: i32 = 0x100;
const FROM: i32 = 0x101;
const KIND: i32 = 0x102;
const REPO: i32 = 0x103;
const NUMBER: i32 = 0x104;
const NOTE: i32 = 0x105;
const CREATED_AT: i32 = 0x106;
const ACKED: i32 = 0x107;

/// Backing state for [`qobject::InboxModel`].
#[derive(Default)]
pub struct InboxModelRust {
    pub(crate) inbox: Inbox,
    pub(crate) unread: i32,
}

fn kind_name(kind: NotificationKind) -> &'static str {
    match kind {
        NotificationKind::ReviewRequested => "review_requested",
        NotificationKind::Nudge => "nudge",
        NotificationKind::Message => "message",
        NotificationKind::Other => "other",
    }
}

impl qobject::InboxModel {
    /// Adds a notification at the top. Redeliveries (same id) are ignored.
    // Not called until a server connection exists; kept so the model is complete and testable.
    #[allow(dead_code)]
    pub fn push(mut self: Pin<&mut Self>, notification: Notification) {
        if self.inbox.row_of(notification.id).is_some() {
            return;
        }
        // Newest first: the model shows the inbox reversed, so a new item is always row 0.
        // SAFETY: paired with `end_insert_rows` below, with no early exit in between.
        unsafe {
            self.as_mut()
                .begin_insert_rows(&QModelIndex::default(), 0, 0)
        };
        crate::notify::announce(&notification);
        self.as_mut().rust_mut().inbox.insert(notification);
        unsafe { self.as_mut().end_insert_rows() };
        self.as_mut().refresh_unread();
        tracing::debug!(
            rows = self.inbox.items().len(),
            unread = self.unread,
            "inbox: notification added"
        );
    }

    /// Forgets every notification (sign-out).
    pub fn reset(mut self: Pin<&mut Self>) {
        self.as_mut().begin_reset_model();
        self.as_mut().rust_mut().inbox = Inbox::default();
        self.as_mut().end_reset_model();
        self.as_mut().refresh_unread();
    }

    /// Marks a notification acknowledged. Returns whether anything changed.
    pub fn mark_acked(mut self: Pin<&mut Self>, id: u64) -> bool {
        // The row does not change when a notification is acknowledged, so look it up once.
        let Some(row) = self.inbox.row_of(id) else {
            return false;
        };
        if !self.as_mut().rust_mut().inbox.ack(id) {
            return false;
        }
        let index = self.inbox_index(row as i32, 0, &QModelIndex::default());
        self.as_mut()
            .inbox_data_changed(&index, &index, &Default::default());
        self.as_mut().refresh_unread();
        tracing::debug!(
            row,
            unread = self.unread,
            "inbox: notification acknowledged"
        );
        true
    }

    /// The user read this notification.
    pub fn ack(mut self: Pin<&mut Self>, id: i64) {
        let Ok(id) = u64::try_from(id) else { return };
        if self.as_mut().mark_acked(id) {
            self.ack_requested(id as i64);
        }
    }

    fn refresh_unread(mut self: Pin<&mut Self>) {
        let unread = self.inbox.unacked() as i32;
        self.as_mut().set_unread(unread);
    }

    pub fn row_count(&self, _parent: &QModelIndex) -> i32 {
        self.inbox.items().len() as i32
    }

    pub fn role_names(&self) -> QHash<QHashPair_i32_QByteArray> {
        let mut roles = QHash::<QHashPair_i32_QByteArray>::default();
        for (role, name) in [
            // Not "id": that is a reserved word in QML and could not be a delegate property.
            (ID, "notificationId"),
            (FROM, "from"),
            (KIND, "kind"),
            (REPO, "repo"),
            (NUMBER, "number"),
            (NOTE, "note"),
            (CREATED_AT, "createdAt"),
            (ACKED, "acked"),
        ] {
            roles.insert(role, QByteArray::from(name));
        }
        roles
    }

    pub fn data(&self, index: &QModelIndex, role: i32) -> QVariant {
        let Some(n) = usize::try_from(index.row())
            .ok()
            .and_then(|row| self.inbox.at_row(row))
        else {
            return QVariant::default();
        };
        let text = |s: &str| QVariant::from(&QString::from(s));
        match role {
            ID => QVariant::from(&(n.id as i64)),
            FROM => text(&n.from),
            KIND => text(kind_name(n.kind)),
            REPO => text(&n.pr.repo),
            NUMBER => QVariant::from(&(n.pr.number as i64)),
            NOTE => text(n.note.as_deref().unwrap_or_default()),
            CREATED_AT => QVariant::from(&n.created_at.timestamp()),
            ACKED => QVariant::from(&self.inbox.is_acked(n.id)),
            _ => QVariant::default(),
        }
    }
}
