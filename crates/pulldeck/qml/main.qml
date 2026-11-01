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

    SignInDialog {
        id: signIn
        deck: backend
    }

    ActionDialog {
        id: actions
        deck: backend
    }

    // Unix seconds, ticked so the "5m ago" labels stay current between refreshes.
    property real now: Date.now() / 1000

    Component.onCompleted: {
        // The inbox must be attached before a server session can start delivering to it.
        backend.attachInbox(inbox);
        backend.refresh();
    }

    // Reading a notification in the inbox tells the server.
    Connections {
        target: inbox

        function onAckRequested(notificationId: int): void {
            backend.ackNotification(notificationId);
        }
    }

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
                text: backend.authError !== "" ? backend.authError : backend.status
                opacity: 0.7
                elide: Text.ElideRight
                Layout.fillWidth: true
            }
            Label {
                visible: backend.serverConfigured
                text: backend.connection
                opacity: 0.7
            }
            BusyIndicator {
                running: backend.loading
                visible: backend.loading
                implicitHeight: 24
                implicitWidth: 24
            }
            ToolButton {
                text: backend.auth === "signedIn" ? qsTr("Sign out") : qsTr("Sign in…")
                onClicked: backend.auth === "signedIn" ? backend.signOut() : signIn.open()
            }
            ToolButton {
                text: qsTr("Refresh")
                // The server pushes changes, so there is nothing to refresh by hand.
                visible: !backend.serverConfigured
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
                        // Collaboration actions need a server session.
                        ToolButton {
                            text: "⋯"
                            visible: backend.auth === "signedIn"
                            enabled: backend.connection === "live"
                            onClicked: menu.popup()

                            Menu {
                                id: menu

                                MenuItem {
                                    text: qsTr("Request review…")
                                    onTriggered: actions.show("review", row.repo, row.number)
                                }
                                MenuItem {
                                    text: qsTr("Nudge…")
                                    onTriggered: actions.show("nudge", row.repo, row.number)
                                }
                                MenuItem {
                                    text: qsTr("Message…")
                                    onTriggered: actions.show("message", row.repo, row.number)
                                }
                            }
                        }
                    }
                }
            }

            InboxView {
                model: inbox
                formatter: backend
                now: root.now
                onAcknowledge: notificationId => inbox.ack(notificationId)
            }
        }
    }
}
