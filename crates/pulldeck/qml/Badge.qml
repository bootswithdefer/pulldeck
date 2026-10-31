import QtQuick
import QtQuick.Controls

// Small coloured pill for review/check state. Hidden when there is nothing to say.
Rectangle {
    id: badge
    property string kind: "review"   // "review" | "checks"
    property string value: "none"

    readonly property var spec: ({
        "review": {
            "approved": ["Approved", "#2da44e"],
            "changes_requested": ["Changes requested", "#cf222e"],
            "review_required": ["Review needed", "#bf8700"]
        },
        "checks": {
            "success": ["CI passing", "#2da44e"],
            "failure": ["CI failing", "#cf222e"],
            "pending": ["CI running", "#bf8700"]
        }
    })
    readonly property var entry: spec[kind][value]

    visible: entry !== undefined
    implicitWidth: label.implicitWidth + 16
    implicitHeight: label.implicitHeight + 6
    radius: height / 2
    color: entry ? Qt.alpha(entry[1], 0.18) : "transparent"
    border.color: entry ? entry[1] : "transparent"

    Label {
        id: label
        anchors.centerIn: parent
        text: badge.entry ? badge.entry[0] : ""
        color: badge.entry ? badge.entry[1] : palette.text
        font.pointSize: Qt.application.font.pointSize - 1
    }
}
