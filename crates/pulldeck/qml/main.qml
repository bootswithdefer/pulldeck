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

    // Open pull requests where the user's review is requested; new ones raise a notification.
    PrListModel {
        id: reviews
        announce: true
    }

    // The user's most recently merged or closed pull requests.
    PrListModel {
        id: history
    }

    SignInDialog {
        id: signIn
        deck: backend
    }

    ActionDialog {
        id: actions
        deck: backend
    }

    // How many review requests the user has looked at. A rise while the window is not in front
    // lights the tray icon, so a hidden app still says something is waiting.
    property int seenReviews: 0
    readonly property bool needsAttention: inbox.unread > 0 || reviews.count > seenReviews

    onActiveChanged: {
        if (active) {
            seenReviews = reviews.count;
        }
    }

    Connections {
        target: reviews

        function onCountChanged(): void {
            if (root.active) {
                root.seenReviews = reviews.count;
            }
        }
    }

    readonly property string trayTooltip: qsTr("pulldeck: %1 unread, %2").arg(inbox.unread).arg(reviews.count === 1 ? qsTr("1 review request") : qsTr("%1 review requests").arg(reviews.count))

    onNeedsAttentionChanged: syncTray()
    onTrayTooltipChanged: syncTray()

    function syncTray(): void {
        backend.setTrayState(needsAttention, trayTooltip);
    }

    // The tray icon is drawn by the backend; it only asks for these.
    Connections {
        target: backend

        function onTrayToggleRequested(): void {
            root.toggle();
        }

        function onTrayQuitRequested(): void {
            Qt.quit();
        }
    }

    function toggle(): void {
        if (visible) {
            hide();
        } else {
            show();
            raise();
            requestActivate();
        }
    }

    // Unix seconds, ticked so the "5m ago" labels stay current between refreshes.
    property real now: Date.now() / 1000

    Component.onCompleted: {
        // The inbox must be attached before a server session can start delivering to it.
        backend.attachInbox(inbox);
        backend.attachReviews(reviews);
        backend.attachHistory(history);
        // With a tray, closing the window hides it; without one (a bare window manager) it quits.
        syncTray();
        backend.startTray();
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
            Switch {
                visible: backend.auth === "signedIn"
                text: qsTr("Share what I'm viewing")
                checked: backend.presence
                onToggled: backend.setPresenceEnabled(checked)
            }
            ToolButton {
                text: backend.auth === "signedIn" ? qsTr("Sign out") : qsTr("Sign in…")
                onClicked: backend.auth === "signedIn" ? backend.signOut() : signIn.open()
            }
            ToolButton {
                text: "⚙"
                onClicked: settingsMenu.popup()

                Menu {
                    id: settingsMenu

                    MenuItem {
                        text: qsTr("Desktop notifications")
                        checkable: true
                        checked: backend.notifications
                        onToggled: backend.setNotificationsEnabled(checked)
                    }
                }
            }
            ToolButton {
                text: qsTr("Refresh")
                // The server pushes changes, but not for repositories it cannot see.
                enabled: !backend.loading
                onClicked: backend.refresh()
            }
        }
    }

    ColumnLayout {
        anchors.fill: parent
        spacing: 0

        // The server can reach us but not GitHub on our behalf. The list below may be stale or
        // empty until this is fixed.
        Pane {
            visible: backend.auth === "signedIn" && backend.syncStatus !== "ok"
            Layout.fillWidth: true
            background: Rectangle {
                color: palette.alternateBase
            }

            RowLayout {
                anchors.fill: parent
                spacing: 12

                Label {
                    Layout.fillWidth: true
                    wrapMode: Text.Wrap
                    text: backend.syncStatus === "needsSignin" ? qsTr("GitHub no longer accepts your login, so your pull requests may be out of date.") : qsTr("The pulldeck GitHub App isn't installed on any repositories you can see, so there is nothing to show yet.")
                }
                Button {
                    visible: backend.syncStatus === "needsSignin"
                    text: qsTr("Sign in again")
                    onClicked: signIn.open()
                }
                Button {
                    visible: backend.syncStatus === "notInstalled" && backend.installUrl !== ""
                    text: qsTr("Install the app")
                    onClicked: Qt.openUrlExternally(backend.installUrl)
                }
            }
        }

        TabBar {
            id: tabs
            Layout.fillWidth: true

            TabButton {
                text: qsTr("Pull requests")
            }
            TabButton {
                text: qsTr("Review requested") + (reviews.count > 0 ? " (" + reviews.count + ")" : "")
            }
            TabButton {
                text: qsTr("Inbox") + (inbox.unread > 0 ? " (" + inbox.unread + ")" : "")
            }
            TabButton {
                text: qsTr("History")
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
                    required property bool serverBacked
                    width: ListView.view.width
                    onClicked: {
                        backend.viewing(row.repo, row.number);
                        Qt.openUrlExternally(row.url);
                    }

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
                            // Only pull requests the server tracks can take its actions.
                            visible: backend.auth === "signedIn" && row.serverBacked
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

            ListView {
                id: reviewList
                clip: true
                model: reviews
                spacing: 1
                ScrollBar.vertical: ScrollBar {}

                Label {
                    anchors.centerIn: parent
                    visible: reviewList.count === 0
                    text: qsTr("Nobody is waiting for your review")
                    opacity: 0.6
                }

                delegate: ItemDelegate {
                    id: request
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
                    onClicked: Qt.openUrlExternally(request.url)

                    contentItem: RowLayout {
                        spacing: 12

                        ColumnLayout {
                            spacing: 2
                            Layout.fillWidth: true

                            Label {
                                text: (request.isDraft ? qsTr("[draft] ") : "") + request.title
                                font.bold: true
                                elide: Text.ElideRight
                                Layout.fillWidth: true
                            }
                            Label {
                                text: request.repo + " #" + request.number + qsTr(" by ") + request.author
                                opacity: 0.65
                                font.pointSize: Application.font.pointSize - 1
                            }
                        }

                        Badge {
                            kind: "checks"
                            value: request.checks
                        }
                        Label {
                            text: backend.ago(request.updatedAt, root.now)
                            opacity: 0.6
                            Layout.minimumWidth: 70
                            horizontalAlignment: Text.AlignRight
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

            ListView {
                id: historyList
                clip: true
                model: history
                spacing: 1
                ScrollBar.vertical: ScrollBar {}

                Label {
                    anchors.centerIn: parent
                    visible: historyList.count === 0
                    text: qsTr("No merged or closed pull requests yet")
                    opacity: 0.6
                }

                delegate: ItemDelegate {
                    id: past
                    required property string title
                    required property string repo
                    required property int number
                    required property string url
                    required property real updatedAt
                    width: ListView.view.width
                    onClicked: Qt.openUrlExternally(past.url)

                    contentItem: RowLayout {
                        spacing: 12

                        ColumnLayout {
                            spacing: 2
                            Layout.fillWidth: true

                            Label {
                                text: past.title
                                font.bold: true
                                elide: Text.ElideRight
                                Layout.fillWidth: true
                            }
                            Label {
                                text: past.repo + " #" + past.number
                                opacity: 0.65
                                font.pointSize: Application.font.pointSize - 1
                            }
                        }
                        Label {
                            text: backend.ago(past.updatedAt, root.now)
                            opacity: 0.6
                            Layout.minimumWidth: 70
                            horizontalAlignment: Text.AlignRight
                        }
                    }
                }
            }
        }
    }
}
