pragma ComponentBehavior: Bound

import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import io.github.bootswithdefer.pulldeck

// What other people sent you about pull requests, newest first. Bind `model` to an InboxModel.
ListView {
    id: view

    // Turns timestamps into text like "5m ago" (typed rather than a function property, which the
    // QML linter flags), relative to `now` in Unix seconds.
    property PrBackend formatter
    property real now: 0

    // Emitted when the user reads a notification; the owner tells the server.
    signal acknowledge(int notificationId)

    clip: true
    spacing: 1
    ScrollBar.vertical: ScrollBar {}

    Label {
        anchors.centerIn: parent
        visible: view.count === 0
        text: qsTr("No notifications")
        opacity: 0.6
    }

    delegate: ItemDelegate {
        id: row

        // One per role exposed by InboxModel (see role_names in inbox.rs).
        required property int notificationId
        required property string from
        required property string kind
        required property string repo
        required property int number
        required property string note
        required property real createdAt
        required property bool acked

        readonly property string summary: {
            switch (kind) {
            case "review_requested":
                return qsTr("%1 asked you to review").arg(from);
            case "nudge":
                return qsTr("%1 nudged you about").arg(from);
            case "message":
                return qsTr("%1 wrote to you about").arg(from);
            default:
                return qsTr("%1 sent you a notification about").arg(from);
            }
        }

        width: ListView.view.width
        onClicked: {
            view.acknowledge(row.notificationId);
            Qt.openUrlExternally("https://github.com/" + row.repo + "/pull/" + row.number);
        }

        contentItem: RowLayout {
            spacing: 12

            // Unread marker.
            Rectangle {
                implicitWidth: 8
                implicitHeight: 8
                radius: 4
                color: palette.highlight
                opacity: row.acked ? 0 : 1
            }

            ColumnLayout {
                spacing: 2
                Layout.fillWidth: true

                Label {
                    text: row.summary + " " + row.repo + " #" + row.number
                    font.bold: !row.acked
                    elide: Text.ElideRight
                    Layout.fillWidth: true
                }
                Label {
                    visible: row.note.length > 0
                    text: row.note
                    opacity: 0.7
                    elide: Text.ElideRight
                    Layout.fillWidth: true
                }
            }

            Label {
                text: view.formatter ? view.formatter.ago(row.createdAt, view.now) : ""
                opacity: 0.6
                Layout.minimumWidth: 70
                horizontalAlignment: Text.AlignRight
            }
        }
    }
}
