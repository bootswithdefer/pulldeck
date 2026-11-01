pragma ComponentBehavior: Bound

import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import io.github.bootswithdefer.pulldeck

ApplicationWindow {
    id: root
    width: 960
    height: 640
    visible: true
    title: backend.user ? qsTr("pulldeck — %1").arg(backend.user) : qsTr("pulldeck")
    color: palette.window

    PrBackend {
        id: backend
    }

    InboxModel {
        id: inbox
    }

    // Unix seconds, ticked so the "5m ago" labels stay current between refreshes.
    property real now: Date.now() / 1000

    Component.onCompleted: backend.refresh()

    Timer {
        interval: 60000
        running: true
        repeat: true
        onTriggered: backend.refresh()
    }

    Timer {
        interval: 30000
        running: true
        repeat: true
        onTriggered: root.now = Date.now() / 1000
    }

    header: ToolBar {
        RowLayout {
            anchors.fill: parent
            anchors.leftMargin: 12
            anchors.rightMargin: 8

            Label {
                text: qsTr("Your pull requests")
                font.bold: true
            }
            Label {
                text: backend.status
                opacity: 0.7
                elide: Text.ElideRight
                Layout.fillWidth: true
            }
            BusyIndicator {
                running: backend.loading
                visible: backend.loading
                implicitHeight: 24
                implicitWidth: 24
            }
            ToolButton {
                text: qsTr("Refresh")
                enabled: !backend.loading
                onClicked: backend.refresh()
            }
        }
    }

    ColumnLayout {
        anchors.fill: parent
        spacing: 0

        TabBar {
            id: tabs
            Layout.fillWidth: true

            TabButton {
                text: qsTr("Pull requests")
            }
            TabButton {
                text: qsTr("Inbox") + (inbox.unread > 0 ? " (" + inbox.unread + ")" : "")
            }
        }

        StackLayout {
            currentIndex: tabs.currentIndex
            Layout.fillWidth: true
            Layout.fillHeight: true

            ListView {
                id: list
                clip: true
                model: backend
                spacing: 1
                ScrollBar.vertical: ScrollBar {}

                Label {
                    anchors.centerIn: parent
                    visible: list.count === 0 && !backend.loading
                    text: qsTr("No open pull requests")
                    opacity: 0.6
                }

                delegate: ItemDelegate {
                    id: row
                    // One per role exposed by PrBackend (see role_names in backend.rs).
                    required property string title
                    required property string repo
                    required property int number
                    required property string url
                    required property string author
                    required property bool isDraft
                    required property string review
                    required property string checks
                    required property real updatedAt
                    width: ListView.view.width
                    onClicked: Qt.openUrlExternally(row.url)

                    contentItem: RowLayout {
                        spacing: 12

                        ColumnLayout {
                            spacing: 2
                            Layout.fillWidth: true

                            Label {
                                text: (row.isDraft ? qsTr("[draft] ") : "") + row.title
                                font.bold: true
                                elide: Text.ElideRight
                                Layout.fillWidth: true
                            }
                            Label {
                                text: row.repo + " #" + row.number
                                opacity: 0.65
                                font.pointSize: Application.font.pointSize - 1
                            }
                        }

                        Badge {
                            kind: "review"
                            value: row.review
                        }
                        Badge {
                            kind: "checks"
                            value: row.checks
                        }
                        Label {
                            text: backend.ago(row.updatedAt, root.now)
                            opacity: 0.6
                            Layout.minimumWidth: 70
                            horizontalAlignment: Text.AlignRight
                        }
                    }
                }
            }

            InboxView {
                model: inbox
                age: timestamp => backend.ago(timestamp, root.now)
                onAcknowledge: notificationId => inbox.ack(notificationId)
            }
        }
    }
}
