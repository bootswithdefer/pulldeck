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

    PrBackend { id: backend }

    readonly property var prs: JSON.parse(backend.prsJson)

    Component.onCompleted: backend.refresh()

    Timer {
        interval: 60000
        running: true
        repeat: true
        onTriggered: backend.refresh()
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

    ListView {
        id: list
        anchors.fill: parent
        clip: true
        model: root.prs
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
            required property var modelData
            width: ListView.view.width
            onClicked: Qt.openUrlExternally(modelData.url)

            contentItem: RowLayout {
                spacing: 12

                ColumnLayout {
                    spacing: 2
                    Layout.fillWidth: true

                    Label {
                        text: (row.modelData.is_draft ? qsTr("[draft] ") : "") + row.modelData.title
                        font.bold: true
                        elide: Text.ElideRight
                        Layout.fillWidth: true
                    }
                    Label {
                        text: row.modelData.repo + " #" + row.modelData.number
                        opacity: 0.65
                        font.pointSize: Qt.application.font.pointSize - 1
                    }
                }

                Badge {
                    kind: "review"
                    value: row.modelData.review
                }
                Badge {
                    kind: "checks"
                    value: row.modelData.checks
                }
                Label {
                    text: row.modelData.updated_ago
                    opacity: 0.6
                    Layout.minimumWidth: 70
                    horizontalAlignment: Text.AlignRight
                }
            }
        }
    }
}
